//! Comprehensive test suite for the core change tracking pipeline:
//! resolve_baseline -> daemon/hook writes -> reconcile_task.
//!
//! Run with: cargo test -p rew-core --test change_tracking

use std::path::{Path, PathBuf};
use tempfile::TempDir;

use rew_core::baseline::{resolve_baseline, Baseline};
use rew_core::db::Database;
use rew_core::objects::ObjectStore;
use rew_core::reconcile::{reconcile_task, ReconcileResult};
use rew_core::types::{Change, ChangeType, Task, TaskStatus};

// ============================================================
// Test Environment
// ============================================================

struct TestEnv {
    dir: TempDir,
    db: Database,
    objects_root: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        db.initialize().unwrap();
        let objects_root = dir.path().join("objects");
        std::fs::create_dir_all(&objects_root).unwrap();
        Self { dir, db, objects_root }
    }

    fn write_file(&self, name: &str, content: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    fn delete_file(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    fn store_file(&self, path: &Path) -> String {
        let store = ObjectStore::new(self.objects_root.clone()).unwrap();
        store.store(path).unwrap()
    }

    fn create_active_task(&self, id: &str) {
        let task = Task {
            id: id.to_string(),
            prompt: Some("test".into()),
            tool: Some("test-tool".into()),
            started_at: chrono::Utc::now(),
            completed_at: None,
            status: TaskStatus::Active,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        self.db.create_task(&task).unwrap();
    }

    fn create_completed_task(&self, id: &str) {
        let task = Task {
            id: id.to_string(),
            prompt: Some("test".into()),
            tool: Some("test-tool".into()),
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        self.db.create_task(&task).unwrap();
    }

    fn complete_task(&self, id: &str) {
        self.db
            .update_task_status(id, &TaskStatus::Completed, Some(chrono::Utc::now()))
            .unwrap();
    }

    fn seed_file_index(&self, path: &str, fast_hash: &str, content_hash: Option<&str>) {
        self.db
            .upsert_live_file_index_entry(
                path,
                1000,
                fast_hash,
                content_hash,
                "test",
                "seed",
                &chrono::Utc::now().to_rfc3339(),
                Some(1),
            )
            .unwrap();
    }

    fn insert_change(&self, change: &Change) -> i64 {
        self.db.insert_change(change).unwrap()
    }

    fn upsert_change(&self, change: &Change) -> i64 {
        self.db.upsert_change(change).unwrap()
    }

    fn reconcile(&self, task_id: &str) -> ReconcileResult {
        reconcile_task(&self.db, task_id, &self.objects_root).unwrap()
    }

    fn baseline(&self, task_id: &str, path: &Path) -> Baseline {
        resolve_baseline(&self.db, task_id, path, None)
    }

    fn baseline_with_session(&self, task_id: &str, path: &Path, session_key: &str) -> Baseline {
        resolve_baseline(&self.db, task_id, path, Some(session_key))
    }

    fn changes(&self, task_id: &str) -> Vec<Change> {
        self.db.get_changes_for_task(task_id).unwrap()
    }

    fn make_change(
        &self,
        task_id: &str,
        file_path: &Path,
        change_type: ChangeType,
        old_hash: Option<&str>,
        new_hash: Option<&str>,
        attribution: &str,
    ) -> Change {
        Change {
            id: None,
            task_id: task_id.to_string(),
            file_path: file_path.to_path_buf(),
            change_type,
            old_hash: old_hash.map(|s| s.to_string()),
            new_hash: new_hash.map(|s| s.to_string()),
            diff_text: None,
            lines_added: 0,
            lines_removed: 0,
            attribution: Some(attribution.to_string()),
            old_file_path: None,
        }
    }
}

// ============================================================
// A. resolve_baseline scenarios
// ============================================================

#[test]
fn a1_baseline_brand_new_file() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.dir.path().join("never_seen.txt");

    let bl = env.baseline("t1", &path);
    assert!(!bl.existed);
    assert!(bl.hash.is_none());
}

#[test]
fn a2_baseline_file_index_with_content_hash() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.dir.path().join("indexed.txt");
    let path_str = path.to_string_lossy().to_string();

    env.seed_file_index(&path_str, "fast-111", Some("sha256_aaa"));

    let bl = env.baseline("t1", &path);
    assert!(bl.existed);
    assert_eq!(bl.hash.as_deref(), Some("sha256_aaa"));
}

#[test]
fn a3_baseline_file_index_fast_hash_no_backup() {
    // resolve_baseline Layer 4 fast_hash upgrade uses rew_home_dir() internally,
    // which points to ~/.rew/objects/ — not the test directory. So in unit tests,
    // when file_index has only fast_hash (content_hash=None), the ObjectStore
    // lookup will miss, yielding existed=true but hash=None.
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.dir.path().join("fast_only.txt");
    let path_str = path.to_string_lossy().to_string();
    env.seed_file_index(&path_str, "fast-999-888-100", None);

    let bl = env.baseline("t1", &path);
    assert!(bl.existed, "file_index entry means file was seen at scan time");
    assert!(bl.hash.is_none(), "No content_hash and no backup → hash is None");
}

#[test]
fn a4_baseline_same_task_created_record() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("new.txt", "new");
    let sha = env.store_file(&path);

    let change = env.make_change("t1", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.insert_change(&change);

    let bl = env.baseline("t1", &path);
    assert!(!bl.existed, "Created record means file didn't exist before task");
    assert!(bl.hash.is_none());
}

#[test]
fn a5_baseline_same_task_modified_record() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("mod.txt", "modified");
    let sha_new = env.store_file(&path);

    let change = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_old_abc"), Some(&sha_new), "hook",
    );
    env.insert_change(&change);

    let bl = env.baseline("t1", &path);
    assert!(bl.existed);
    assert_eq!(bl.hash.as_deref(), Some("sha_old_abc"));
}

