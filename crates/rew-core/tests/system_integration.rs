//! System-level integration tests: end-to-end from input to DB output.
//!
//! These tests exercise the **production code path** through the full pipeline:
//!   Hook envelope → process_hook_event → DB
//!   File operation → resolve_baseline → upsert_change → DB
//!   reconcile_task → final DB state
//!
//! All state is isolated in TempDir — zero side effects on the host system.
//!
//! Run with: cargo test -p rew-core --test system_integration

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::Utc;
use tempfile::TempDir;

use rew_core::baseline::resolve_baseline_with_objects_root;
use rew_core::db::Database;
use rew_core::file_index::sync_file_index_after_change;
use rew_core::hook_events::{
    process_hook_event_with_objects_root,
    HookEventEnvelope, HookEventPayload,
    ObservedPathChange, PostToolObservedPayload, PromptStartedPayload,
    TaskStopRequestedPayload,
};
use rew_core::objects::{sha256_file, ObjectStore};
use rew_core::reconcile::reconcile_task;
use rew_core::restore::TaskRestoreEngine;
use rew_core::types::{Change, ChangeType, TaskStatus};

use std::collections::BTreeMap;

// ============================================================
// Test Environment (IntegrationEnv)
// ============================================================

struct IntegrationEnv {
    _dir: TempDir,
    db: Database,
    objects_root: PathBuf,
    work_dir: PathBuf,
    event_counter: AtomicU32,
}

