use crate::db::Database;
use crate::error::RewResult;
use crate::objects::sha256_file;
use crate::types::{Change, ChangeType};
use std::path::Path;

fn file_mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn sync_live_file_index_entry(
    db: &Database,
    file_path: &Path,
    hash: &str,
    source: &str,
    event_kind: &str,
    seen_at: &str,
) -> RewResult<()> {
    db.upsert_live_file_index_entry(
        &file_path.to_string_lossy(),
        file_mtime_secs(file_path),
        hash,
        Some(hash),
        source,
        event_kind,
        seen_at,
        None,
    )?;
    Ok(())
}

pub fn sync_file_index_after_rename(
    db: &Database,
    old_path: &Path,
    new_path: &Path,
    content_hash: Option<&str>,
    source: &str,
    seen_at: &str,
) -> RewResult<()> {
    if !new_path.exists() {
        return Ok(());
    }
    let effective_hash = match content_hash {
        Some(hash) => Some(hash.to_string()),
        None => sha256_file(new_path).ok(),
    };
    let Some(hash) = effective_hash.as_deref() else {
        return Ok(());
    };
    db.mark_file_index_renamed(
        &old_path.to_string_lossy(),
        &new_path.to_string_lossy(),
        file_mtime_secs(new_path),
        hash,
        Some(hash),
        source,
        seen_at,
    )?;
    Ok(())
}

pub fn sync_file_index_after_reconcile(
    db: &Database,
    file_path: &Path,
    current_hash: Option<&str>,
    restore_hash: Option<&str>,
) -> RewResult<()> {
    if file_path.exists() {
        if let Some(hash) = current_hash {
            sync_live_file_index_entry(
                db,
                file_path,
                hash,
                "reconcile",
                "reconcile_live",
                &chrono::Utc::now().to_rfc3339(),
            )?;
        }
        return Ok(());
    }

    db.mark_file_index_deleted(
        &file_path.to_string_lossy(),
        restore_hash,
        "reconcile",
        "reconcile_deleted",
        &chrono::Utc::now().to_rfc3339(),
    )?;
    Ok(())
}

/// Synchronize `file_index` with the latest observed change record.
///
/// This helper is shared by daemon and hook so both paths always apply the
/// exact same live/tombstone semantics.
pub fn sync_file_index_after_change(
    db: &Database,
    change: &Change,
    source: &str,
    seen_at: &str,
) -> RewResult<()> {
    match change.change_type {
        ChangeType::Renamed => {
            if !change.file_path.exists() {
                return Ok(());
            }
            let Some(hash) = change.new_hash.as_deref().or(change.old_hash.as_deref()) else {
                return Ok(());
            };
            if let Some(old_path) = change.old_file_path.as_ref() {
                sync_file_index_after_rename(
                    db,
                    old_path,
                    &change.file_path,
                    change.new_hash.as_deref().or(change.old_hash.as_deref()),
                    source,
                    seen_at,
                )?;
            } else {
                sync_live_file_index_entry(db, &change.file_path, hash, source, "renamed", seen_at)?;
            }
        }
        ChangeType::Created | ChangeType::Modified => {
            if !change.file_path.exists() {
                return Ok(());
            }
            let Some(hash) = change.new_hash.as_deref().or(change.old_hash.as_deref()) else {
                return Ok(());
            };
            let event_kind = match change.change_type {
                ChangeType::Created => "created",
                ChangeType::Modified => "modified",
                ChangeType::Renamed | ChangeType::Deleted => unreachable!(),
            };
            sync_live_file_index_entry(db, &change.file_path, hash, source, event_kind, seen_at)?;
        }
        ChangeType::Deleted => {
            if change.file_path.exists() {
                if let Ok(current_hash) = sha256_file(&change.file_path) {
                    sync_live_file_index_entry(
                        db,
                        &change.file_path,
                        &current_hash,
                        source,
                        "delete_ignored_live",
                        seen_at,
                    )?;
                }
            } else {
                db.mark_file_index_deleted(
                    &change.file_path.to_string_lossy(),
                    change.old_hash.as_deref().or(change.new_hash.as_deref()),
                    source,
                    "deleted",
                    seen_at,
                )?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::types::Change;
    use chrono::Utc;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let rew_home = tempfile::TempDir::new().unwrap();
        let db = Database::open(&rew_home.path().join("snapshots.db")).unwrap();
        db.initialize().unwrap();
        (rew_home, db)
    }

    #[test]
    fn renamed_change_tombstones_old_path_and_rehydrates_new_path() {
        let (dir, db) = temp_db();
        let old_path = dir.path().join("old.txt");
        let new_path = dir.path().join("new.txt");
        std::fs::write(&new_path, "renamed content").unwrap();
        let hash = sha256_file(&new_path).unwrap();

        db.upsert_live_file_index_entry(
            &old_path.to_string_lossy(),
            1,
            &hash,
            Some(&hash),
            "test",
            "seed",
            &Utc::now().to_rfc3339(),
            None,
        )
        .unwrap();

        let change = Change {
            id: None,
            task_id: "t1".to_string(),
            file_path: new_path.clone(),
            change_type: ChangeType::Renamed,
            old_hash: Some(hash.clone()),
            new_hash: Some(hash.clone()),
            diff_text: None,
            lines_added: 0,
            lines_removed: 0,
            attribution: Some("fsevent_active".into()),
            old_file_path: Some(old_path.clone()),
        };

        sync_file_index_after_change(&db, &change, "daemon", &Utc::now().to_rfc3339()).unwrap();

        let old_entry = db
            .get_file_index_entry(&old_path.to_string_lossy())
            .unwrap()
            .unwrap();
        let new_entry = db
            .get_file_index_entry(&new_path.to_string_lossy())
            .unwrap()
            .unwrap();
        assert!(!old_entry.exists_now);
        assert_eq!(old_entry.last_event_kind.as_deref(), Some("rename_old"));
        assert!(new_entry.exists_now);
        assert_eq!(new_entry.last_event_kind.as_deref(), Some("rename_new"));
    }

    #[test]
    fn stale_deleted_event_does_not_tombstone_existing_live_file() {
        let (dir, db) = temp_db();
        let file_path = dir.path().join("live.txt");
        std::fs::write(&file_path, "still here").unwrap();

        let change = Change {
            id: None,
            task_id: "t1".to_string(),
            file_path: file_path.clone(),
            change_type: ChangeType::Deleted,
            old_hash: Some("old-hash".into()),
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 1,
            attribution: Some("fsevent_active".into()),
            old_file_path: None,
        };

        sync_file_index_after_change(&db, &change, "daemon", &Utc::now().to_rfc3339()).unwrap();

        let entry = db
            .get_file_index_entry(&file_path.to_string_lossy())
            .unwrap()
            .unwrap();
        assert!(entry.exists_now);
        assert_eq!(entry.last_event_kind.as_deref(), Some("delete_ignored_live"));
        assert!(entry.deleted_at.is_none());
    }
}