#[test]
fn a6_baseline_previous_task_deleted() {
    let env = TestEnv::new();
    env.create_completed_task("t_prev");
    env.create_active_task("t_cur");

    let path = env.dir.path().join("was_deleted.txt");
    let change = env.make_change(
        "t_prev", &path, ChangeType::Deleted,
        Some("sha_before_delete"), None, "hook",
    );
    env.insert_change(&change);

    let bl = env.baseline("t_cur", &path);
    assert!(!bl.existed, "Previous task deleted this file");
    assert!(bl.hash.is_none());
}

#[test]
fn a6b_baseline_previous_task_deleted_but_live_file_index_wins() {
    let env = TestEnv::new();
    env.create_completed_task("t_prev");
    env.create_active_task("t_cur");

    let path = env.write_file("restored_again.txt", "restored content\n");
    let sha_live = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    let deleted = env.make_change(
        "t_prev", &path, ChangeType::Deleted,
        Some("sha_before_delete"), None, "monitoring",
    );
    env.insert_change(&deleted);
    env.seed_file_index(&path_str, &sha_live, Some(&sha_live));

    let bl = env.baseline("t_cur", &path);
    assert!(
        bl.existed,
        "A live file_index row should override a stale historical Deleted record"
    );
    assert_eq!(bl.hash.as_deref(), Some(sha_live.as_str()));
}

#[test]
fn a7_baseline_previous_task_modified() {
    let env = TestEnv::new();
    env.create_completed_task("t_prev");
    env.create_active_task("t_cur");

    let path = env.dir.path().join("was_modified.txt");
    let change = env.make_change(
        "t_prev", &path, ChangeType::Modified,
        Some("sha_v1"), Some("sha_v2"), "hook",
    );
    env.insert_change(&change);

    let bl = env.baseline("t_cur", &path);
    assert!(bl.existed);
    assert_eq!(bl.hash.as_deref(), Some("sha_v2"), "Should use new_hash from previous task");
}

#[test]
fn a8_baseline_pre_tool_hash() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.dir.path().join("pre_tool.txt");
    let path_str = path.to_string_lossy().to_string();

    env.db
        .set_pre_tool_hash("session-abc", &path_str, "sha_pre_tool")
        .unwrap();

    let bl = env.baseline_with_session("t1", &path, "session-abc");
    assert!(bl.existed);
    assert_eq!(bl.hash.as_deref(), Some("sha_pre_tool"));
}