impl IntegrationEnv {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        let objects_root = dir.path().join("objects");
        let work_dir = dir.path().join("project");
        std::fs::create_dir_all(&objects_root).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        Self {
            _dir: dir,
            db,
            objects_root,
            work_dir,
            event_counter: AtomicU32::new(0),
        }
    }

    fn next_id(&self) -> u32 {
        self.event_counter.fetch_add(1, Ordering::Relaxed)
    }

    // ---- File helpers ----

    fn file_path(&self, relative: &str) -> PathBuf {
        let p = self.work_dir.join(relative);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        p
    }

    fn write_file(&self, relative: &str, content: &str) -> PathBuf {
        let p = self.file_path(relative);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn delete_file(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    fn store_file(&self, path: &Path) -> String {
        ObjectStore::new(self.objects_root.clone())
            .unwrap()
            .store(path)
            .unwrap()
    }

    #[allow(dead_code)]
    fn store_content(&self, name: &str, content: &str) -> String {
        let tmp = self.work_dir.join(format!(".tmp_{}", name));
        std::fs::write(&tmp, content).unwrap();
        let hash = self.store_file(&tmp);
        let _ = std::fs::remove_file(&tmp);
        hash
    }

    // ---- Hook pipeline (calls production process_hook_event) ----

    fn hook_prompt(&self, session: &str, tool: &str) -> String {
        self.hook_prompt_with_prompt_text(session, tool, "test prompt")
    }

    fn hook_prompt_with_prompt_text(&self, session: &str, tool: &str, prompt: &str) -> String {
        let n = self.next_id();
        let envelope = HookEventEnvelope {
            event_id: format!("prompt_{n}"),
            idempotency_key: format!("prompt:{session}:{n}"),
            created_at: Utc::now().to_rfc3339(),
            payload_version: 1,
            payload: HookEventPayload::PromptStarted(PromptStartedPayload {
                tool_source: tool.to_string(),
                session_key: session.to_string(),
                prompt: Some(prompt.to_string()),
                cwd: Some(self.work_dir.to_string_lossy().to_string()),
                model: Some("test-model".to_string()),
                conversation_id: None,
                generation_id: None,
            }),
        };
        let outcome = process_hook_event_with_objects_root(
            &self.db, &envelope, &self.objects_root,
        ).unwrap();
        let task_id = outcome.task_id.expect("prompt should create task");
        self.age_task(&task_id, 5);
        task_id
    }

    /// Record a post-tool observation for a native file tool (attribution=hook).
    fn hook_post_tool_native(
        &self,
        session: &str,
        tool_name: &str,
        observations: Vec<ObservedPathChange>,
    ) {
        let n = self.next_id();
        let envelope = HookEventEnvelope {
            event_id: format!("post_{n}"),
            idempotency_key: format!("post:{session}:{n}"),
            created_at: Utc::now().to_rfc3339(),
            payload_version: 1,
            payload: HookEventPayload::PostToolObserved(PostToolObservedPayload {
                tool_source: "test".to_string(),
                session_key: session.to_string(),
                tool_name: tool_name.to_string(),
                cwd: Some(self.work_dir.to_string_lossy().to_string()),
                observations,
            }),
        };
        process_hook_event_with_objects_root(&self.db, &envelope, &self.objects_root).unwrap();
    }

    /// Record a post-tool observation for a Bash/shell tool (attribution=bash_predicted).
    fn hook_post_tool_bash(
        &self,
        session: &str,
        observations: Vec<ObservedPathChange>,
    ) {
        let n = self.next_id();
        let envelope = HookEventEnvelope {
            event_id: format!("post_{n}"),
            idempotency_key: format!("post:{session}:{n}"),
            created_at: Utc::now().to_rfc3339(),
            payload_version: 1,
            payload: HookEventPayload::PostToolObserved(PostToolObservedPayload {
                tool_source: "test".to_string(),
                session_key: session.to_string(),
                tool_name: "Bash".to_string(),
                cwd: Some(self.work_dir.to_string_lossy().to_string()),
                observations,
            }),
        };
        process_hook_event_with_objects_root(&self.db, &envelope, &self.objects_root).unwrap();
    }

    fn hook_stop(&self, session: &str) {
        let n = self.next_id();
        let envelope = HookEventEnvelope {
            event_id: format!("stop_{n}"),
            idempotency_key: format!("stop:{session}:{n}"),
            created_at: Utc::now().to_rfc3339(),
            payload_version: 1,
            payload: HookEventPayload::TaskStopRequested(TaskStopRequestedPayload {
                tool_source: "test".to_string(),
                session_key: session.to_string(),
                generation_id: None,
            }),
        };
        process_hook_event_with_objects_root(&self.db, &envelope, &self.objects_root).unwrap();
    }

    fn seed_pre_tool_hash(&self, session: &str, file_path: &Path, hash: &str) {
        let pre_tool_root = rew_core::pre_tool_store::pre_tool_store_root_for_objects_root(
            &self.objects_root,
        );
        let path_str = file_path.to_string_lossy().to_string();
        rew_core::pre_tool_store::set_pre_tool_hash_in(&pre_tool_root, session, &path_str, hash)
            .unwrap();
    }

    // ---- FSEvent pipeline (replicates daemon.rs Phase 3 logic) ----

    fn fsevent(&self, task_id: &str, path: &Path, kind: FsEventKind, attribution: &str) {
        let baseline = resolve_baseline_with_objects_root(
            &self.db,
            task_id,
            path,
            None,
            self.objects_root.clone(),
        );

        let change_type = match kind {
            FsEventKind::Created => {
                if baseline.existed { ChangeType::Modified } else { ChangeType::Created }
            }
            FsEventKind::Modified => {
                if baseline.existed { ChangeType::Modified } else { ChangeType::Created }
            }
            FsEventKind::Deleted => ChangeType::Deleted,
        };

        let obj_store = ObjectStore::new(self.objects_root.clone()).unwrap();
        let new_hash = if path.exists() {
            obj_store.store(path).ok()
        } else {
            None
        };

        let (old_hash, new_hash) = match change_type {
            ChangeType::Created => (None, new_hash),
            ChangeType::Modified => (baseline.hash, new_hash),
            ChangeType::Deleted => {
                if !baseline.existed {
                    (None, None)
                } else {
                    (baseline.hash, None)
                }
            }
            ChangeType::Renamed => unreachable!("fsevent helper does not handle renames"),
        };

        if old_hash.is_some() && new_hash.is_some() && old_hash == new_hash {
            return;
        }
        if matches!(change_type, ChangeType::Created) && new_hash.is_none() {
            return;
        }

        let change = Change {
            id: None,
            task_id: task_id.to_string(),
            file_path: path.to_path_buf(),
            change_type,
            old_hash,
            new_hash,
            diff_text: None,
            lines_added: 0,
            lines_removed: 0,
            attribution: Some(attribution.to_string()),
            old_file_path: None,
        };
        self.db.upsert_change(&change).unwrap();
        sync_file_index_after_change(
            &self.db,
            &change,
            attribution,
            &Utc::now().to_rfc3339(),
        )
        .ok();
    }

    // ---- Finalization ----

    fn finalize(&self, task_id: &str) {
        reconcile_task(&self.db, task_id, &self.objects_root).unwrap();
        self.db.refresh_task_rollup_from_changes(task_id).unwrap();
    }

    fn finalize_queued(&self) -> Vec<String> {
        let mut ids = Vec::new();
        while let Some(job) = self.db.claim_next_task_finalization_job().unwrap() {
            reconcile_task(&self.db, &job.task_id, &self.objects_root).unwrap();
            self.db.refresh_task_rollup_from_changes(&job.task_id).unwrap();
            self.db.mark_task_finalization_done(&job.task_id).unwrap();
            ids.push(job.task_id);
        }
        ids
    }

    // ---- Assertions ----

    fn changes(&self, task_id: &str) -> Vec<Change> {
        self.db.get_changes_for_task(task_id).unwrap()
    }

    fn assert_changes_count(&self, task_id: &str, expected: usize) {
        let changes = self.changes(task_id);
        assert_eq!(
            changes.len(),
            expected,
            "task {task_id}: expected {expected} changes, got {}.\nActual: {:#?}",
            changes.len(),
            changes.iter().map(|c| format!(
                "  {:?}({}, {}) attr={}",
                c.change_type,
                c.old_hash.as_deref().unwrap_or("None"),
                c.new_hash.as_deref().unwrap_or("None"),
                c.attribution.as_deref().unwrap_or("?"),
            )).collect::<Vec<_>>(),
        );
    }

    fn assert_no_changes(&self, task_id: &str) {
        self.assert_changes_count(task_id, 0);
    }

    fn assert_change(
        &self,
        task_id: &str,
        filename: &str,
        expected_type: ChangeType,
    ) {
        let changes = self.changes(task_id);
        let found = changes.iter().find(|c| {
            c.file_path.to_string_lossy().ends_with(filename)
        });
        assert!(
            found.is_some(),
            "No change found for file '{filename}' in task {task_id}.\nChanges: {:?}",
            changes.iter().map(|c| c.file_path.to_string_lossy().to_string()).collect::<Vec<_>>(),
        );
        let c = found.unwrap();
        assert_eq!(
            c.change_type, expected_type,
            "File '{filename}': expected {:?}, got {:?}",
            expected_type, c.change_type,
        );
    }

    fn assert_task_status(&self, task_id: &str, expected: TaskStatus) {
        let task = self.db.get_task(task_id).unwrap().expect("task should exist");
        assert_eq!(
            task.status, expected,
            "task {task_id}: expected {:?}, got {:?}",
            expected, task.status,
        );
    }

    fn assert_attribution(&self, task_id: &str, filename: &str, expected: &str) {
        let changes = self.changes(task_id);
        let c = changes
            .iter()
            .find(|c| c.file_path.to_string_lossy().ends_with(filename))
            .unwrap_or_else(|| panic!("No change for '{filename}' in task {task_id}"));
        assert_eq!(
            c.attribution.as_deref(),
            Some(expected),
            "File '{filename}': expected attribution '{expected}', got {:?}",
            c.attribution,
        );
    }

    // ---- Monitoring task helper ----

    fn create_monitoring_task(&self, task_id: &str) {
        self.db.create_task(&rew_core::types::Task {
            id: task_id.to_string(),
            prompt: None,
            tool: Some("文件监听".to_string()),
            started_at: Utc::now(),
            completed_at: None,
            status: TaskStatus::Active,
            risk_level: None,
            summary: None,
            cwd: None,
        }).unwrap();
    }

    // ---- Restore helper ----

    fn restore_task(&self, task_id: &str) -> rew_core::restore::UndoResult {
        let engine = TaskRestoreEngine::new(self.objects_root.clone())
            .with_cleanup_boundaries(vec![self.work_dir.clone()]);
        engine.undo_task(&self.db, task_id).unwrap()
    }

    // ---- Internal helpers ----

    fn age_task(&self, task_id: &str, seconds: i64) {
        let started_at = (Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339();
        self.db
            .connection()
            .execute(
                "UPDATE tasks SET started_at = ?1 WHERE id = ?2",
                rusqlite::params![started_at, task_id],
            )
            .unwrap();
    }
}

// ============================================================
// Independent Oracle: ground-truth validation via disk snapshots
// ============================================================
//
// Mirrors the approach in git_semantics.rs (assert_git_aligned):
// instead of trusting rew's output, we compare baseline disk state
// vs. final disk state to derive the "correct" answer, then check
// that rew's reconciled changes match.

/// A snapshot of all files under a directory tree: path → content SHA-256.
type DiskSnapshot = BTreeMap<PathBuf, String>;

/// What the oracle thinks a single file change should be.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OracleChange {
    kind: char, // 'A' Created, 'M' Modified, 'D' Deleted
    path: String,
}

impl IntegrationEnv {
    /// Take a snapshot of all files under work_dir: path → SHA-256 hash.
    fn snapshot_disk(&self) -> DiskSnapshot {
        let mut snap = BTreeMap::new();
        Self::walk_dir(&self.work_dir, &mut snap);
        snap
    }

    fn walk_dir(dir: &Path, snap: &mut DiskSnapshot) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map_or(false, |n| n.to_string_lossy().starts_with(".tmp_")) {
                    continue;
                }
                Self::walk_dir(&path, snap);
            } else if path.is_file() {
                if let Ok(hash) = sha256_file(&path) {
                    snap.insert(path, hash);
                }
            }
        }
    }

    /// Compute ground-truth changes by comparing baseline and current disk.
    fn oracle_changes(&self, baseline: &DiskSnapshot) -> Vec<OracleChange> {
        let current = self.snapshot_disk();
        let mut changes = Vec::new();

        for (path, old_hash) in baseline {
            match current.get(path) {
                None => {
                    changes.push(OracleChange {
                        kind: 'D',
                        path: path.strip_prefix(&self.work_dir).unwrap_or(path).to_string_lossy().to_string(),
                    });
                }
                Some(new_hash) if new_hash != old_hash => {
                    changes.push(OracleChange {
                        kind: 'M',
                        path: path.strip_prefix(&self.work_dir).unwrap_or(path).to_string_lossy().to_string(),
                    });
                }
                _ => {} // same hash → no change
            }
        }
        for (path, _) in &current {
            if !baseline.contains_key(path) {
                changes.push(OracleChange {
                    kind: 'A',
                    path: path.strip_prefix(&self.work_dir).unwrap_or(path).to_string_lossy().to_string(),
                });
            }
        }
        changes.sort();
        changes
    }

    /// Convert rew's reconciled changes to the same normalized format.
    fn rew_to_oracle(&self, changes: &[Change]) -> Vec<OracleChange> {
        let mut out: Vec<_> = changes.iter().map(|c| {
            OracleChange {
                kind: match c.change_type {
                    ChangeType::Created => 'A',
                    ChangeType::Modified => 'M',
                    ChangeType::Deleted => 'D',
                    ChangeType::Renamed => 'R',
                },
                path: c.file_path.strip_prefix(&self.work_dir).unwrap_or(&c.file_path)
                    .to_string_lossy().to_string(),
            }
        }).collect();
        out.sort();
        out
    }

    /// Assert that rew's reconciled output matches the oracle's ground truth.
    /// The oracle computes what git would say: baseline → current disk.
    fn assert_oracle_aligned(&self, task_id: &str, baseline: &DiskSnapshot) {
        let rew_changes = self.changes(task_id);
        let oracle = self.oracle_changes(baseline);
        let rew_norm = self.rew_to_oracle(&rew_changes);
        assert_eq!(
            rew_norm, oracle,
            "rew diverges from oracle (disk-snapshot ground truth)\n\
             rew:    {rew_norm:?}\n\
             oracle: {oracle:?}"
        );
    }
}

