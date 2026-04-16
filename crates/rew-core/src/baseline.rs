//! Baseline resolution for file change tracking.
//!
//! Determines whether a file existed before a task started and provides
//! the best available content hash for that prior state. Used by both
//! the CLI hook and the daemon to ensure consistent `change_type` and
//! `old_hash` determination.

use crate::db::Database;
use crate::objects::ObjectStore;
use crate::pre_tool_store::{get_pre_tool_hash_in, pre_tool_store_root_for_objects_root};
use crate::types::ChangeType;
use std::path::{Path, PathBuf};

/// Result of resolving a file's baseline state before the current task.
pub struct Baseline {
    /// Whether the file existed before this task started.
    pub existed: bool,
    /// Content hash (SHA-256) of the file's prior state.
    pub hash: Option<String>,
}

fn resolve_live_file_index_baseline_with_store(
    db: &Database,
    path_str: &str,
    store: Option<&ObjectStore>,
) -> Option<Baseline> {
    // file_index APIs only expose live rows here (`exists_now = 1`), which is
    // exactly what we need when a historical `changes` row says Deleted but a
    // later restore has already re-hydrated the path back to live state.
    let (_, fast_hash, content_hash) = match db.get_file_index(path_str) {
        Ok(Some(entry)) => entry,
        _ => return None,
    };

    if let (Some(store), Some(content_hash)) = (store, content_hash.as_deref()) {
        if store.exists(content_hash) {
            return Some(Baseline {
                existed: true,
                hash: Some(content_hash.to_string()),
            });
        }
    }

    if let Some(store) = store {
        if let Some(backup_path) = store.retrieve(&fast_hash) {
            if let Ok(sha) = store.store(&backup_path) {
                let _ = db.update_file_index_content_hash(path_str, &sha);
                return Some(Baseline {
                    existed: true,
                    hash: Some(sha),
                });
            }
        }
    }

    Some(Baseline {
        existed: true,
        hash: None,
    })
}

/// Resolve the baseline state for a file in the context of a task.
///
/// Uses a 5-level fallback chain:
///
/// 1. Existing change record in this task → checks change_type:
///    - Created → file didn't exist before task → existed=false
///    - Modified/Renamed/Deleted → preserves original old_hash
/// 2. pre-tool file store → captured right before AI edit (hook only)
/// 3. Latest change from a PREVIOUS task → checks change_type to detect deletions
/// 4. file_index table → startup scan baseline (lazy SHA-256 upgrade from fast hash)
/// 5. No record → file never seen by rew
pub fn resolve_baseline(
    db: &Database,
    task_id: &str,
    file_path: &Path,
    session_key: Option<&str>,
) -> Baseline {
    resolve_baseline_with_objects_root(
        db,
        task_id,
        file_path,
        session_key,
        crate::rew_home_dir().join("objects"),
    )
}