#[test]
fn a9_baseline_tombstoned_file_index_does_not_mark_path_existing() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.dir.path().join("deleted_from_index.txt");
    let path_str = path.to_string_lossy().to_string();

    env.seed_file_index(&path_str, "fast-deleted", Some("sha_deleted"));
    env.db
        .mark_file_index_deleted(
            &path_str,
            Some("sha_deleted"),
            "test",
            "deleted",
            &chrono::Utc::now().to_rfc3339(),
        )
        .unwrap();

    let bl = env.baseline("t1", &path);
    assert!(!bl.existed);
    assert!(bl.hash.is_none());
}

// ============================================================
// B. Single task, single file change scenarios
// ============================================================

#[test]
fn b1_create_new_file() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("new.txt", "hello world\n");
    let sha = env.store_file(&path);

    let change = env.make_change("t1", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.upsert_change(&change);

    let result = env.reconcile("t1");
    assert_eq!(result.removed, 0);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Created);
    assert!(changes[0].new_hash.is_some());
    assert!(changes[0].lines_added > 0);
}

#[test]
fn b2_modify_existing_file() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("exist.txt", "line1\nline2\n");
    let sha_old = env.store_file(&path);

    std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
    let sha_new = env.store_file(&path);

    let change = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some(&sha_old), Some(&sha_new), "hook",
    );
    env.upsert_change(&change);

    let result = env.reconcile("t1");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Modified);
    assert_eq!(changes[0].old_hash.as_deref(), Some(sha_old.as_str()));
    assert_eq!(changes[0].new_hash.as_deref(), Some(sha_new.as_str()));
    assert!(changes[0].lines_added >= 1);
    let _ = result;
}

#[test]
fn b2b_restored_file_modify_stays_modified() {
    let env = TestEnv::new();
    env.create_completed_task("t_prev");
    env.create_active_task("t_cur");

    let path = env.write_file("restored_then_modified.txt", "line1\nline2\n");
    let sha_old = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    env.insert_change(&env.make_change(
        "t_prev", &path, ChangeType::Deleted,
        Some(&sha_old), None, "monitoring",
    ));
    env.seed_file_index(&path_str, &sha_old, Some(&sha_old));

    std::fs::write(&path, "line1\nchanged\nline2\n").unwrap();
    let sha_new = env.store_file(&path);

    let baseline = env.baseline("t_cur", &path);
    let change_type = if baseline.existed {
        ChangeType::Modified
    } else {
        ChangeType::Created
    };
    env.upsert_change(&env.make_change(
        "t_cur",
        &path,
        change_type,
        baseline.hash.as_deref(),
        Some(&sha_new),
        "fsevent_active",
    ));

    env.reconcile("t_cur");
    let changes = env.changes("t_cur");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Modified);
    assert_eq!(changes[0].old_hash.as_deref(), Some(sha_old.as_str()));
    assert_eq!(changes[0].new_hash.as_deref(), Some(sha_new.as_str()));
}

#[test]
fn b3_delete_existing_file() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("to_delete.txt", "will be deleted\n");
    let sha_old = env.store_file(&path);
    env.delete_file(&path);

    let change = env.make_change(
        "t1", &path, ChangeType::Deleted,
        Some(&sha_old), None, "fsevent_active",
    );
    env.upsert_change(&change);

    let result = env.reconcile("t1");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Deleted);
    assert!(changes[0].new_hash.is_none());
    let _ = result;
}

#[test]
fn b3b_restored_file_delete_stays_deleted() {
    let env = TestEnv::new();
    env.create_completed_task("t_prev");
    env.create_active_task("t_cur");

    let path = env.write_file("restored_then_deleted.txt", "keep me\n");
    let sha_old = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    env.insert_change(&env.make_change(
        "t_prev", &path, ChangeType::Deleted,
        Some(&sha_old), None, "monitoring",
    ));
    env.seed_file_index(&path_str, &sha_old, Some(&sha_old));

    env.delete_file(&path);
    let baseline = env.baseline("t_cur", &path);
    env.upsert_change(&env.make_change(
        "t_cur",
        &path,
        ChangeType::Deleted,
        baseline.hash.as_deref(),
        None,
        "fsevent_active",
    ));

    env.reconcile("t_cur");
    let changes = env.changes("t_cur");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Deleted);
    assert_eq!(changes[0].old_hash.as_deref(), Some(sha_old.as_str()));
    assert!(changes[0].new_hash.is_none());
    let file_index = env
        .db
        .get_file_index_entry(&path_str)
        .unwrap()
        .unwrap();
    assert!(!file_index.exists_now, "Deleted reconcile result must tombstone file_index");
}