#[derive(Clone, Copy)]
enum FsEventKind {
    Created,
    Modified,
    Deleted,
}

// ============================================================
// A. Hook native file tools (attribution=hook)
// ============================================================

#[test]
fn a1_create_new_file() {
    let env = IntegrationEnv::new();
    let baseline = env.snapshot_disk();
    let session = "s:a1";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("new.txt", "hello world\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "new.txt", ChangeType::Created);
    env.assert_attribution(&task_id, "new.txt", "hook");
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn a2_modify_existing_file() {
    let env = IntegrationEnv::new();
    let path = env.write_file("existing.txt", "old content\n");
    let old_hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let session = "s:a2";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    std::fs::write(&path, "new content\n").unwrap();
    let new_hash = env.store_file(&path);

    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash.clone()),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "existing.txt", ChangeType::Modified);
    let c = &env.changes(&task_id)[0];
    assert_eq!(c.old_hash.as_deref(), Some(old_hash.as_str()));
    assert_eq!(c.new_hash.as_deref(), Some(new_hash.as_str()));
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn a3_delete_existing_file() {
    let env = IntegrationEnv::new();
    let path = env.write_file("to_delete.txt", "content\n");
    let old_hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let session = "s:a3";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    env.delete_file(&path);
    env.hook_post_tool_native(session, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "to_delete.txt", ChangeType::Deleted);
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn a4_create_then_delete_ephemeral() {
    let env = IntegrationEnv::new();
    let baseline = env.snapshot_disk();
    let session = "s:a4";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("ephemeral.txt", "temp\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.delete_file(&path);
    env.hook_post_tool_native(session, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn a5_modify_then_revert_net_zero() {
    let env = IntegrationEnv::new();
    let path = env.write_file("revert.txt", "original\n");
    let original_hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let session = "s:a5";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &original_hash);

    std::fs::write(&path, "modified\n").unwrap();
    let modified_hash = env.store_file(&path);
    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(modified_hash),
    }]);

    std::fs::write(&path, "original\n").unwrap();
    let reverted_hash = env.store_file(&path);
    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(reverted_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn a6_delete_then_recreate_same_content() {
    let env = IntegrationEnv::new();
    let path = env.write_file("roundtrip.txt", "same content\n");
    let hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let session = "s:a6";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &hash);

    env.delete_file(&path);
    env.hook_post_tool_native(session, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    std::fs::write(&path, "same content\n").unwrap();
    let new_hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn a7_delete_then_recreate_different_content() {
    let env = IntegrationEnv::new();
    let path = env.write_file("replace.txt", "old content\n");
    let old_hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let session = "s:a7";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    env.delete_file(&path);
    env.hook_post_tool_native(session, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    std::fs::write(&path, "brand new content\n").unwrap();
    let new_hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "replace.txt", ChangeType::Modified);
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn a8_modify_multiple_times() {
    let env = IntegrationEnv::new();
    let path = env.write_file("multi.txt", "v1\n");
    let v1_hash = env.store_file(&path);

    let session = "s:a8";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &v1_hash);

    for content in &["v2\n", "v3\n", "v4\n"] {
        std::fs::write(&path, content).unwrap();
        let hash = env.store_file(&path);
        env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
            file_path: path.to_string_lossy().to_string(),
            file_exists_after: true,
            new_hash: Some(hash),
        }]);
    }

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "multi.txt", ChangeType::Modified);
    let c = &env.changes(&task_id)[0];
    assert_eq!(c.old_hash.as_deref(), Some(v1_hash.as_str()), "old_hash should be v1");
}

#[test]
fn a9_create_empty_file() {
    let env = IntegrationEnv::new();
    let session = "s:a9";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("empty.txt", "");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "empty.txt", ChangeType::Created);
}

#[test]
fn a10_modify_append_content() {
    let env = IntegrationEnv::new();
    let path = env.write_file("append.txt", "line1\n");
    let old_hash = env.store_file(&path);

    let session = "s:a10";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    std::fs::write(&path, "line1\nline2\n").unwrap();
    let new_hash = env.store_file(&path);
    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "append.txt", ChangeType::Modified);
}

// ============================================================
// B. Hook Bash tools (attribution=bash_predicted)
// ============================================================

#[test]
fn b1_bash_echo_create() {
    let env = IntegrationEnv::new();
    let baseline = env.snapshot_disk();
    let session = "s:b1";
    let task_id = env.hook_prompt(session, "claude-code");

    let path = env.write_file("bash_created.txt", "echo content\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "bash_created.txt", ChangeType::Created);
    env.assert_attribution(&task_id, "bash_created.txt", "bash_predicted");
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn b2_bash_rm_delete() {
    let env = IntegrationEnv::new();
    let path = env.write_file("to_rm.txt", "content\n");
    let old_hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let session = "s:b2";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    env.delete_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "to_rm.txt", ChangeType::Deleted);
    env.assert_attribution(&task_id, "to_rm.txt", "bash_predicted");
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn b3_bash_create_then_delete() {
    let env = IntegrationEnv::new();
    let baseline = env.snapshot_disk();
    let session = "s:b3";
    let task_id = env.hook_prompt(session, "claude-code");

    let path = env.write_file("bash_ephemeral.txt", "temp\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.delete_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
    env.assert_oracle_aligned(&task_id, &baseline);
}

#[test]
fn b4_bash_sed_modify() {
    let env = IntegrationEnv::new();
    let path = env.write_file("sed_target.txt", "old line\n");
    let old_hash = env.store_file(&path);

    let session = "s:b4";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    std::fs::write(&path, "new line\n").unwrap();
    let new_hash = env.store_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "sed_target.txt", ChangeType::Modified);
    env.assert_attribution(&task_id, "sed_target.txt", "bash_predicted");
}

#[test]
fn b5_bash_cp_create() {
    let env = IntegrationEnv::new();
    let _src = env.write_file("source.txt", "source content\n");

    let session = "s:b5";
    let task_id = env.hook_prompt(session, "claude-code");

    let dst = env.write_file("dest.txt", "source content\n");
    let hash = env.store_file(&dst);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: dst.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "dest.txt", ChangeType::Created);
}

#[test]
fn b6_bash_mv_rename() {
    let env = IntegrationEnv::new();
    let old_path = env.write_file("old_name.txt", "rename me\n");
    let old_hash = env.store_file(&old_path);

    let session = "s:b6";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &old_path, &old_hash);

    let new_path = env.file_path("new_name.txt");
    std::fs::rename(&old_path, &new_path).unwrap();
    let new_hash = env.store_file(&new_path);

    env.hook_post_tool_bash(session, vec![
        ObservedPathChange {
            file_path: old_path.to_string_lossy().to_string(),
            file_exists_after: false,
            new_hash: None,
        },
        ObservedPathChange {
            file_path: new_path.to_string_lossy().to_string(),
            file_exists_after: true,
            new_hash: Some(new_hash),
        },
    ]);

    env.hook_stop(session);
    env.finalize_queued();

    let changes = env.changes(&task_id);
    assert_eq!(changes.len(), 1, "delete + create of same content should pair into rename");
    assert_eq!(changes[0].change_type, ChangeType::Renamed);
}

