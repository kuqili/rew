//! Baseline resolution for file change tracking.
//!
//! Determines whether a file existed before a task started and provides
//! the best available content hash for that prior state. Used by both
//! the CLI hook and the daemon to ensure consistent `change_type` and
//! `old_hash` determination.

use crate::db::Database;
use crate::objects::{sha256_file, ObjectStore};
use crate::types::ChangeType;
use std::path::Path;

/// Result of resolving a file's baseline state before the current task.
pub struct Baseline {
    /// Whether the file existed before this task started.
    pub existed: bool,
    /// Content hash (SHA-256) of the file's prior state.
    pub hash: Option<String>,
}

/// Resolve the baseline state for a file in the context of a task.
///
/// Uses a 5-level fallback chain (all DB-backed, no file I/O):
///
/// 1. Existing change record in this task → checks change_type:
///    - Created → file didn't exist before task → existed=false
///    - Modified/Renamed/Deleted → preserves original old_hash
/// 2. pre_tool_hashes table → captured right before AI edit (hook only)
/// 3. Latest change from a PREVIOUS task → checks change_type to detect deletions
/// 4. file_index table → startup scan baseline (lazy SHA-256 upgrade from fast hash)
/// 5. No record → file never seen by rew
pub fn resolve_baseline(
    db: &Database,
    task_id: &str,
    file_path: &Path,
    session_key: Option<&str>,
) -> Baseline {
    let path_str = file_path.to_string_lossy().to_string();

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
        return Baseline {
            existed: true,
            hash: old_hash,
        };
    }

    // 2. pre_tool backup captured a hash → file existed before edit (hook path only)
    if let Some(sk) = session_key {
        if let Ok(Some(h)) = db.get_pre_tool_hash(sk, &path_str) {
            return Baseline {
                existed: true,
                hash: Some(h),
            };
        }
    }

    // 3. Latest change from a PREVIOUS task (excludes current task_id to avoid
    //    circular reference — reading our own Created record as "baseline")
    if let Ok(Some(prev)) = db.get_latest_change_for_file_excluding_task(file_path, task_id) {
        return match prev.change_type {
            ChangeType::Deleted => Baseline {
                existed: false,
                hash: None,
            },
            _ => Baseline {
                existed: true,
                hash: prev.new_hash.or(prev.old_hash),
            },
        };
    }

    // 4. file_index (startup scan) → file was known at last scan.
    //    content_hash may be None (scanner only stores fast_hash for speed).
    //    In that case, compute SHA-256 from the object store backup and persist it.
    if let Ok(Some(content_hash)) = db.get_file_index_hash(&path_str) {
        return Baseline {
            existed: true,
            hash: Some(content_hash),
        };
    }
    // content_hash is None — try to upgrade from fast_hash via object store
    if let Ok(Some((_, fast_hash, _))) = db.get_file_index(&path_str) {
        let rew_dir = crate::rew_home_dir();
        if let Ok(store) = ObjectStore::new(rew_dir.join("objects")) {
            if let Some(backup_path) = store.retrieve(&fast_hash) {
                if let Ok(sha) = sha256_file(&backup_path) {
                    let _ = db.update_file_index_content_hash(&path_str, &sha);
                    return Baseline {
                        existed: true,
                        hash: Some(sha),
                    };
                }
            }
        }
        // Object store backup missing — file existed but we can't get content hash
        return Baseline {
            existed: true,
            hash: None,
        };
    }

    // 5. No record at all → file never seen
    Baseline {
        existed: false,
        hash: None,
    }
}