#[test]
fn b4_create_then_delete_net_zero() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.dir.path().join("ephemeral.txt");
    // File was created then deleted — on disk it doesn't exist
    // old_hash=None (never existed), new_hash=None (file gone)
    let change = env.make_change("t1", &path, ChangeType::Created, None, None, "fsevent_active");
    env.upsert_change(&change);

    let result = env.reconcile("t1");
    assert_eq!(result.removed, 1, "Ephemeral (None, None) should be removed");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 0);
}

#[test]
fn b4b_ephemeral_delete_tombstones_live_file_index() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("ephemeral_indexed.txt", "temp content\n");
    let sha = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();
    env.seed_file_index(&path_str, &sha, Some(&sha));

    env.delete_file(&path);
    let change = env.make_change("t1", &path, ChangeType::Created, None, None, "fsevent_active");
    env.upsert_change(&change);

    let result = env.reconcile("t1");
    assert_eq!(result.removed, 1);
    let file_index = env
        .db
        .get_file_index_entry(&path_str)
        .unwrap()
        .unwrap();
    assert!(
        !file_index.exists_now,
        "Even ephemeral create-then-delete must leave file_index tombstoned"
    );
}

#[test]
fn b5_modify_then_revert_net_zero() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("revert.txt", "original content\n");
    let sha_orig = env.store_file(&path);

    // Record a modification with old=sha_orig, new=sha_something
    // But the file on disk is actually reverted to original
    let change = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some(&sha_orig), Some("sha_intermediate"), "hook",
    );
    env.upsert_change(&change);

    // reconcile reads the actual file and computes current_hash = sha_orig
    // old_hash == current_hash → net-zero → remove
    let result = env.reconcile("t1");
    assert_eq!(result.removed, 1, "Modified-then-reverted should be net-zero");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 0);
}

#[test]
fn b6_delete_then_recreate() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("recreated.txt", "new version content\n");
    let sha_old = "sha_original_version";
    let _ = env.store_file(&path);

    // Record as if it was deleted and then we upsert with new state
    let change = env.make_change(
        "t1", &path, ChangeType::Deleted,
        Some(sha_old), None, "fsevent_active",
    );
    env.upsert_change(&change);

    // reconcile sees: old_hash=sha_old, file exists on disk → Modified
    let result = env.reconcile("t1");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].change_type,
        ChangeType::Modified,
        "Delete then recreate with different content should become Modified"
    );
    assert_eq!(changes[0].old_hash.as_deref(), Some(sha_old));
    assert!(changes[0].new_hash.is_some());
    let _ = result;
}

#[test]
fn b7_multiple_modifications_same_file() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("multi.txt", "v1\n");
    let sha_v1 = env.store_file(&path);

    // First upsert: old=v1, new=v2
    let c1 = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some(&sha_v1), Some("sha_v2"), "hook",
    );
    env.upsert_change(&c1);

    // Write v3 on disk
    std::fs::write(&path, "v1\nv2\nv3\n").unwrap();
    let sha_v3 = env.store_file(&path);

    // Second upsert: new=v3 (upsert preserves original old_hash)
    let c2 = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some(&sha_v1), Some(&sha_v3), "hook",
    );
    env.upsert_change(&c2);

    // Should be exactly 1 record due to upsert
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].old_hash.as_deref(), Some(sha_v1.as_str()));

    let result = env.reconcile("t1");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Modified);
    assert!(changes[0].lines_added >= 2);
    let _ = result;
}

// ============================================================
// C. Rename pairing scenarios
// ============================================================