#[test]
fn b7_bash_touch_create() {
    let env = IntegrationEnv::new();
    let session = "s:b7";
    let task_id = env.hook_prompt(session, "claude-code");

    let path = env.write_file("touched.txt", "");
    let hash = env.store_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "touched.txt", ChangeType::Created);
}

#[test]
fn b8_bash_modify_then_rm() {
    let env = IntegrationEnv::new();
    let path = env.write_file("mod_then_rm.txt", "original\n");
    let original_hash = env.store_file(&path);

    let session = "s:b8";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &path, &original_hash);

    std::fs::write(&path, "modified\n").unwrap();
    let mod_hash = env.store_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(mod_hash),
    }]);

    env.delete_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "mod_then_rm.txt", ChangeType::Deleted);
    let c = &env.changes(&task_id)[0];
    assert!(c.old_hash.is_some(), "old_hash should be the original hash");
}

// ============================================================
// C. Pure FSEvent (manual operations / no AI task)
// ============================================================

#[test]
fn c1_manual_create() {
    let env = IntegrationEnv::new();
    let baseline = env.snapshot_disk();
    let task_id = "fs_manual_c1";
    env.create_monitoring_task(task_id);

    let path = env.write_file("manual_new.txt", "manual content\n");
    env.fsevent(task_id, &path, FsEventKind::Created, "monitoring");
    env.finalize(task_id);

    env.assert_changes_count(task_id, 1);
    env.assert_change(task_id, "manual_new.txt", ChangeType::Created);
    env.assert_attribution(task_id, "manual_new.txt", "monitoring");
    env.assert_oracle_aligned(task_id, &baseline);
}

#[test]
fn c2_manual_modify() {
    let env = IntegrationEnv::new();
    let path = env.write_file("manual_edit.txt", "before\n");
    let old_hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let task_id = "fs_manual_c2";
    env.create_monitoring_task(task_id);

    env.db.upsert_live_file_index_entry(
        &path.to_string_lossy(), 100, "fast", Some(&old_hash),
        "test", "seed", &Utc::now().to_rfc3339(), Some(1),
    ).unwrap();

    std::fs::write(&path, "after\n").unwrap();
    env.fsevent(task_id, &path, FsEventKind::Modified, "monitoring");
    env.finalize(task_id);

    env.assert_changes_count(task_id, 1);
    env.assert_change(task_id, "manual_edit.txt", ChangeType::Modified);
    env.assert_oracle_aligned(task_id, &baseline);
}

#[test]
fn c3_manual_delete() {
    let env = IntegrationEnv::new();
    let path = env.write_file("manual_del.txt", "content\n");
    let hash = env.store_file(&path);
    let baseline = env.snapshot_disk();

    let task_id = "fs_manual_c3";
    env.create_monitoring_task(task_id);

    env.db.upsert_live_file_index_entry(
        &path.to_string_lossy(), 100, "fast", Some(&hash),
        "test", "seed", &Utc::now().to_rfc3339(), Some(1),
    ).unwrap();

    env.delete_file(&path);
    env.fsevent(task_id, &path, FsEventKind::Deleted, "monitoring");
    env.finalize(task_id);

    env.assert_changes_count(task_id, 1);
    env.assert_change(task_id, "manual_del.txt", ChangeType::Deleted);
    env.assert_oracle_aligned(task_id, &baseline);
}

#[test]
fn c4_manual_create_then_delete() {
    let env = IntegrationEnv::new();
    let baseline = env.snapshot_disk();
    let task_id = "fs_manual_c4";
    env.create_monitoring_task(task_id);

    let path = env.write_file("manual_ephemeral.txt", "temp\n");
    env.fsevent(task_id, &path, FsEventKind::Created, "monitoring");
    env.delete_file(&path);
    env.fsevent(task_id, &path, FsEventKind::Deleted, "monitoring");
    env.finalize(task_id);

    env.assert_no_changes(task_id);
    env.assert_oracle_aligned(task_id, &baseline);
}

#[test]
fn c5_fsevent_during_active_ai() {
    let env = IntegrationEnv::new();
    let session = "s:c5";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("manual_during_ai.txt", "manual edit\n");
    env.fsevent(&task_id, &path, FsEventKind::Created, "fsevent_active");

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_attribution(&task_id, "manual_during_ai.txt", "fsevent_active");
}

#[test]
fn c6_fsevent_during_grace() {
    let env = IntegrationEnv::new();
    let session = "s:c6";
    let task_id = env.hook_prompt(session, "test-tool");
    env.hook_stop(session);

    let path = env.write_file("grace_edit.txt", "grace content\n");
    env.fsevent(&task_id, &path, FsEventKind::Created, "fsevent_grace");
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_attribution(&task_id, "grace_edit.txt", "fsevent_grace");
}

// ============================================================
// D. Dual-pipeline competition (Hook + FSEvent on same file)
// ============================================================

#[test]
fn d1_hook_first_fsevent_blocked() {
    let env = IntegrationEnv::new();
    let session = "s:d1";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("d1.txt", "content\n");
    let hash = env.store_file(&path);

    // Hook writes first (attribution=hook)
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    // FSEvent arrives later (attribution=fsevent_active)
    env.fsevent(&task_id, &path, FsEventKind::Created, "fsevent_active");

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_attribution(&task_id, "d1.txt", "hook");
}

#[test]
fn d2_fsevent_first_hook_overwrites() {
    let env = IntegrationEnv::new();
    let path = env.write_file("d2.txt", "old\n");
    let old_hash = env.store_file(&path);

    // Seed file_index so FSEvent can resolve baseline
    env.db.upsert_live_file_index_entry(
        &path.to_string_lossy(), 100, "fast", Some(&old_hash),
        "test", "seed", &Utc::now().to_rfc3339(), Some(1),
    ).unwrap();

    let session = "s:d2";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    // FSEvent writes first
    std::fs::write(&path, "new\n").unwrap();
    env.fsevent(&task_id, &path, FsEventKind::Modified, "fsevent_active");

    // Hook writes later (should overwrite)
    let new_hash = env.store_file(&path);
    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_attribution(&task_id, "d2.txt", "hook");
    let c = &env.changes(&task_id)[0];
    assert_eq!(c.old_hash.as_deref(), Some(old_hash.as_str()),
        "hook's old_hash (from pre-tool) should win");
}

#[test]
fn d3_bash_first_fsevent_blocked() {
    let env = IntegrationEnv::new();
    let session = "s:d3";
    let task_id = env.hook_prompt(session, "claude-code");

    let path = env.write_file("d3.txt", "bash content\n");
    let hash = env.store_file(&path);

    // bash_predicted writes first
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    // FSEvent arrives later — should be blocked by is_hook_source
    env.fsevent(&task_id, &path, FsEventKind::Created, "fsevent_active");

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_attribution(&task_id, "d3.txt", "bash_predicted");
}