/// Variant of `resolve_baseline` that reads objects from a caller-provided root.
/// Used by isolated tests so they do not depend on the user's real `~/.rew`.
pub fn resolve_baseline_with_objects_root(
    db: &Database,
    task_id: &str,
    file_path: &Path,
    session_key: Option<&str>,
    objects_root: PathBuf,
) -> Baseline {
    let path_str = file_path.to_string_lossy().to_string();
    let pre_tool_root = pre_tool_store_root_for_objects_root(&objects_root);
    let store = ObjectStore::new(objects_root).ok();

    // 1. This task already has a record for this file → check change_type
    if let Ok(Some((change_type_str, old_hash))) =
        db.get_task_file_baseline_info(task_id, file_path)
    {
        if change_type_str == "created" {
            return Baseline {
                existed: false,
                hash: None,
            };
        }
        // Ephemeral delete: old_hash=None means the file was created and
        // deleted within this task (never existed before the task started).
        if change_type_str == "deleted" && old_hash.is_none() {
            return Baseline {
                existed: false,
                hash: None,
            };
        }
        return Baseline {
            existed: true,
            hash: old_hash,
        };
    }

    // 2. pre_tool backup captured a hash → file existed before edit (hook path only)
    if let Some(sk) = session_key {
        if let Ok(Some(h)) = get_pre_tool_hash_in(&pre_tool_root, sk, &path_str) {
            return Baseline {
                existed: true,
                hash: Some(h),
            };
        }
    }

    // 3. Latest change from a PREVIOUS task (excludes current task_id to avoid
    //    circular reference — reading our own Created record as "baseline").
    //
    //    Both branches cross-check against file_index because restore
    //    operations (undo_task / undo_file / undo_directory) tombstone
    //    file_index but do NOT delete change records:
    //    - Deleted branch: file_index live row means a restore brought
    //      the file back → existed=true.
    //    - Non-Deleted branch: file_index tombstone means a restore
    //      removed the file since this change was recorded → existed=false.
    if let Ok(Some(prev)) = db.get_latest_change_for_file_excluding_task(file_path, task_id) {
        return match prev.change_type {
            ChangeType::Deleted => resolve_live_file_index_baseline_with_store(
                db,
                &path_str,
                store.as_ref(),
            )
            .unwrap_or(Baseline {
                    existed: false,
                    hash: None,
                }),
            _ => {
                if let Ok(Some(entry)) = db.get_file_index_entry(&path_str) {
                    if !entry.exists_now {
                        return Baseline { existed: false, hash: None };
                    }
                }
                Baseline {
                    existed: true,
                    hash: prev.new_hash.or(prev.old_hash),
                }
            }
        };
    }

    // 4. file_index (startup scan) → file was known at last scan.
    //    content_hash may be None (scanner only stores fast_hash for speed).
    //    In that case, compute SHA-256 from the object store backup and persist it.
    if let Some(baseline) = resolve_live_file_index_baseline_with_store(db, &path_str, store.as_ref()) {
        return baseline;
    }

    // 5. No record at all → file never seen
    Baseline {
        existed: false,
        hash: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::sha256_file;
    use tempfile::TempDir;

    fn temp_db_and_store() -> (TempDir, Database, ObjectStore) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();
        (dir, db, store)
    }

    #[test]
    fn stale_content_hash_repairs_from_fast_object_and_materializes_sha_object() {
        let (_dir, db, store) = temp_db_and_store();
        let source = TempDir::new().unwrap();
        let file_path = source.path().join("stale.txt");
        std::fs::write(&file_path, "before content\nline two\n").unwrap();

        let fast_hash = store.store_fast(&file_path).unwrap();
        let actual_sha = sha256_file(&file_path).unwrap();
        let path_str = "/tmp/stale.txt";
        db.upsert_live_file_index_entry(
            path_str,
            1,
            &fast_hash,
            Some("sha_missing_old_object"),
            "test",
            "seed",
            &chrono::Utc::now().to_rfc3339(),
            Some(1),
        )
        .unwrap();

        let baseline = resolve_live_file_index_baseline_with_store(&db, path_str, Some(&store))
            .expect("baseline should resolve from fast object");
        assert!(baseline.existed);
        assert_eq!(baseline.hash.as_deref(), Some(actual_sha.as_str()));
        assert!(store.exists(&actual_sha), "resolved SHA should be materialized in object store");
        assert_eq!(
            db.get_file_index_hash(path_str).unwrap().as_deref(),
            Some(actual_sha.as_str())
        );
    }

    #[test]
    fn existing_content_hash_object_is_trusted_without_repair() {
        let (_dir, db, store) = temp_db_and_store();
        let source = TempDir::new().unwrap();
        let file_path = source.path().join("valid.txt");
        std::fs::write(&file_path, "stable content\n").unwrap();

        let sha = store.store(&file_path).unwrap();
        let path_str = "/tmp/valid.txt";
        db.upsert_live_file_index_entry(
            path_str,
            1,
            "fast-placeholder",
            Some(&sha),
            "test",
            "seed",
            &chrono::Utc::now().to_rfc3339(),
            Some(1),
        )
        .unwrap();

        let baseline = resolve_live_file_index_baseline_with_store(&db, path_str, Some(&store))
            .expect("baseline should resolve from existing content hash");
        assert!(baseline.existed);
        assert_eq!(baseline.hash.as_deref(), Some(sha.as_str()));
    }
}