#[test]
fn c1_rename_identical_content() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path_a = env.write_file("a.txt", "same content\n");
    let sha = env.store_file(&path_a);

    // Simulate: file A deleted, file B created with identical content
    let path_b = env.write_file("b.txt", "same content\n");
    let sha_b = env.store_file(&path_b);
    assert_eq!(sha, sha_b, "Content-addressable: same content = same hash");

    env.delete_file(&path_a);

    let del = env.make_change("t1", &path_a, ChangeType::Deleted, Some(&sha), None, "fsevent_active");
    let cre = env.make_change("t1", &path_b, ChangeType::Created, None, Some(&sha_b), "fsevent_active");
    env.upsert_change(&del);
    env.upsert_change(&cre);

    let result = env.reconcile("t1");
    assert_eq!(result.renames_paired, 1);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1, "Deleted record should be removed, Created becomes Renamed");
    assert_eq!(changes[0].change_type, ChangeType::Renamed);
    assert_eq!(
        changes[0].old_file_path.as_deref(),
        Some(path_a.as_path()),
        "old_file_path should be the deleted file"
    );
    assert_eq!(changes[0].file_path, path_b);
    let old_entry = env
        .db
        .get_file_index_entry(&path_a.to_string_lossy())
        .unwrap()
        .unwrap();
    let new_entry = env
        .db
        .get_file_index_entry(&path_b.to_string_lossy())
        .unwrap()
        .unwrap();
    assert!(!old_entry.exists_now);
    assert_eq!(old_entry.last_event_kind.as_deref(), Some("rename_old"));
    assert!(new_entry.exists_now);
    assert_eq!(new_entry.last_event_kind.as_deref(), Some("rename_new"));
}

#[test]
fn c2_rename_different_content_no_pair() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path_a = env.write_file("a.txt", "content A\n");
    let sha_a = env.store_file(&path_a);
    let path_b = env.write_file("b.txt", "different content B\n");
    let sha_b = env.store_file(&path_b);

    env.delete_file(&path_a);

    let del = env.make_change("t1", &path_a, ChangeType::Deleted, Some(&sha_a), None, "fsevent_active");
    let cre = env.make_change("t1", &path_b, ChangeType::Created, None, Some(&sha_b), "fsevent_active");
    env.upsert_change(&del);
    env.upsert_change(&cre);

    let result = env.reconcile("t1");
    assert_eq!(result.renames_paired, 0, "Different content should not pair");

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 2);
}

#[test]
fn c3_rename_one_match_one_independent() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path_a = env.write_file("a.txt", "shared content\n");
    let sha_shared = env.store_file(&path_a);
    let path_b = env.write_file("b.txt", "shared content\n");
    let sha_b = env.store_file(&path_b);
    let path_c = env.write_file("c.txt", "unique content\n");
    let sha_c = env.store_file(&path_c);

    env.delete_file(&path_a);

    let del = env.make_change("t1", &path_a, ChangeType::Deleted, Some(&sha_shared), None, "fsevent_active");
    let cre_b = env.make_change("t1", &path_b, ChangeType::Created, None, Some(&sha_b), "fsevent_active");
    let cre_c = env.make_change("t1", &path_c, ChangeType::Created, None, Some(&sha_c), "fsevent_active");
    env.upsert_change(&del);
    env.upsert_change(&cre_b);
    env.upsert_change(&cre_c);

    let result = env.reconcile("t1");
    assert_eq!(result.renames_paired, 1);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 2, "1 Renamed + 1 Created");

    let renamed: Vec<_> = changes.iter().filter(|c| c.change_type == ChangeType::Renamed).collect();
    let created: Vec<_> = changes.iter().filter(|c| c.change_type == ChangeType::Created).collect();
    assert_eq!(renamed.len(), 1);
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].file_path, path_c);
}

#[test]
fn c4_copy_rename_modify_chain() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    // Original file A exists. Copy to B, rename B to C, modify C.
    // After all operations: A still exists, C exists with modified content.
    // From rew's perspective (daemon events): Created(C) with new content.
    // A was never deleted. No rename pairing should happen.
    let path_a = env.write_file("original.txt", "original\n");
    let _sha_a = env.store_file(&path_a);

    let path_c = env.write_file("renamed_modified.txt", "original\nmodified line\n");
    let sha_c = env.store_file(&path_c);

    let cre = env.make_change("t1", &path_c, ChangeType::Created, None, Some(&sha_c), "fsevent_active");
    env.upsert_change(&cre);

    let result = env.reconcile("t1");
    assert_eq!(result.renames_paired, 0);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Created);
}

// ============================================================
// D. Hook & Daemon interaction (upsert_change attribution priority)
// ============================================================