#[test]
fn d4_fsevent_first_bash_overwrites() {
    let env = IntegrationEnv::new();
    let path = env.write_file("d4.txt", "old\n");
    let old_hash = env.store_file(&path);

    let session = "s:d4";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    std::fs::write(&path, "new\n").unwrap();
    env.fsevent(&task_id, &path, FsEventKind::Modified, "fsevent_active");

    let new_hash = env.store_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_attribution(&task_id, "d4.txt", "bash_predicted");
}

#[test]
fn d5_bash_create_delete_dual_fsevent() {
    // THE critical scenario: claude-code creates a file via Bash,
    // then deletes via Bash. Both hook and daemon write changes.
    let env = IntegrationEnv::new();
    let session = "s:d5";
    let task_id = env.hook_prompt(session, "claude-code");

    let path = env.write_file("d5_ephemeral.txt", "temp\n");
    let hash = env.store_file(&path);

    // Hook: bash created
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash.clone()),
    }]);
    // Daemon: FSEvent Created
    env.fsevent(&task_id, &path, FsEventKind::Created, "fsevent_active");

    // Hook: bash deleted
    env.delete_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);
    // Daemon: FSEvent Deleted
    env.fsevent(&task_id, &path, FsEventKind::Deleted, "fsevent_active");

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
}

#[test]
fn d6_native_create_delete_dual_fsevent() {
    let env = IntegrationEnv::new();
    let session = "s:d6";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("d6_ephemeral.txt", "temp\n");
    let hash = env.store_file(&path);

    // Hook: native created
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash.clone()),
    }]);
    // Daemon: FSEvent Created
    env.fsevent(&task_id, &path, FsEventKind::Created, "fsevent_active");

    // Hook: native deleted
    env.delete_file(&path);
    env.hook_post_tool_native(session, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);
    // Daemon: FSEvent Deleted
    env.fsevent(&task_id, &path, FsEventKind::Deleted, "fsevent_active");

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
}

#[test]
fn d7_bash_modify_fsevent_different_hash() {
    let env = IntegrationEnv::new();
    let path = env.write_file("d7.txt", "original\n");
    let original_hash = env.store_file(&path);

    let session = "s:d7";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &path, &original_hash);

    // Bash modifies file
    std::fs::write(&path, "modified by bash\n").unwrap();
    let bash_hash = env.store_file(&path);
    env.hook_post_tool_bash(session, vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(bash_hash),
    }]);

    // FSEvent arrives with the same or different hash — should be blocked
    env.fsevent(&task_id, &path, FsEventKind::Modified, "fsevent_active");

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_attribution(&task_id, "d7.txt", "bash_predicted");
}

// ============================================================
// E. Cross-task boundaries
// ============================================================

#[test]
fn e1_task1_create_task2_modify() {
    let env = IntegrationEnv::new();

    // Task 1: create file
    let session1 = "s:e1-t1";
    let _t1 = env.hook_prompt(session1, "test-tool");
    let path = env.write_file("cross.txt", "created in t1\n");
    let t1_hash = env.store_file(&path);
    env.hook_post_tool_native(session1, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(t1_hash.clone()),
    }]);
    env.hook_stop(session1);
    env.finalize_queued();

    // Snapshot before T2 starts (after T1 completed)
    let baseline_t2 = env.snapshot_disk();

    // Task 2: modify same file
    let session2 = "s:e1-t2";
    let t2 = env.hook_prompt(session2, "test-tool");
    env.seed_pre_tool_hash(session2, &path, &t1_hash);
    std::fs::write(&path, "modified in t2\n").unwrap();
    let t2_hash = env.store_file(&path);
    env.hook_post_tool_native(session2, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(t2_hash.clone()),
    }]);
    env.hook_stop(session2);
    env.finalize_queued();

    env.assert_changes_count(&t2, 1);
    let c = &env.changes(&t2)[0];
    assert_eq!(c.old_hash.as_deref(), Some(t1_hash.as_str()),
        "t2's old_hash should be t1's new_hash");
    assert_eq!(c.new_hash.as_deref(), Some(t2_hash.as_str()));
    env.assert_oracle_aligned(&t2, &baseline_t2);
}

#[test]
fn e2_task1_modify_task2_revert() {
    let env = IntegrationEnv::new();
    let path = env.write_file("revert_cross.txt", "original\n");
    let original_hash = env.store_file(&path);

    // Task 1: modify
    let s1 = "s:e2-t1";
    let _t1 = env.hook_prompt(s1, "test-tool");
    env.seed_pre_tool_hash(s1, &path, &original_hash);
    std::fs::write(&path, "changed\n").unwrap();
    let changed_hash = env.store_file(&path);
    env.hook_post_tool_native(s1, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(changed_hash.clone()),
    }]);
    env.hook_stop(s1);
    env.finalize_queued();

    // Snapshot before T2
    let baseline_t2 = env.snapshot_disk();

    // Task 2: revert to original content.
    // From T2's perspective: old_hash=changed_hash, current=original_hash.
    // Since old != current, this IS a real change (Modified), not net-zero.
    // The file went from "changed" back to "original" — that's a modification.
    let s2 = "s:e2-t2";
    let t2 = env.hook_prompt(s2, "test-tool");
    env.seed_pre_tool_hash(s2, &path, &changed_hash);
    std::fs::write(&path, "original\n").unwrap();
    let reverted_hash = env.store_file(&path);
    env.hook_post_tool_native(s2, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(reverted_hash),
    }]);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t2, 1);
    env.assert_change(&t2, "revert_cross.txt", ChangeType::Modified);
    let c = &env.changes(&t2)[0];
    assert_eq!(c.old_hash.as_deref(), Some(changed_hash.as_str()),
        "old_hash should be the state at T2 start");
    env.assert_oracle_aligned(&t2, &baseline_t2);
}

#[test]
fn e3_task1_delete_task2_recreate() {
    let env = IntegrationEnv::new();
    let path = env.write_file("del_recreate.txt", "original\n");
    let hash = env.store_file(&path);

    // Task 1: delete
    let s1 = "s:e3-t1";
    let _t1 = env.hook_prompt(s1, "test-tool");
    env.seed_pre_tool_hash(s1, &path, &hash);
    env.delete_file(&path);
    env.hook_post_tool_native(s1, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);
    env.hook_stop(s1);
    env.finalize_queued();

    // Snapshot before T2 (file is deleted)
    let baseline_t2 = env.snapshot_disk();

    // Task 2: recreate
    let s2 = "s:e3-t2";
    let t2 = env.hook_prompt(s2, "test-tool");
    let path = env.write_file("del_recreate.txt", "new content\n");
    let new_hash = env.store_file(&path);
    env.hook_post_tool_native(s2, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t2, 1);
    env.assert_change(&t2, "del_recreate.txt", ChangeType::Created);
    env.assert_oracle_aligned(&t2, &baseline_t2);
}

#[test]
fn e4_prompt_rollover() {
    let env = IntegrationEnv::new();
    let session = "s:e4";

    let t1 = env.hook_prompt(session, "test-tool");
    env.assert_task_status(&t1, TaskStatus::Active);

    let t2 = env.hook_prompt_with_prompt_text(session, "test-tool", "second prompt");
    env.assert_task_status(&t1, TaskStatus::Completed);
    env.assert_task_status(&t2, TaskStatus::Active);

    assert_ne!(t1, t2);
}

// ============================================================
// F. Multi-file operations
// ============================================================