#[test]
fn d1_hook_then_daemon_hook_preserved() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("file.txt", "content\n");

    // Hook writes first
    let hook_change = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_old_precise"), Some("sha_new_1"), "hook",
    );
    env.upsert_change(&hook_change);

    // Daemon writes same file (fsevent_active)
    let daemon_change = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_old_daemon"), Some("sha_new_2"), "fsevent_active",
    );
    env.upsert_change(&daemon_change);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    // Hook record should NOT be overwritten by daemon
    assert_eq!(changes[0].attribution.as_deref(), Some("hook"));
    assert_eq!(
        changes[0].old_hash.as_deref(),
        Some("sha_old_precise"),
        "hook's old_hash must be preserved"
    );
}

#[test]
fn d2_daemon_then_hook_hook_wins() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("file.txt", "content\n");

    // Daemon writes first
    let daemon_change = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_old_daemon"), Some("sha_new_1"), "fsevent_active",
    );
    env.upsert_change(&daemon_change);

    // Hook writes later
    let hook_change = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_old_hook"), Some("sha_new_2"), "hook",
    );
    env.upsert_change(&hook_change);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].attribution.as_deref(), Some("hook"));
    // When hook writes, old_hash uses hook's value preferentially
    assert_eq!(
        changes[0].old_hash.as_deref(),
        Some("sha_old_hook"),
        "hook's old_hash should take priority"
    );
}

#[test]
fn d3_monitoring_then_hook_overwrites() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("file.txt", "content\n");

    let mon = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_mon"), Some("sha_new_mon"), "monitoring",
    );
    env.upsert_change(&mon);

    let hook = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_hook"), Some("sha_new_hook"), "hook",
    );
    env.upsert_change(&hook);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].attribution.as_deref(), Some("hook"));
    assert_eq!(changes[0].new_hash.as_deref(), Some("sha_new_hook"));
}

#[test]
fn d4_hook_created_then_daemon_dedup() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("created.txt", "content\n");
    let sha = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    // Register active session so is_change_already_recorded can find it
    env.db
        .insert_active_session("sess-1", "t1", "test", chrono::Utc::now())
        .unwrap();

    let hook = env.make_change("t1", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.upsert_change(&hook);

    // Daemon checks dedup
    let is_recorded = env.db.is_change_already_recorded(&path_str, &sha).unwrap();
    assert!(is_recorded, "Daemon should detect the hook's record and skip");
}

#[test]
fn d5_hook_modified_daemon_different_hash() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("evolving.txt", "v1\n");

    let hook = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_old"), Some("sha_v1"), "hook",
    );
    env.upsert_change(&hook);

    // File changed again (e.g. bash tool), daemon detects new hash
    // Since existing is "hook", daemon write should be blocked
    let daemon = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_old"), Some("sha_v2"), "fsevent_active",
    );
    env.upsert_change(&daemon);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    // Hook record is preserved (daemon can't overwrite hook)
    assert_eq!(changes[0].attribution.as_deref(), Some("hook"));
    assert_eq!(
        changes[0].new_hash.as_deref(),
        Some("sha_v1"),
        "hook attribution blocks daemon update"
    );
}

// ============================================================
// E. Multi-task scenarios
// ============================================================

#[test]
fn e1_cross_task_baseline_modified() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("file.txt", "v1\n");

    let c = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_v0"), Some("sha_v1"), "hook",
    );
    env.upsert_change(&c);
    env.complete_task("t1");

    env.create_active_task("t2");
    let bl = env.baseline("t2", &path);
    assert!(bl.existed);
    assert_eq!(bl.hash.as_deref(), Some("sha_v1"), "Layer 3 should return t1's new_hash");
}

#[test]
fn e2_cross_task_baseline_created() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("newfile.txt", "created\n");
    let sha = env.store_file(&path);

    let c = env.make_change("t1", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.upsert_change(&c);
    env.complete_task("t1");

    env.create_active_task("t2");
    let bl = env.baseline("t2", &path);
    assert!(bl.existed, "File was created by t1, so it exists for t2");
    assert_eq!(bl.hash.as_deref(), Some(sha.as_str()));
}

#[test]
fn e3_cross_task_baseline_deleted() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.dir.path().join("was_here.txt");

    let c = env.make_change(
        "t1", &path, ChangeType::Deleted,
        Some("sha_before"), None, "hook",
    );
    env.upsert_change(&c);
    env.complete_task("t1");

    env.create_active_task("t2");
    let bl = env.baseline("t2", &path);
    assert!(!bl.existed, "Layer 3 detects previous Deleted");
    assert!(bl.hash.is_none());
}