#[test]
fn f1_three_files_independent() {
    let env = IntegrationEnv::new();
    let session = "s:f1";
    let task_id = env.hook_prompt(session, "test-tool");

    for name in &["file_a.txt", "file_b.txt", "file_c.txt"] {
        let path = env.write_file(name, &format!("content of {name}\n"));
        let hash = env.store_file(&path);
        env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
            file_path: path.to_string_lossy().to_string(),
            file_exists_after: true,
            new_hash: Some(hash),
        }]);
    }

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 3);
    env.assert_change(&task_id, "file_a.txt", ChangeType::Created);
    env.assert_change(&task_id, "file_b.txt", ChangeType::Created);
    env.assert_change(&task_id, "file_c.txt", ChangeType::Created);
}

#[test]
fn f2_mixed_operations() {
    let env = IntegrationEnv::new();
    let path_b = env.write_file("mix_b.txt", "old b\n");
    let hash_b = env.store_file(&path_b);
    let path_c = env.write_file("mix_c.txt", "to delete\n");
    let hash_c = env.store_file(&path_c);

    let session = "s:f2";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path_b, &hash_b);
    env.seed_pre_tool_hash(session, &path_c, &hash_c);

    // Create A
    let path_a = env.write_file("mix_a.txt", "new a\n");
    let hash_a = env.store_file(&path_a);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path_a.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash_a),
    }]);

    // Modify B
    std::fs::write(&path_b, "new b\n").unwrap();
    let new_hash_b = env.store_file(&path_b);
    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path_b.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash_b),
    }]);

    // Delete C
    env.delete_file(&path_c);
    env.hook_post_tool_native(session, "Delete", vec![ObservedPathChange {
        file_path: path_c.to_string_lossy().to_string(),
        file_exists_after: false,
        new_hash: None,
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 3);
    env.assert_change(&task_id, "mix_a.txt", ChangeType::Created);
    env.assert_change(&task_id, "mix_b.txt", ChangeType::Modified);
    env.assert_change(&task_id, "mix_c.txt", ChangeType::Deleted);
}

// ============================================================
// G. Directory operations
// ============================================================

#[test]
fn g1_create_files_in_new_directory() {
    let env = IntegrationEnv::new();
    let session = "s:g1";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("newdir/sub/file.txt", "content\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "file.txt", ChangeType::Created);
}

#[test]
fn g2_rm_rf_directory_with_multiple_files() {
    let env = IntegrationEnv::new();
    let f1 = env.write_file("rmdir/a.txt", "file a\n");
    let h1 = env.store_file(&f1);
    let f2 = env.write_file("rmdir/b.txt", "file b\n");
    let h2 = env.store_file(&f2);
    let f3 = env.write_file("rmdir/sub/c.txt", "file c\n");
    let h3 = env.store_file(&f3);

    let session = "s:g2";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &f1, &h1);
    env.seed_pre_tool_hash(session, &f2, &h2);
    env.seed_pre_tool_hash(session, &f3, &h3);

    std::fs::remove_dir_all(env.work_dir.join("rmdir")).unwrap();

    env.hook_post_tool_bash(session, vec![
        ObservedPathChange {
            file_path: f1.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
        ObservedPathChange {
            file_path: f2.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
        ObservedPathChange {
            file_path: f3.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
    ]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 3);
    env.assert_change(&task_id, "a.txt", ChangeType::Deleted);
    env.assert_change(&task_id, "b.txt", ChangeType::Deleted);
    env.assert_change(&task_id, "c.txt", ChangeType::Deleted);
}

#[test]
fn g3_rename_directory_via_mv() {
    let env = IntegrationEnv::new();
    let f1 = env.write_file("olddir/x.txt", "x content\n");
    let h1 = env.store_file(&f1);
    let f2 = env.write_file("olddir/y.txt", "y content\n");
    let h2 = env.store_file(&f2);

    let session = "s:g3";
    let task_id = env.hook_prompt(session, "claude-code");
    env.seed_pre_tool_hash(session, &f1, &h1);
    env.seed_pre_tool_hash(session, &f2, &h2);

    let new_dir = env.work_dir.join("newdir");
    std::fs::rename(env.work_dir.join("olddir"), &new_dir).unwrap();

    let nf1 = new_dir.join("x.txt");
    let nf2 = new_dir.join("y.txt");
    let nh1 = env.store_file(&nf1);
    let nh2 = env.store_file(&nf2);

    env.hook_post_tool_bash(session, vec![
        ObservedPathChange {
            file_path: f1.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
        ObservedPathChange {
            file_path: f2.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
        ObservedPathChange {
            file_path: nf1.to_string_lossy().to_string(),
            file_exists_after: true, new_hash: Some(nh1),
        },
        ObservedPathChange {
            file_path: nf2.to_string_lossy().to_string(),
            file_exists_after: true, new_hash: Some(nh2),
        },
    ]);

    env.hook_stop(session);
    env.finalize_queued();

    let changes = env.changes(&task_id);
    assert_eq!(changes.len(), 2, "two renames expected");
    assert!(changes.iter().all(|c| c.change_type == ChangeType::Renamed),
        "all should be Renamed, got: {:?}", changes.iter().map(|c| &c.change_type).collect::<Vec<_>>());
}

#[test]
fn g4_create_and_delete_empty_directory_no_changes() {
    let env = IntegrationEnv::new();
    let session = "s:g4";
    let task_id = env.hook_prompt(session, "claude-code");

    let dir = env.work_dir.join("emptydir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::remove_dir(&dir).unwrap();

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
}

// ============================================================
// H. Reconcile rename pairing
// ============================================================

#[test]
fn h1_rename_exact_hash_match() {
    let env = IntegrationEnv::new();
    let old_path = env.write_file("h1_old.txt", "exact match content\n");
    let hash = env.store_file(&old_path);

    let session = "s:h1";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &old_path, &hash);

    let new_path = env.file_path("h1_new.txt");
    std::fs::rename(&old_path, &new_path).unwrap();
    let new_hash = env.store_file(&new_path);

    env.hook_post_tool_native(session, "Edit", vec![
        ObservedPathChange {
            file_path: old_path.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
        ObservedPathChange {
            file_path: new_path.to_string_lossy().to_string(),
            file_exists_after: true, new_hash: Some(new_hash),
        },
    ]);

    env.hook_stop(session);
    env.finalize_queued();

    let changes = env.changes(&task_id);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].change_type, ChangeType::Renamed);
    assert!(changes[0].old_file_path.is_some(), "old_file_path should record the original name");
}

#[test]
fn h2_rename_fuzzy_match_similar_content() {
    let env = IntegrationEnv::new();
    let original = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
    let modified = "line1\nline2\nline3_changed\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";

    let old_path = env.write_file("h2_old.rs", original);
    let old_hash = env.store_file(&old_path);

    let session = "s:h2";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &old_path, &old_hash);

    env.delete_file(&old_path);
    let new_path = env.write_file("h2_new.rs", modified);
    let new_hash = env.store_file(&new_path);

    env.hook_post_tool_native(session, "Edit", vec![
        ObservedPathChange {
            file_path: old_path.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
        ObservedPathChange {
            file_path: new_path.to_string_lossy().to_string(),
            file_exists_after: true, new_hash: Some(new_hash),
        },
    ]);

    env.hook_stop(session);
    env.finalize_queued();

    let changes = env.changes(&task_id);
    assert_eq!(changes.len(), 1, "similar content should pair as rename");
    assert_eq!(changes[0].change_type, ChangeType::Renamed);
}

#[test]
fn h3_no_rename_when_content_completely_different() {
    let env = IntegrationEnv::new();
    let old_path = env.write_file("h3_old.txt", "completely different old content\n");
    let old_hash = env.store_file(&old_path);

    let session = "s:h3";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &old_path, &old_hash);

    env.delete_file(&old_path);
    let new_path = env.write_file("h3_new.py", "#!/usr/bin/env python3\nprint('hello')\n");
    let new_hash = env.store_file(&new_path);

    env.hook_post_tool_native(session, "Edit", vec![
        ObservedPathChange {
            file_path: old_path.to_string_lossy().to_string(),
            file_exists_after: false, new_hash: None,
        },
        ObservedPathChange {
            file_path: new_path.to_string_lossy().to_string(),
            file_exists_after: true, new_hash: Some(new_hash),
        },
    ]);

    env.hook_stop(session);
    env.finalize_queued();

    let changes = env.changes(&task_id);
    assert_eq!(changes.len(), 2, "completely different content should NOT pair");
    let types: Vec<_> = changes.iter().map(|c| &c.change_type).collect();
    assert!(types.contains(&&ChangeType::Deleted));
    assert!(types.contains(&&ChangeType::Created));
}

#[test]
fn h4_type_correction_created_file_existed_before() {
    // FSEvent says Created but file_index knows it existed → should be Modified after reconcile
    let env = IntegrationEnv::new();
    let path = env.write_file("h4.txt", "old\n");
    let old_hash = env.store_file(&path);

    let task_id = "h4_task";
    env.create_monitoring_task(task_id);

    env.db.upsert_live_file_index_entry(
        &path.to_string_lossy(), 100, "fast", Some(&old_hash),
        "test", "seed", &Utc::now().to_rfc3339(), Some(1),
    ).unwrap();

    std::fs::write(&path, "new\n").unwrap();
    env.fsevent(task_id, &path, FsEventKind::Modified, "monitoring");
    env.finalize(task_id);

    env.assert_changes_count(task_id, 1);
    env.assert_change(task_id, "h4.txt", ChangeType::Modified);
}

// ============================================================
// I. Special filenames
// ============================================================

#[test]
fn i1_filename_with_spaces() {
    let env = IntegrationEnv::new();
    let session = "s:i1";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("my folder/my file.txt", "space content\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "my file.txt", ChangeType::Created);
}

#[test]
fn i2_filename_with_chinese_characters() {
    let env = IntegrationEnv::new();
    let session = "s:i2";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("文档/测试文件.txt", "中文内容\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "测试文件.txt", ChangeType::Created);
}

#[test]
fn i3_filename_with_emoji() {
    let env = IntegrationEnv::new();
    let session = "s:i3";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("🚀rocket/📝notes.txt", "emoji path\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "📝notes.txt", ChangeType::Created);
}

#[test]
fn i4_long_path() {
    let env = IntegrationEnv::new();
    let session = "s:i4";
    let task_id = env.hook_prompt(session, "test-tool");

    let deep = "a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p";
    let long_name = "very_long_filename_that_tests_path_length_handling.txt";
    let relative = format!("{deep}/{long_name}");
    let path = env.write_file(&relative, "deep content\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, long_name, ChangeType::Created);
}

#[test]
fn i5_filename_with_special_chars() {
    let env = IntegrationEnv::new();
    let session = "s:i5";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("special-chars/file (1) [backup].txt", "special\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
}

// ============================================================
// J. File content edge cases
// ============================================================

#[test]
fn j1_empty_file_create_modify_delete() {
    let env = IntegrationEnv::new();
    let session = "s:j1";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.write_file("empty_lifecycle.txt", "");
    let h1 = env.store_file(&path);
    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h1.clone()),
    }]);

    std::fs::write(&path, "now has content\n").unwrap();
    let h2 = env.store_file(&path);
    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h2),
    }]);

    env.delete_file(&path);
    env.hook_post_tool_native(session, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false, new_hash: None,
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_no_changes(&task_id);
}

#[test]
fn j2_binary_file() {
    let env = IntegrationEnv::new();
    let session = "s:j2";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.file_path("binary.bin");
    let binary_content: Vec<u8> = (0..256).map(|i| i as u8).collect();
    std::fs::write(&path, &binary_content).unwrap();
    let hash = env.store_file(&path);

    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "binary.bin", ChangeType::Created);
}

#[test]
fn j3_large_file() {
    let env = IntegrationEnv::new();
    let session = "s:j3";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.file_path("large.txt");
    let large_content: String = "x".repeat(1_100_000);
    std::fs::write(&path, &large_content).unwrap();
    let hash = env.store_file(&path);

    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "large.txt", ChangeType::Created);
}

#[test]
fn j4_single_very_long_line() {
    let env = IntegrationEnv::new();
    let session = "s:j4";
    let task_id = env.hook_prompt(session, "test-tool");

    let path = env.file_path("longline.txt");
    let content = "a".repeat(500_000) + "\n";
    std::fs::write(&path, &content).unwrap();
    let hash = env.store_file(&path);

    env.hook_post_tool_native(session, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "longline.txt", ChangeType::Created);
}

#[test]
fn j5_binary_modify() {
    let env = IntegrationEnv::new();
    let path = env.file_path("bin_mod.dat");
    let old_bytes: Vec<u8> = vec![0x00, 0x01, 0x02, 0xFF];
    std::fs::write(&path, &old_bytes).unwrap();
    let old_hash = env.store_file(&path);

    let session = "s:j5";
    let task_id = env.hook_prompt(session, "test-tool");
    env.seed_pre_tool_hash(session, &path, &old_hash);

    let new_bytes: Vec<u8> = vec![0xFF, 0xFE, 0xFD, 0x00];
    std::fs::write(&path, &new_bytes).unwrap();
    let new_hash = env.store_file(&path);

    env.hook_post_tool_native(session, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true,
        new_hash: Some(new_hash),
    }]);

    env.hook_stop(session);
    env.finalize_queued();

    env.assert_changes_count(&task_id, 1);
    env.assert_change(&task_id, "bin_mod.dat", ChangeType::Modified);
}

// ============================================================
// K. Concurrent multi-session
// ============================================================

#[test]
fn k1_two_sessions_different_files() {
    let env = IntegrationEnv::new();

    let s1 = "s:k1-session1";
    let s2 = "s:k1-session2";
    let t1 = env.hook_prompt(s1, "tool-a");
    let t2 = env.hook_prompt(s2, "tool-b");

    let p1 = env.write_file("k1_a.txt", "from session 1\n");
    let h1 = env.store_file(&p1);
    env.hook_post_tool_native(s1, "write_to_file", vec![ObservedPathChange {
        file_path: p1.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h1),
    }]);

    let p2 = env.write_file("k1_b.txt", "from session 2\n");
    let h2 = env.store_file(&p2);
    env.hook_post_tool_native(s2, "write_to_file", vec![ObservedPathChange {
        file_path: p2.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h2),
    }]);

    env.hook_stop(s1);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t1, 1);
    env.assert_change(&t1, "k1_a.txt", ChangeType::Created);
    env.assert_changes_count(&t2, 1);
    env.assert_change(&t2, "k1_b.txt", ChangeType::Created);
}