#[test]
fn e4_concurrent_tasks_independent() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    env.create_active_task("t2");

    let path = env.write_file("shared.txt", "base\n");
    let sha_base = env.store_file(&path);

    // t1 sees baseline
    let bl1 = env.baseline("t1", &path);
    // t2 sees baseline
    let bl2 = env.baseline("t2", &path);

    // Both should see the same initial state (no changes recorded yet)
    assert_eq!(bl1.existed, bl2.existed);

    // t1 records a change
    let c1 = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some(&sha_base), Some("sha_t1"), "hook",
    );
    env.upsert_change(&c1);

    // t2's baseline should NOT see t1's change (Layer 3 excludes current task,
    // and t1's record is not in t2's task)
    let bl2_after = env.baseline("t2", &path);
    // t2 has no record for this file, Layer 3 will find t1's record
    // but t1 is a different task and not excluded, so it WILL see t1's change
    // This is expected: in real usage, t1 was created before t2 so its changes
    // represent the latest known state
    assert!(bl2_after.existed);

    // Both tasks can have independent change records
    let c2 = env.make_change(
        "t2", &path, ChangeType::Modified,
        bl2_after.hash.as_deref(), Some("sha_t2"), "hook",
    );
    env.upsert_change(&c2);

    let changes_t1 = env.changes("t1");
    let changes_t2 = env.changes("t2");
    assert_eq!(changes_t1.len(), 1);
    assert_eq!(changes_t2.len(), 1);
    assert_ne!(changes_t1[0].new_hash, changes_t2[0].new_hash);
}

#[test]
fn e5_grace_period_attribution() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    env.complete_task("t1");

    let path = env.write_file("delayed.txt", "content\n");
    let sha = env.store_file(&path);

    // Simulate daemon recording with grace attribution
    let grace = env.make_change(
        "t1", &path, ChangeType::Modified,
        None, Some(&sha), "fsevent_grace",
    );
    env.upsert_change(&grace);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].attribution.as_deref(), Some("fsevent_grace"));
    assert_eq!(changes[0].task_id, "t1");
}

// ============================================================
// F. is_change_already_recorded dedup scenarios
// ============================================================

#[test]
fn f1_dedup_active_session_match() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    env.db
        .insert_active_session("sess-1", "t1", "test", chrono::Utc::now())
        .unwrap();

    let path = env.write_file("f1.txt", "data\n");
    let sha = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    let c = env.make_change("t1", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.upsert_change(&c);

    assert!(env.db.is_change_already_recorded(&path_str, &sha).unwrap());
}

#[test]
fn f2_dedup_recently_completed() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("f2.txt", "data\n");
    let sha = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    let c = env.make_change("t1", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.upsert_change(&c);

    // Complete task just now (within 90s)
    env.complete_task("t1");

    assert!(
        env.db.is_change_already_recorded(&path_str, &sha).unwrap(),
        "Recently completed task should match"
    );
}

#[test]
fn f3_dedup_completed_older_than_90s() {
    let env = TestEnv::new();

    let old_time = chrono::Utc::now() - chrono::Duration::seconds(200);
    let task = Task {
        id: "t_old".to_string(),
        prompt: Some("old".into()),
        tool: Some("test".into()),
        started_at: old_time - chrono::Duration::seconds(10),
        completed_at: Some(old_time),
        status: TaskStatus::Completed,
        risk_level: None,
        summary: None,
        cwd: None,
    };
    env.db.create_task(&task).unwrap();

    let path = env.write_file("f3.txt", "data\n");
    let sha = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    let c = env.make_change("t_old", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.upsert_change(&c);

    assert!(
        !env.db.is_change_already_recorded(&path_str, &sha).unwrap(),
        "Task completed >90s ago should NOT match"
    );
}

#[test]
fn f4_dedup_same_path_different_hash() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    env.db
        .insert_active_session("sess-1", "t1", "test", chrono::Utc::now())
        .unwrap();

    let path = env.write_file("f4.txt", "data v1\n");
    let sha_v1 = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    let c = env.make_change("t1", &path, ChangeType::Created, None, Some(&sha_v1), "hook");
    env.upsert_change(&c);

    // Different hash for same path → should NOT match
    assert!(
        !env.db
            .is_change_already_recorded(&path_str, "sha_completely_different")
            .unwrap(),
        "Different hash should not match"
    );
}