#[test]
fn k2_two_sessions_interleaved_operations() {
    let env = IntegrationEnv::new();

    let s1 = "s:k2-s1";
    let s2 = "s:k2-s2";
    let t1 = env.hook_prompt(s1, "tool-a");
    let t2 = env.hook_prompt(s2, "tool-b");

    // S1 creates file
    let p1 = env.write_file("k2_shared_area/s1.txt", "s1 content\n");
    let h1 = env.store_file(&p1);
    env.hook_post_tool_native(s1, "write_to_file", vec![ObservedPathChange {
        file_path: p1.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h1),
    }]);

    // S2 creates file in same directory
    let p2 = env.write_file("k2_shared_area/s2.txt", "s2 content\n");
    let h2 = env.store_file(&p2);
    env.hook_post_tool_native(s2, "write_to_file", vec![ObservedPathChange {
        file_path: p2.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h2),
    }]);

    // S1 creates another file
    let p3 = env.write_file("k2_shared_area/s1_extra.txt", "extra\n");
    let h3 = env.store_file(&p3);
    env.hook_post_tool_native(s1, "write_to_file", vec![ObservedPathChange {
        file_path: p3.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h3),
    }]);

    env.hook_stop(s1);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t1, 2);
    env.assert_changes_count(&t2, 1);
}

#[test]
fn k3_session_stops_while_other_continues() {
    let env = IntegrationEnv::new();

    let s1 = "s:k3-s1";
    let s2 = "s:k3-s2";
    let t1 = env.hook_prompt(s1, "tool-a");
    let t2 = env.hook_prompt(s2, "tool-b");

    let p1 = env.write_file("k3_a.txt", "a\n");
    let h1 = env.store_file(&p1);
    env.hook_post_tool_native(s1, "write_to_file", vec![ObservedPathChange {
        file_path: p1.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h1),
    }]);

    env.hook_stop(s1);
    env.finalize_queued();
    env.assert_task_status(&t1, TaskStatus::Completed);
    env.assert_task_status(&t2, TaskStatus::Active);

    let p2 = env.write_file("k3_b.txt", "b\n");
    let h2 = env.store_file(&p2);
    env.hook_post_tool_native(s2, "write_to_file", vec![ObservedPathChange {
        file_path: p2.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h2),
    }]);

    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t1, 1);
    env.assert_changes_count(&t2, 1);
}

// ============================================================
// L. Restore then continue operations
// ============================================================

#[test]
fn l1_restore_created_file_then_new_task_creates_again() {
    let env = IntegrationEnv::new();

    // Task 1: create file
    let s1 = "s:l1-t1";
    let t1 = env.hook_prompt(s1, "test-tool");
    let path = env.write_file("l1.txt", "first version\n");
    let hash = env.store_file(&path);
    env.hook_post_tool_native(s1, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(hash),
    }]);
    env.hook_stop(s1);
    env.finalize_queued();

    // Restore (undo) task 1 → deletes the created file
    let result = env.restore_task(&t1);
    assert_eq!(result.files_deleted, 1);
    assert!(!path.exists(), "file should be deleted after restore");

    // Snapshot AFTER restore — file does not exist
    let baseline_t2 = env.snapshot_disk();

    // Task 2: create same file again.
    let s2 = "s:l1-t2";
    let t2 = env.hook_prompt(s2, "test-tool");
    let path = env.write_file("l1.txt", "second version\n");
    let hash2 = env.store_file(&path);
    env.hook_post_tool_native(s2, "write_to_file", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(hash2),
    }]);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t2, 1);
    env.assert_change(&t2, "l1.txt", ChangeType::Created);
    env.assert_oracle_aligned(&t2, &baseline_t2);
}

#[test]
fn l2_restore_modified_file_then_modify_again() {
    let env = IntegrationEnv::new();
    let path = env.write_file("l2.txt", "original\n");
    let original_hash = env.store_file(&path);

    // Task 1: modify file
    let s1 = "s:l2-t1";
    let t1 = env.hook_prompt(s1, "test-tool");
    env.seed_pre_tool_hash(s1, &path, &original_hash);
    std::fs::write(&path, "modified\n").unwrap();
    let mod_hash = env.store_file(&path);
    env.hook_post_tool_native(s1, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(mod_hash),
    }]);
    env.hook_stop(s1);
    env.finalize_queued();

    // Restore → file goes back to "original"
    let result = env.restore_task(&t1);
    assert_eq!(result.files_restored, 1);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "original\n");

    // Snapshot after restore
    let baseline_t2 = env.snapshot_disk();

    // Task 2: modify same file again
    let s2 = "s:l2-t2";
    let t2 = env.hook_prompt(s2, "test-tool");
    let restored_hash = env.store_file(&path);
    env.seed_pre_tool_hash(s2, &path, &restored_hash);
    std::fs::write(&path, "re-modified\n").unwrap();
    let new_hash = env.store_file(&path);
    env.hook_post_tool_native(s2, "Edit", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(new_hash),
    }]);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t2, 1);
    env.assert_change(&t2, "l2.txt", ChangeType::Modified);
    env.assert_oracle_aligned(&t2, &baseline_t2);
}

#[test]
fn l3_restore_deleted_file_then_delete_again() {
    let env = IntegrationEnv::new();
    let path = env.write_file("l3.txt", "to be deleted\n");
    let hash = env.store_file(&path);

    // Task 1: delete file
    let s1 = "s:l3-t1";
    let t1 = env.hook_prompt(s1, "test-tool");
    env.seed_pre_tool_hash(s1, &path, &hash);
    env.delete_file(&path);
    env.hook_post_tool_native(s1, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false, new_hash: None,
    }]);
    env.hook_stop(s1);
    env.finalize_queued();

    // Restore → file comes back
    let result = env.restore_task(&t1);
    assert_eq!(result.files_restored, 1);
    assert!(path.exists(), "file should be restored");

    // Snapshot after restore
    let baseline_t2 = env.snapshot_disk();

    // Task 2: delete the same file again
    let s2 = "s:l3-t2";
    let t2 = env.hook_prompt(s2, "test-tool");
    let restored_hash = env.store_file(&path);
    env.seed_pre_tool_hash(s2, &path, &restored_hash);
    env.delete_file(&path);
    env.hook_post_tool_native(s2, "Delete", vec![ObservedPathChange {
        file_path: path.to_string_lossy().to_string(),
        file_exists_after: false, new_hash: None,
    }]);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t2, 1);
    env.assert_change(&t2, "l3.txt", ChangeType::Deleted);
    env.assert_oracle_aligned(&t2, &baseline_t2);
}

#[test]
fn l4_restore_multi_file_task_then_partial_redo() {
    let env = IntegrationEnv::new();

    // Task 1: create 3 files
    let s1 = "s:l4-t1";
    let t1 = env.hook_prompt(s1, "test-tool");
    for name in &["l4_a.txt", "l4_b.txt", "l4_c.txt"] {
        let p = env.write_file(name, &format!("content of {name}\n"));
        let h = env.store_file(&p);
        env.hook_post_tool_native(s1, "write_to_file", vec![ObservedPathChange {
            file_path: p.to_string_lossy().to_string(),
            file_exists_after: true, new_hash: Some(h),
        }]);
    }
    env.hook_stop(s1);
    env.finalize_queued();

    // Restore all 3
    let result = env.restore_task(&t1);
    assert_eq!(result.files_deleted, 3);

    // Snapshot after restore — none of the 3 files exist
    let baseline_t2 = env.snapshot_disk();

    // Task 2: only recreate 1 of the 3
    let s2 = "s:l4-t2";
    let t2 = env.hook_prompt(s2, "test-tool");
    let p = env.write_file("l4_b.txt", "new b content\n");
    let h = env.store_file(&p);
    env.hook_post_tool_native(s2, "write_to_file", vec![ObservedPathChange {
        file_path: p.to_string_lossy().to_string(),
        file_exists_after: true, new_hash: Some(h),
    }]);
    env.hook_stop(s2);
    env.finalize_queued();

    env.assert_changes_count(&t2, 1);
    env.assert_change(&t2, "l4_b.txt", ChangeType::Created);
    env.assert_oracle_aligned(&t2, &baseline_t2);
}