#[test]
fn f5_dedup_completed_within_90s_boundary() {
    let env = TestEnv::new();

    let recent_time = chrono::Utc::now() - chrono::Duration::seconds(89);
    let task = Task {
        id: "t_recent".to_string(),
        prompt: Some("recent".into()),
        tool: Some("test".into()),
        started_at: recent_time - chrono::Duration::seconds(10),
        completed_at: Some(recent_time),
        status: TaskStatus::Completed,
        risk_level: None,
        summary: None,
        cwd: None,
    };
    env.db.create_task(&task).unwrap();

    let path = env.write_file("f5.txt", "data\n");
    let sha = env.store_file(&path);
    let path_str = path.to_string_lossy().to_string();

    let c = env.make_change("t_recent", &path, ChangeType::Created, None, Some(&sha), "hook");
    env.upsert_change(&c);

    assert!(
        env.db.is_change_already_recorded(&path_str, &sha).unwrap(),
        "Task completed within 90s should match"
    );
}

// ============================================================
// G. Edge cases and boundary scenarios
// ============================================================

#[test]
fn g1_reconcile_empty_task() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let result = env.reconcile("t1");
    assert_eq!(result.removed, 0);
    assert_eq!(result.updated, 0);
    assert_eq!(result.renames_paired, 0);
}

#[test]
fn g2_file_deleted_during_reconcile() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("vanish.txt", "will vanish\n");
    let sha_old = env.store_file(&path);
    let sha_new = "sha_intermediate";

    let c = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some(&sha_old), Some(sha_new), "hook",
    );
    env.upsert_change(&c);

    // Delete the file before reconcile
    env.delete_file(&path);

    let result = env.reconcile("t1");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].change_type,
        ChangeType::Deleted,
        "File gone during reconcile should become Deleted"
    );
    assert!(changes[0].new_hash.is_none());
    let _ = result;
}

#[test]
fn g3_object_store_missing_hash() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path = env.write_file("exists.txt", "content\n");
    let sha_new = env.store_file(&path);

    // Record with old_hash pointing to a hash NOT in object store
    let c = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("nonexistent_hash_abc123"), Some(&sha_new), "hook",
    );
    env.upsert_change(&c);

    // reconcile should not panic even though old_hash can't be retrieved
    let result = env.reconcile("t1");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    // Lines may be recalculated with old_bytes=[] but should not crash
    let _ = result;
}

#[test]
fn g4_triple_upsert_same_file() {
    let env = TestEnv::new();
    env.create_active_task("t1");
    let path = env.write_file("triple.txt", "content\n");

    let c1 = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_original"), Some("sha_v1"), "fsevent_active",
    );
    env.upsert_change(&c1);

    let c2 = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_original"), Some("sha_v2"), "fsevent_active",
    );
    env.upsert_change(&c2);

    let c3 = env.make_change(
        "t1", &path, ChangeType::Modified,
        Some("sha_original"), Some("sha_v3"), "fsevent_active",
    );
    env.upsert_change(&c3);

    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1, "Upsert should keep exactly 1 record");
    assert_eq!(
        changes[0].old_hash.as_deref(),
        Some("sha_original"),
        "Original old_hash must be preserved through all upserts"
    );
    assert_eq!(
        changes[0].new_hash.as_deref(),
        Some("sha_v3"),
        "new_hash should reflect latest upsert"
    );
}

#[test]
fn g5_unicode_and_spaces_in_path() {
    let env = TestEnv::new();
    env.create_active_task("t1");

    let path_cn = env.write_file("中文目录/文件 名.txt", "中文内容\n");
    let sha_cn = env.store_file(&path_cn);

    let c = env.make_change(
        "t1", &path_cn, ChangeType::Created,
        None, Some(&sha_cn), "hook",
    );
    env.upsert_change(&c);

    let bl = env.baseline("t1", &path_cn);
    // Layer 1 should find the Created record
    assert!(!bl.existed);

    let result = env.reconcile("t1");
    let changes = env.changes("t1");
    assert_eq!(changes.len(), 1);
    assert!(changes[0].file_path.to_string_lossy().contains("中文"));
    let _ = result;
}
