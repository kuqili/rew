//! Background daemon that runs the file watching pipeline and anomaly detection.
//!
//! Integrates the real rew-core Pipeline → RuleEngine → snapshot recording,
//! and emits events to the Tauri frontend for live timeline updates.
#![allow(dead_code)]

use crate::state::{self, AppState, DirScanStatus, DirStatus, PipelineCmd, SuppressedRestorePath};
use crate::tray::{self, TrayStatus};
use rew_core::backup::{BackupEngine, BackupJob};
use rew_core::config::RewConfig;
use rew_core::detector::RuleEngine;
use rew_core::file_index::sync_file_index_after_change;
use rew_core::hook_events::{
    claim_oldest_hook_event, mark_hook_event_done, mark_hook_event_failed, process_hook_event,
    requeue_hook_spool_processing_files,
};
use rew_core::objects::ObjectStore;
use rew_core::pipeline;
use rew_core::pre_tool_store::delete_all_pre_tool_hashes;
use rew_core::scanner::ProgressCallback;
use rew_core::snapshot::tmutil::TmutilWrapper;
use rew_core::traits::AnomalyDetector;
use rew_core::types::{
    Change, ChangeType, FileEventKind, SnapshotTrigger, Task, TaskStatus,
};
use rew_core::rew_home_dir;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{error, info, warn};

const SUPPRESSED_PATH_TTL_SECS: u64 = 90;
const SUPPRESSED_RESTORE_TTL_SECS: u64 = 600;

fn build_task_summary(changes: &[Change]) -> Option<String> {
    if changes.is_empty() {
        return None;
    }

    let created = changes
        .iter()
        .filter(|c| c.change_type == ChangeType::Created)
        .count();
    let modified = changes
        .iter()
        .filter(|c| c.change_type == ChangeType::Modified)
        .count();
    let deleted = changes
        .iter()
        .filter(|c| c.change_type == ChangeType::Deleted)
        .count();

    Some(format!(
        "{} files changed (+{} created, ~{} modified, -{} deleted)",
        changes.len(), created, modified, deleted
    ))
}

fn finalize_task_job(db: &rew_core::db::Database, task_id: &str) -> rew_core::error::RewResult<()> {
    let objects_root = rew_home_dir().join("objects");
    let _ = rew_core::reconcile::reconcile_task(db, task_id, &objects_root);

    let changes = db.get_changes_for_task(task_id)?;
    let (changes_count, _, _) = db.refresh_task_rollup_from_changes(task_id).unwrap_or((
        changes.len() as u32,
        changes.iter().map(|c| c.lines_added).sum(),
        changes.iter().map(|c| c.lines_removed).sum(),
    ));

    if let Some(summary) = build_task_summary(&changes) {
        db.update_task_summary(task_id, &summary)?;
    } else {
        db.clear_task_summary(task_id)?;
    }

    if let Some(task) = db.get_task(task_id)? {
        let end_at = task.completed_at.unwrap_or_else(chrono::Utc::now);
        let duration_secs =
            (end_at - task.started_at).num_milliseconds().max(0) as f64 / 1000.0;
        db.finalize_task_stats(task_id, duration_secs, changes_count as i32)?;
    }

    Ok(())
}

fn start_task_finalization_worker(app: &AppHandle) {
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let db_path = rew_home_dir().join("snapshots.db");
        let db = match rew_core::db::Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                error!("Finalization worker failed to open DB: {}", e);
                return;
            }
        };
        let _ = db.initialize();
        let _ = db.recover_stale_task_finalizations();

        loop {
            match db.claim_next_task_finalization_job() {
                Ok(Some(job)) => {
                    let task_id = job.task_id.clone();
                    let _ = app_handle.emit("task-updated", serde_json::json!({
                        "task_id": task_id,
                        "finalization_status": "running",
                    }));
                    match finalize_task_job(&db, &job.task_id) {
                        Ok(()) => {
                            let _ = db.mark_task_finalization_done(&job.task_id);
                            let _ = app_handle.emit("task-updated", serde_json::json!({
                                "task_id": job.task_id,
                                "finalization_status": "done",
                            }));
                        }
                        Err(err) => {
                            error!("Failed to finalize task {}: {}", job.task_id, err);
                            let _ = db.mark_task_finalization_failed(&job.task_id, &err.to_string());
                            let _ = app_handle.emit("task-updated", serde_json::json!({
                                "task_id": job.task_id,
                                "finalization_status": "failed",
                            }));
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                    }
                }
                Ok(None) => {
                    std::thread::sleep(std::time::Duration::from_millis(350));
                }
                Err(err) => {
                    error!("Finalization worker queue error: {}", err);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    });
}

fn start_hook_event_writer(app: &AppHandle) {
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let db_path = rew_home_dir().join("snapshots.db");
        let db = match rew_core::db::Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                error!("Hook event writer failed to open DB: {}", e);
                return;
            }
        };
        let _ = db.initialize();
        let _ = requeue_hook_spool_processing_files();

        loop {
            match claim_oldest_hook_event() {
                Ok(Some((processing_path, envelope))) => {
                    match process_hook_event(&db, &envelope) {
                        Ok(outcome) => {
                            let _ = mark_hook_event_done(&processing_path);
                            if let Some(task_id) = outcome.task_id {
                                let _ = app_handle.emit("task-updated", &task_id);
                            }
                        }
                        Err(err) => {
                            error!("Failed to process hook event {}: {}", envelope.event_id, err);
                            let _ = mark_hook_event_failed(&processing_path, &err.to_string());
                        }
                    }
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(120)),
                Err(err) => {
                    error!("Hook event writer queue error: {}", err);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    });
}

fn should_suppress_restore_event(
    event: &rew_core::types::FileEvent,
    entry: &SuppressedRestorePath,
) -> bool {
    if entry.deleted {
        return !event.path.exists();
    }

    match event.kind {
        FileEventKind::Created => true,
        FileEventKind::Modified | FileEventKind::Renamed => {
            let Some(expected_hash) = entry.expected_content_hash.as_deref() else {
                return false;
            };
            if !event.path.exists() {
                return false;
            }
            rew_core::objects::sha256_file(&event.path)
                .map(|hash| hash == expected_hash)
                .unwrap_or(false)
        }
        FileEventKind::Deleted => false,
    }
}

fn resolve_fsevent_task_route(
    db: &rew_core::db::Database,
    grace_seconds: i64,
) -> (Option<String>, &'static str) {
    if let Ok(Some(tid)) = db.get_most_recent_active_task_id() {
        (Some(tid), "fsevent_active")
    } else if let Ok(Some(tid)) = db.get_recently_completed_task_id(grace_seconds) {
        (Some(tid), "fsevent_grace")
    } else {
        (None, "monitoring")
    }
}

fn expand_deleted_directory_events(
    db: &rew_core::db::Database,
    events: &[rew_core::types::FileEvent],
) -> (
    Vec<rew_core::types::FileEvent>,
    Vec<(std::path::PathBuf, usize)>,
) {
    let existing_paths: std::collections::HashSet<_> =
        events.iter().map(|event| event.path.clone()).collect();
    let mut expanded = Vec::with_capacity(events.len());
    let mut deleted_dirs = Vec::new();

    for event in events {
        if event.kind != FileEventKind::Deleted || event.path.exists() {
            expanded.push(event.clone());
            continue;
        }

        let descendants = db
            .list_live_files_under_dir(&event.path)
            .unwrap_or_default()
            .into_iter()
            .filter(|path| !existing_paths.contains(path))
            .collect::<Vec<_>>();

        if descendants.is_empty() {
            expanded.push(event.clone());
            continue;
        }

        deleted_dirs.push((event.path.clone(), descendants.len()));

        info!(
            "Expanded deleted directory {} into {} file-level deletions",
            event.path.display(),
            descendants.len()
        );

        for path in descendants {
            expanded.push(rew_core::types::FileEvent {
                path,
                kind: FileEventKind::Deleted,
                timestamp: event.timestamp,
                size_bytes: None,
            });
        }
    }

    (expanded, deleted_dirs)
}

/// Start the background protection daemon.
/// Runs file watching, anomaly detection, and snapshot management.
pub fn start_daemon(app: &AppHandle) {
    // On startup: clean up stale AI tasks left behind by interrupted sessions,
    // then restore the previous monitoring window.
    recover_stale_ai_tasks_on_startup(app);
    restore_monitoring_window_from_db(app);
    start_hook_event_writer(app);
    start_task_finalization_worker(app);

    let app_handle = app.clone();

    // Read config to get watch dirs
    let config = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock().unwrap();
        cfg.clone()
    };

    // === Background full scan with progress reporting ===
    let scan_config = config.clone();
    let scan_progress = {
        let state = app.state::<AppState>();
        state.scan_progress.clone()
    };
    let scan_app = app.clone();

    std::thread::spawn(move || {
        info!("Starting background full scan of protected directories...");

        // Initialize progress state: preserve completed dirs, add missing ones as pending
        {
            let mut progress = scan_progress.lock().unwrap();
            progress.is_scanning = true;

            // Build set of dirs from config
            let config_dirs: std::collections::HashSet<String> = scan_config.watch_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect();

            // Remove dirs no longer in config
            progress.dirs.retain(|d| config_dirs.contains(&d.path));

            // Add new dirs that aren't in progress yet; reset "scanning" (interrupted) to "pending"
            for dir in &scan_config.watch_dirs {
                let dir_str = dir.display().to_string();
                if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == dir_str) {
                    // If it was mid-scan when app quit, reset to pending
                    if ds.status == DirStatus::Scanning {
                        ds.status = DirStatus::Pending;
                    }
                    // Complete dirs stay complete — incremental scan will skip quickly
                } else {
                    progress.dirs.push(DirScanStatus {
                        path: dir_str,
                        status: DirStatus::Pending,
                        files_total: 0,
                        files_done: 0,
                        files_stored: 0,
                        files_skipped: 0,
                        files_failed: 0,
                        last_completed_at: None,
                    });
                }
            }
        }

        // Build the progress callback
        let sp = scan_progress.clone();
        let ah = scan_app.clone();
        let callback: ProgressCallback = Box::new(move |update| {
            // Update shared state
            if let Ok(mut progress) = sp.lock() {
                progress.current_dir = Some(update.dir.clone());
                if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == update.dir) {
                    // Previously completed dirs: keep showing "complete" during incremental rescan.
                    // User doesn't need to see the brief scanning state for already-protected dirs.
                    if ds.status != DirStatus::Complete {
                        ds.status = DirStatus::Scanning;
                    }
                    ds.files_total = update.files_total_estimate;
                    ds.files_done = update.files_scanned;
                    ds.files_stored = update.files_stored;
                    ds.files_skipped = update.files_skipped;
                    ds.files_failed = update.files_failed;
                }
            }
            // Emit event to frontend (throttled by scanner at every 100 files)
            let _ = ah.emit("scan-progress", &update);
        });

        // Build dir-complete callback: mark each dir as "complete" immediately
        let sp_dir = scan_progress.clone();
        let dir_complete: rew_core::scanner::DirCompleteCallback = Box::new(move |dir_path| {
            if let Ok(mut progress) = sp_dir.lock() {
                if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == dir_path) {
                    ds.status = DirStatus::Complete;
                    ds.last_completed_at = Some(chrono::Utc::now().to_rfc3339());
                }
                // Persist after each dir completes
                state::save_scan_status(&progress);
            }
        });

        // Merge per-directory ignore rules into scan patterns so they apply
        // during initial scan too (not just daemon batch processing).
        let mut scan_patterns = scan_config.ignore_patterns.clone();
        for (dir_str, dir_cfg) in &scan_config.dir_ignore {
            for excluded in &dir_cfg.exclude_dirs {
                if excluded.contains('/') {
                    scan_patterns.push(format!("{}/{}/**", dir_str, excluded));
                } else {
                    scan_patterns.push(format!("{}/{}/**", dir_str, excluded));
                }
            }
            for ext in &dir_cfg.exclude_extensions {
                let bare = ext.strip_prefix('.').unwrap_or(ext);
                scan_patterns.push(format!("{}/**/*.{}", dir_str, bare));
                scan_patterns.push(format!("{}/**/.{}", dir_str, bare));
            }
        }

        // Open a dedicated DB connection for the scanner thread (Connection is !Send,
        // so we open a fresh one rather than sharing the daemon's Mutex<Database>).
        let scan_db = match rew_core::db::Database::open(&rew_home_dir().join("snapshots.db")) {
            Ok(db) => { let _ = db.initialize(); db }
            Err(e) => {
                warn!("Scanner: failed to open DB, falling back without file_index: {}", e);
                // Cannot continue without DB in new architecture
                return;
            }
        };

        let result = rew_core::scanner::full_scan(
            &scan_config.watch_dirs,
            &scan_patterns,
            &rew_home_dir(),
            &scan_db,
            Some(callback),
            Some(dir_complete),
            scan_config.max_file_size_bytes,
        );

        // Mark all dirs complete and persist
        {
            let mut progress = scan_progress.lock().unwrap();
            progress.is_scanning = false;
            progress.current_dir = None;
            let now = chrono::Utc::now().to_rfc3339();
            for ds in &mut progress.dirs {
                if ds.status == DirStatus::Scanning || ds.status == DirStatus::Pending {
                    ds.status = DirStatus::Complete;
                    ds.last_completed_at = Some(now.clone());
                }
            }
            state::save_scan_status(&progress);
        }

        // Final event
        let _ = scan_app.emit("scan-complete", serde_json::json!({
            "files_scanned": result.files_scanned,
            "files_stored": result.files_stored,
            "elapsed_secs": result.elapsed.as_secs_f64(),
        }));

        info!(
            "Background scan finished: {} scanned, {} stored, {} skipped, {} failed ({:.1}s)",
            result.files_scanned, result.files_stored, result.files_skipped,
            result.files_failed, result.elapsed.as_secs_f64(),
        );
    });

    // Spawn the main protection loop on a dedicated thread with its own tokio runtime
    std::thread::spawn(move || {
        info!("rew daemon starting real pipeline...");

        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                error!("Failed to create tokio runtime for daemon: {}", e);
                return;
            }
        };

        rt.block_on(async {
            if let Err(e) = run_pipeline(&app_handle, &config).await {
                error!("Pipeline error: {}", e);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{
        expand_deleted_directory_events, resolve_fsevent_task_route, should_suppress_restore_event,
        sync_file_index_after_change,
    };
    use crate::state::SuppressedRestorePath;
    use chrono::{Duration, Utc};
    use rew_core::db::Database;
    use rew_core::types::{Change, ChangeType, FileEvent, FileEventKind, Task, TaskStatus};
    use std::path::PathBuf;
    use std::time::Instant;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        (dir, db)
    }

    fn create_task(
        db: &Database,
        id: &str,
        started_at: chrono::DateTime<Utc>,
        completed_at: Option<chrono::DateTime<Utc>>,
        status: TaskStatus,
    ) {
        let task = Task {
            id: id.to_string(),
            prompt: Some("test".into()),
            tool: Some("test-tool".into()),
            started_at,
            completed_at,
            status,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();
    }

    #[test]
    fn route_prefers_most_recent_active_task() {
        let (_dir, db) = temp_db();
        let now = Utc::now();

        create_task(&db, "t_old", now - Duration::seconds(10), None, TaskStatus::Active);
        db.insert_active_session("s_old", "t_old", "test", now - Duration::seconds(10))
            .unwrap();

        create_task(&db, "t_new", now, None, TaskStatus::Active);
        db.insert_active_session("s_new", "t_new", "test", now)
            .unwrap();

        let (task_id, attribution) = resolve_fsevent_task_route(&db, 15);
        assert_eq!(task_id.as_deref(), Some("t_new"));
        assert_eq!(attribution, "fsevent_active");
    }

    #[test]
    fn route_falls_back_to_recently_completed_task() {
        let (_dir, db) = temp_db();
        let now = Utc::now();

        create_task(
            &db,
            "t_done",
            now - Duration::seconds(20),
            Some(now - Duration::seconds(5)),
            TaskStatus::Completed,
        );

        let (task_id, attribution) = resolve_fsevent_task_route(&db, 15);
        assert_eq!(task_id.as_deref(), Some("t_done"));
        assert_eq!(attribution, "fsevent_grace");
    }

    #[test]
    fn route_prefers_latest_active_task_even_across_multiple_ai_tools() {
        let (_dir, db) = temp_db();
        let now = Utc::now();

        create_task(
            &db,
            "t_codebuddy",
            now - Duration::seconds(8),
            None,
            TaskStatus::Active,
        );
        db.insert_active_session(
            "codebuddy:session",
            "t_codebuddy",
            "codebuddy",
            now - Duration::seconds(8),
        )
        .unwrap();

        create_task(
            &db,
            "t_cursor",
            now - Duration::seconds(2),
            None,
            TaskStatus::Active,
        );
        db.insert_active_session("cursor:conv", "t_cursor", "cursor", now - Duration::seconds(2))
            .unwrap();

        let (task_id, attribution) = resolve_fsevent_task_route(&db, 15);
        assert_eq!(task_id.as_deref(), Some("t_cursor"));
        assert_eq!(attribution, "fsevent_active");
    }

    #[test]
    fn route_keeps_completed_task_just_inside_grace_boundary() {
        let (_dir, db) = temp_db();
        let now = Utc::now();

        create_task(
            &db,
            "t_boundary",
            now - Duration::seconds(30),
            Some(now - Duration::seconds(14)),
            TaskStatus::Completed,
        );

        let (task_id, attribution) = resolve_fsevent_task_route(&db, 15);
        assert_eq!(task_id.as_deref(), Some("t_boundary"));
        assert_eq!(attribution, "fsevent_grace");
    }

    #[test]
    fn route_switches_to_monitoring_immediately_after_grace_expires() {
        let (_dir, db) = temp_db();
        let now = Utc::now();

        create_task(
            &db,
            "t_expired",
            now - Duration::seconds(30),
            Some(now - Duration::seconds(16)),
            TaskStatus::Completed,
        );

        let (task_id, attribution) = resolve_fsevent_task_route(&db, 15);
        assert!(task_id.is_none());
        assert_eq!(attribution, "monitoring");
    }

    #[test]
    fn route_falls_back_to_monitoring_when_no_ai_task_matches() {
        let (_dir, db) = temp_db();
        let now = Utc::now();

        create_task(
            &db,
            "t_old_done",
            now - Duration::seconds(200),
            Some(now - Duration::seconds(120)),
            TaskStatus::Completed,
        );

        let (task_id, attribution) = resolve_fsevent_task_route(&db, 15);
        assert!(task_id.is_none());
        assert_eq!(attribution, "monitoring");
    }

    #[test]
    fn deleted_directory_expands_into_known_child_files() {
        let (dir, db) = temp_db();
        let root = dir.path().join("go");
        let child_a = root.join("main.rs");
        let child_b = root.join("pkg/lib.rs");

        let now = Utc::now().to_rfc3339();
        db.upsert_live_file_index_entry(&child_a.to_string_lossy(), 1, "fast-a", Some("sha-a"), "scanner", "scan_seen", &now, Some(1))
            .unwrap();
        db.upsert_live_file_index_entry(&child_b.to_string_lossy(), 1, "fast-b", Some("sha-b"), "scanner", "scan_seen", &now, Some(1))
            .unwrap();

        let (expanded, deleted_dirs) = expand_deleted_directory_events(
            &db,
            &[FileEvent {
                path: root.clone(),
                kind: FileEventKind::Deleted,
                timestamp: Utc::now(),
                size_bytes: None,
            }],
        );

        let paths: Vec<PathBuf> = expanded.into_iter().map(|event| event.path).collect();
        assert_eq!(paths, vec![child_a, child_b]);
        assert_eq!(deleted_dirs, vec![(root, 2)]);
    }

    #[test]
    fn deleted_directory_does_not_duplicate_existing_child_events() {
        let (dir, db) = temp_db();
        let root = dir.path().join("go");
        let child_a = root.join("main.rs");
        let child_b = root.join("pkg/lib.rs");

        let now = Utc::now().to_rfc3339();
        db.upsert_live_file_index_entry(&child_a.to_string_lossy(), 1, "fast-a", Some("sha-a"), "scanner", "scan_seen", &now, Some(1))
            .unwrap();
        db.upsert_live_file_index_entry(&child_b.to_string_lossy(), 1, "fast-b", Some("sha-b"), "scanner", "scan_seen", &now, Some(1))
            .unwrap();

        let (expanded, deleted_dirs) = expand_deleted_directory_events(
            &db,
            &[
                FileEvent {
                    path: root,
                    kind: FileEventKind::Deleted,
                    timestamp: Utc::now(),
                    size_bytes: None,
                },
                FileEvent {
                    path: child_a.clone(),
                    kind: FileEventKind::Deleted,
                    timestamp: Utc::now(),
                    size_bytes: None,
                },
            ],
        );

        let paths: Vec<PathBuf> = expanded.into_iter().map(|event| event.path).collect();
        assert_eq!(paths, vec![child_b, child_a]);
        assert_eq!(deleted_dirs, vec![(dir.path().join("go"), 1)]);
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

        let _ = sync_file_index_after_change(&db, &change, "daemon", &Utc::now().to_rfc3339());

        let entry = db
            .get_file_index_entry(&file_path.to_string_lossy())
            .unwrap()
            .unwrap();
        assert!(entry.exists_now);
        assert_eq!(entry.last_event_kind.as_deref(), Some("delete_ignored_live"));
        assert!(entry.deleted_at.is_none());
    }

    #[test]
    fn renamed_change_tombstones_old_path_and_rehydrates_new_path() {
        let (dir, db) = temp_db();
        let old_path = dir.path().join("old.txt");
        let new_path = dir.path().join("new.txt");
        std::fs::write(&new_path, "renamed content").unwrap();
        let hash = rew_core::objects::sha256_file(&new_path).unwrap();

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

        let _ = sync_file_index_after_change(&db, &change, "daemon", &Utc::now().to_rfc3339());

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
    fn restore_suppression_keeps_filtering_matching_restore_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();
        let expected_hash = rew_core::objects::sha256_file(&file_path).unwrap();

        let event = FileEvent {
            path: file_path,
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: None,
        };
        let entry = SuppressedRestorePath {
            created_at: Instant::now(),
            expected_content_hash: Some(expected_hash),
            deleted: false,
        };

        assert!(should_suppress_restore_event(&event, &entry));
    }

    #[test]
    fn restore_suppression_allows_real_user_edit_after_content_diverges() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();
        let restored_hash = rew_core::objects::sha256_file(&file_path).unwrap();
        std::fs::write(&file_path, "fn main() { println!(\"changed\"); }\n").unwrap();

        let event = FileEvent {
            path: file_path,
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: None,
        };
        let entry = SuppressedRestorePath {
            created_at: Instant::now(),
            expected_content_hash: Some(restored_hash),
            deleted: false,
        };

        assert!(!should_suppress_restore_event(&event, &entry));
    }

    #[test]
    fn directory_restore_suppression_allows_edit_inside_restored_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("go/pkg");
        let restored_a = root.join("a.rs");
        let restored_b = root.join("b.rs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&restored_a, "fn a() {}\n").unwrap();
        std::fs::write(&restored_b, "fn b() {}\n").unwrap();
        let restored_a_hash = rew_core::objects::sha256_file(&restored_a).unwrap();
        let restored_b_hash = rew_core::objects::sha256_file(&restored_b).unwrap();

        let restored_event_a = FileEvent {
            path: restored_a.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: None,
        };
        let restored_event_b = FileEvent {
            path: restored_b.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: None,
        };
        let entry_a = SuppressedRestorePath {
            created_at: Instant::now(),
            expected_content_hash: Some(restored_a_hash),
            deleted: false,
        };
        let entry_b = SuppressedRestorePath {
            created_at: Instant::now(),
            expected_content_hash: Some(restored_b_hash),
            deleted: false,
        };

        assert!(should_suppress_restore_event(&restored_event_a, &entry_a));
        assert!(should_suppress_restore_event(&restored_event_b, &entry_b));

        std::fs::write(&restored_b, "fn b() { println!(\"edited\"); }\n").unwrap();
        let user_edit_event = FileEvent {
            path: restored_b.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: None,
        };

        assert!(
            !should_suppress_restore_event(&user_edit_event, &entry_b),
            "real user edits under a restored directory must not stay suppressed"
        );
        assert!(
            should_suppress_restore_event(&restored_event_a, &entry_a),
            "neighboring restored files should still suppress the restore echo"
        );
    }
}

/// Run the real file-watching pipeline.
async fn run_pipeline(
    app: &AppHandle,
    config: &RewConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check if there are valid watch directories
    let valid_dirs: Vec<_> = config.watch_dirs.iter().filter(|d| d.exists()).collect();
    if valid_dirs.is_empty() {
        warn!("No valid watch directories found. Directories configured: {:?}", config.watch_dirs);
        warn!("Daemon will wait until directories are configured via the setup wizard.");
        // Poll until config is updated with valid directories (e.g. after setup wizard)
        let config = loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(cfg) = state.config.lock() {
                    let has_dirs = cfg.watch_dirs.iter().any(|d| d.exists());
                    if has_dirs {
                        info!("Watch directories now available, starting pipeline");
                        break cfg.clone();
                    }
                }
            }
        };
        // Recurse with updated config
        return Box::pin(run_pipeline(app, &config)).await;
    }

    info!(
        "Starting pipeline with {} watch directories: {:?}",
        valid_dirs.len(),
        valid_dirs
    );

    // Start the pipeline (watcher → processor → batch output)
    let mut handle = pipeline::start_pipeline(config)?;

    // Create a command channel so Tauri commands can hot-add/remove watch paths.
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<PipelineCmd>();
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut tx_guard) = state.pipeline_tx.lock() {
            *tx_guard = Some(cmd_tx);
        }
    }

    // === L0: Create APFS volume snapshot at startup (safety net) ===
    // This covers all files that exist right now, even if rew process crashes later.
    // tmutil localsnapshot does NOT require sudo on macOS 12+.
    let tmutil = TmutilWrapper::default();
    if tmutil.is_available() {
        match tmutil.create_snapshot() {
            Ok(date) => {
                info!(
                    "L0 APFS snapshot created at startup: com.apple.TimeMachine.{}.local",
                    date
                );
            }
            Err(e) => {
                warn!("Failed to create L0 APFS snapshot (non-fatal): {}", e);
            }
        }
    } else {
        warn!("tmutil not available — L0 APFS snapshot disabled");
    }

    // Create rule engine for anomaly detection
    let rule_engine = RuleEngine::new(config.anomaly_rules.clone(), config.watch_dirs.clone());

    // Central path filter: merges built-in patterns (dist/, node_modules/,
    // .git/, OS noise, etc.) with user-configured ignore_patterns.
    // Used to filter ALL incoming FSEvents before any DB/window logic runs.
    let path_filter = rew_core::watcher::filter::PathFilter::new(&config.ignore_patterns)
        .unwrap_or_else(|e| {
            warn!("PathFilter init failed ({e}), using defaults");
            rew_core::watcher::filter::PathFilter::new(&[]).unwrap()
        });

    // Create backup engine for file backups
    let backup_engine = BackupEngine::new(config)?;

    info!("File watching pipeline is now active");

    // Main event processing loop
    loop {
        // Check if paused
        let paused = app
            .try_state::<AppState>()
            .and_then(|s| s.paused.lock().ok().map(|p| *p))
            .unwrap_or(false);

        if paused {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        }

        // Wait for next batch from the pipeline (with timeout to check pause state)
        tokio::select! {
            // Hot add/remove paths from Tauri commands
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(PipelineCmd::AddPath(path)) => {
                        if let Err(e) = handle.add_watch_path(&path) {
                            warn!("Failed to hot-add watch path {:?}: {}", path, e);
                        } else {
                            info!("Hot-added watch path: {:?}", path);
                        }
                    }
                    Some(PipelineCmd::RemovePath(path)) => {
                        if let Err(e) = handle.remove_watch_path(&path) {
                            warn!("Failed to hot-remove watch path {:?}: {}", path, e);
                        } else {
                            info!("Hot-removed watch path: {:?}", path);
                        }
                    }
                    None => {
                        info!("Pipeline command channel closed");
                    }
                }
            }
            batch_result = handle.recv_batch() => {
                match batch_result {
                    Some((event_batch, _stats)) => {
                        let added = event_batch.count_by_kind(&FileEventKind::Created);
                        let modified = event_batch.count_by_kind(&FileEventKind::Modified);
                        let deleted = event_batch.count_by_kind(&FileEventKind::Deleted);
                        let total = event_batch.events.len();

                        info!(
                            "Batch received: {} events (added={}, modified={}, deleted={})",
                            total, added, modified, deleted
                        );

                        // Run anomaly detection
                        let alerts = rule_engine.analyze(&event_batch);
                        let is_anomaly = !alerts.is_empty();

                        if is_anomaly {
                            for alert in &alerts {
                                warn!(
                                    "ANOMALY: [{}] {} — {}",
                                    alert.severity,
                                    alert.kind_str(),
                                    alert.description
                                );
                            }
                        }

                        // Create snapshot ID early so we can use it for backups
                        let snapshot_id = uuid::Uuid::new_v4();

                        // === BACKUP PHASE: Backup modified/deleted files ===
                        let backup_root = rew_home_dir().join("backups");
                        let backup_job = BackupJob {
                            snapshot_id,
                            events: event_batch.events.clone(),
                            backup_root: backup_root.clone(),
                        };

                        let backup_result = match backup_engine.backup_batch(&backup_job) {
                            Ok(result) => {
                                info!(
                                    "Backup completed: {} files backed up ({} bytes)",
                                    result.files_backed_up,
                                    result.total_size_bytes
                                );
                                if !result.failed_files.is_empty() {
                                    warn!(
                                        "Backup had {} failures: {:?}",
                                        result.failed_files.len(),
                                        result.failed_files.iter().take(3).collect::<Vec<_>>()
                                    );
                                }
                                Some(result)
                            }
                            Err(e) => {
                                error!("Backup failed: {}", e);
                                None
                            }
                        };

                        // Record a snapshot in the database
                        let trigger = if is_anomaly {
                            SnapshotTrigger::Anomaly
                        } else {
                            SnapshotTrigger::Auto
                        };

                        // === V3: Unified FSEvent recording (single source of truth) ===
                        //
                        // FSEvents is the only reliable source of ground truth for file changes.
                        // The AI hook (rew-cli) provides intent metadata (prompt, tool name) but
                        // cannot enumerate Bash-induced file changes. So the daemon records ALL
                        // real file changes — but routes them to different task records:
                        //
                        //   • AI task active (.current_task marker exists)
                        //       → attribute changes to the active AI task_id
                        //       → close/freeze the current monitoring window
                        //       → `upsert_change` is idempotent: if the AI hook already wrote
                        //         the (task_id, file_path) record (for Write/Edit ops), the
                        //         daemon update just refreshes new_hash without losing old_hash.
                        //         Bash-induced changes (not reported by hook) get filled in here.
                        //
                        //   • No AI task (monitoring mode)
                        //       → attribute to the current time-window monitoring task
                        //       → multiple 30-second batches coalesce into one window entry
                        //
                        //   • Rollback in progress
                        //       → skip entirely (rollback writes its own files; don't re-record)
                        //
                        // Two paths are MUTUALLY EXCLUSIVE because the routing decision is made
                        // once per batch based on .current_task, not on per-file heuristics.

                        let real_events: Vec<_> = event_batch.events.iter()
                            .filter(|e| !path_filter.should_ignore(&e.path))
                            .cloned()
                            .collect();

                        if !real_events.is_empty() {
                            if let Some(state) = app.try_state::<AppState>() {
                                let is_rolling_back = state.rolling_back
                                    .lock()
                                    .map(|g| *g)
                                    .unwrap_or(false);

                                // Suppression: filter out events recently written by GUI
                                // rollback operations.
                                // Exact path suppression still handles single-file/task undo.
                                // Directory restore now records exact restored/deleted paths
                                // with expected post-restore hashes, so we suppress only the
                                // restore tail itself, not future real edits under the same dir.
                                //
                                // NOTE: `.stop_suppress` (paths from a recently-ended AI task)
                                // is intentionally NOT consumed here — it must only filter
                                // monitoring-window events, never AI-task events, so that a new
                                // AI task can still record deletions/modifications on files that
                                // were touched by the previous task.
                                let real_events: Vec<_> = {
                                    let now = std::time::Instant::now();
                                    if let Ok(mut sp) = state.suppressed_paths.lock() {
                                        sp.retain(|_, t| now.duration_since(*t).as_secs() < SUPPRESSED_PATH_TTL_SECS);
                                    }
                                    if let Ok(mut suppressed_restore_paths) = state.suppressed_restore_paths.lock() {
                                        suppressed_restore_paths.retain(|_, entry| {
                                            now.duration_since(entry.created_at).as_secs() < SUPPRESSED_RESTORE_TTL_SECS
                                        });
                                    }

                                    let exact_paths = state
                                        .suppressed_paths
                                        .lock()
                                        .ok()
                                        .map(|sp| sp.keys().cloned().collect::<Vec<_>>())
                                        .unwrap_or_default();
                                    let restore_paths = state
                                        .suppressed_restore_paths
                                        .lock()
                                        .ok()
                                        .map(|sp| sp.clone())
                                        .unwrap_or_default();

                                    real_events.into_iter()
                                        .filter(|e| {
                                            if exact_paths.iter().any(|path| path == &e.path) {
                                                return false;
                                            }

                                            if let Some(entry) = restore_paths.get(&e.path) {
                                                if should_suppress_restore_event(e, entry) {
                                                    return false;
                                                }
                                            }

                                            true
                                        })
                                        .collect()
                                };

                                // Per-directory ignore filtering: skip events for files
                                // matching a watch_dir's DirIgnoreConfig.
                                let real_events: Vec<_> = {
                                    let cfg = state.config.lock().ok();
                                    if let Some(ref cfg) = cfg {
                                        if cfg.dir_ignore.is_empty() {
                                            real_events
                                        } else {
                                            real_events.into_iter().filter(|e| {
                                                for (dir_str, dir_cfg) in &cfg.dir_ignore {
                                                    let dir_path = std::path::Path::new(dir_str);
                                                    if e.path.starts_with(dir_path) {
                                                        if rew_core::watcher::filter::PathFilter::should_ignore_by_dir_config(
                                                            &e.path, dir_path, dir_cfg,
                                                        ) {
                                                            return false;
                                                        }
                                                    }
                                                }
                                                true
                                            }).collect()
                                        }
                                    } else {
                                        real_events
                                    }
                                };

                                if is_rolling_back {
                                    info!("读档进行中 — 抑制 {} 个 FSEvent", real_events.len());
                                } else {
                                    // Determine routing: AI task or monitoring window?
                                    // Falls back to grace-period task ID so that
                                    // delayed FSEvents from Bash tool ops (e.g.
                                    // `echo > file`) still land in the AI task
                                    // rather than a new monitoring window.
                                    let (active_ai_task_id, fsevent_attribution) = {
                                        if let Ok(db) = state.db.lock() {
                                            resolve_fsevent_task_route(&db, 15)
                                        } else {
                                            (None, "monitoring")
                                        }
                                    };

                                    let now = chrono::Utc::now();

                                    // is_new_window tracks whether we just created a fresh
                                    // monitoring window vs reused an existing one.  Used later
                                    // to decide if an all-no-op batch should clean up the window.
                                    let mut is_new_window = false;

                                    let task_id: String = if let Some(ref ai_id) = active_ai_task_id {
                                        // ── AI task path ──────────────────────────────────────
                                        // Seal any open monitoring window so it doesn't absorb
                                        // events that belong to the AI task.
                                        {
                                            let mut wg = state.fs_window_task.lock().unwrap();
                                            if let Some(ref old) = *wg {
                                                if let Ok(db) = state.db.lock() {
                                                    let _ = db.update_task_completed_at(&old.task_id, now);
                                                    let objs = rew_home_dir().join("objects");
                                                    let _ = rew_core::reconcile::reconcile_task(&db, &old.task_id, &objs);
                                                    let _ = db.refresh_task_rollup_from_changes(&old.task_id);
                                                }
                                                info!("Sealed monitoring window {} (AI task started)", &old.task_id);
                                            }
                                            *wg = None;
                                        }
                                        ai_id.clone()
                                    } else {
                                        // ── Monitoring window path ────────────────────────────
                                        let window_secs = state.config
                                            .lock()
                                            .map(|c| c.monitoring_window_secs)
                                            .unwrap_or(600);

                                        // DB fallback: if in-memory window was lost (app lifecycle,
                                        // unknown clear path, etc.), recover from DB before creating
                                        // a brand-new window. Locks are acquired sequentially
                                        // (fs_window_task → release → db → release → fs_window_task)
                                        // to avoid holding two locks at once.
                                        {
                                            let need_recover = state.fs_window_task.lock()
                                                .map(|g| g.is_none())
                                                .unwrap_or(false);
                                            if need_recover {
                                                if let Ok(db) = state.db.lock() {
                                                    if let Ok(Some(latest)) = db.get_latest_monitoring_window() {
                                                        let age = (now - latest.started_at).num_seconds() as u64;
                                                        if age < window_secs {
                                                            if let Ok(mut wg) = state.fs_window_task.lock() {
                                                                if wg.is_none() {
                                                                    info!(
                                                                        "Window recovered from DB: {} (age={}s)",
                                                                        latest.id, age
                                                                    );
                                                                    *wg = Some(crate::state::FsWindowTask {
                                                                        task_id: latest.id,
                                                                        started_at: latest.started_at,
                                                                    });
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        let (task_id, old_to_seal) = {
                                            let mut window_guard = state.fs_window_task.lock().unwrap();
                                            let use_existing = window_guard.as_ref().map(|w| {
                                                let elapsed = (now - w.started_at).num_seconds() as u64;
                                                elapsed < window_secs
                                            }).unwrap_or(false);

                                            if use_existing {
                                                let existing_id = window_guard.as_ref().unwrap().task_id.clone();
                                                (existing_id, None)
                                            } else {
                                                is_new_window = true;
                                                let old_id = window_guard.as_ref().map(|w| w.task_id.clone());
                                                let new_id = format!("fs_{}", now.format("%m%d%H%M%S%3f"));
                                                info!("New monitoring window: {} (prev={:?})", new_id, old_id);
                                                *window_guard = Some(crate::state::FsWindowTask {
                                                    task_id: new_id.clone(),
                                                    started_at: now,
                                                });
                                                (new_id, old_id)
                                            }
                                        }; // window_guard released here — safe to lock db

                                        // Seal the expired window (if any) and always update
                                        // current window's completed_at so DB reflects last-event time.
                                        // This survives app crashes: on next startup we see a real timestamp.
                                        if let Ok(db) = state.db.lock() {
                                            if let Some(ref old_id) = old_to_seal {
                                                let _ = db.update_task_completed_at(old_id, now);
                                                let objs = rew_home_dir().join("objects");
                                                let _ = rew_core::reconcile::reconcile_task(&db, old_id, &objs);
                                                let _ = db.refresh_task_rollup_from_changes(old_id);
                                            }
                                            // Track last-event time for the current window
                                            let _ = db.update_task_completed_at(&task_id, now);
                                        }
                                        task_id
                                    };

                                    // Deduplication via is_change_already_recorded (in Phase 3
                                    // below) replaces the old .stop_suppress file filtering.

                                    // After all filtering, if nothing left to record, skip DB write.
                                    // Only clear the in-memory window if we JUST created it in
                                    // this batch (is_new_window). Reused windows must be kept
                                    // alive — they already have changes from earlier batches.
                                    if real_events.is_empty() {
                                        if active_ai_task_id.is_none() && is_new_window {
                                            if let Ok(mut wg) = state.fs_window_task.lock() {
                                                if wg.as_ref().map(|w| w.task_id == task_id).unwrap_or(false) {
                                                    *wg = None;
                                                }
                                            }
                                        }
                                    } else {
                                        let (effective_events, deleted_dir_summaries) = if let Ok(db) = state.db.lock() {
                                            expand_deleted_directory_events(&db, &real_events)
                                        } else {
                                            (real_events.clone(), Vec::new())
                                        };

                                        // If this is a monitoring window batch, ensure the Task row
                                        // exists. For AI task batches the row was created by the hook.
                                        if active_ai_task_id.is_none() {
                                            let real_added = effective_events.iter()
                                                .filter(|e| e.kind == FileEventKind::Created).count();
                                            let real_modified = effective_events.iter()
                                                .filter(|e| e.kind == FileEventKind::Modified).count();
                                            let real_deleted = effective_events.iter()
                                                .filter(|e| matches!(e.kind, FileEventKind::Deleted | FileEventKind::Renamed)).count();

                                            let mut parts = Vec::new();
                                            if real_added > 0 { parts.push(format!("{} 个文件新增", real_added)); }
                                            if real_modified > 0 { parts.push(format!("{} 个文件修改", real_modified)); }
                                            if real_deleted > 0 { parts.push(format!("{} 个文件删除", real_deleted)); }
                                            let summary = parts.join("，");

                                            let task = Task {
                                                id: task_id.clone(),
                                                prompt: Some(if is_anomaly {
                                                    format!("⚠️ 异常操作检测：{}", summary)
                                                } else {
                                                    format!("文件变更：{}", summary)
                                                }),
                                                tool: Some("文件监听".to_string()),
                                                started_at: now,
                                                completed_at: None,
                                                status: TaskStatus::Completed,
                                                risk_level: None,
                                                summary: Some(summary),
                                                cwd: None,
                                            };
                                            // Silently ignore duplicate-key — means we're in an existing window.
                                            if let Ok(db) = state.db.lock() {
                                                let _ = db.create_task(&task);
                                            }
                                        }

                                        if !deleted_dir_summaries.is_empty() {
                                            if let Ok(db) = state.db.lock() {
                                                for (dir_path, file_count) in &deleted_dir_summaries {
                                                    let _ = db.upsert_task_deleted_dir(&task_id, dir_path, *file_count);
                                                }
                                            }
                                        }

                                        let obj_store = ObjectStore::new(rew_home_dir().join("objects")).ok();

                                        // Phase 1: Store current content for existing files
                                        let mut file_hashes: std::collections::HashMap<std::path::PathBuf, String> =
                                            std::collections::HashMap::new();

                                        // For large audio/video files (>= 50 MB) only keep the
                                        // 2 most recent stored versions across all tasks.
                                        // Code/text files are excluded from this trim — their full
                                        // history is small and valuable.
                                        const LARGE_FILE_BYTES: u64 = 50 * 1024 * 1024;
                                        const LARGE_FILE_KEEP_VERSIONS: usize = 2;

                                        for event in effective_events.iter() {
                                            if event.path.exists() {
                                                if let Some(ref store) = obj_store {
                                                    if let Ok(hash) = store.store(&event.path) {
                                                        // Trim old cross-task versions for large audio/video files
                                                        let file_size = std::fs::metadata(&event.path)
                                                            .map(|m| m.len())
                                                            .unwrap_or(0);
                                                        if file_size >= LARGE_FILE_BYTES
                                                            && is_av_file(&event.path)
                                                        {
                                                            let old_hashes = if let Ok(db) = state.db.lock() {
                                                                db.get_old_version_hashes(
                                                                    &event.path,
                                                                    LARGE_FILE_KEEP_VERSIONS,
                                                                )
                                                                .unwrap_or_default()
                                                            } else {
                                                                Vec::new()
                                                            };
                                                            for old_hash in old_hashes {
                                                                let still_ref = if let Ok(db) = state.db.lock() {
                                                                    db.is_hash_referenced(&old_hash).unwrap_or(true)
                                                                } else {
                                                                    true
                                                                };
                                                                if !still_ref {
                                                                    let _ = store.delete(&old_hash);
                                                                }
                                                            }
                                                        }
                                                        file_hashes.insert(event.path.clone(), hash);
                                                    }
                                                }
                                            }
                                        }

                                        // Phase 2: Look up shadow/DB hashes for deleted files
                                        for event in effective_events.iter() {
                                            if !event.path.exists() && !file_hashes.contains_key(&event.path) {
                                                if let Some(h) = rew_core::pipeline::take_shadow_hash(&event.path) {
                                                    file_hashes.insert(event.path.clone(), h);
                                                } else if let Ok(db) = state.db.lock() {
                                                    if let Ok(Some(prev)) = db.get_latest_change_for_file(&event.path) {
                                                        if let Some(h) = prev.new_hash.or(prev.old_hash) {
                                                            file_hashes.insert(event.path.clone(), h);
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // Phase 3: Upsert Change records.
                                        // upsert_change preserves old_hash if a record already
                                        // exists for (task_id, file_path) — so AI hook's precise
                                        // old_hash (captured right before the write) wins when
                                        // both the hook and daemon write for the same file.
                                        let mut changes_written: usize = 0;
                                        for event in &effective_events {
                                            let baseline = if let Ok(db) = state.db.lock() {
                                                rew_core::baseline::resolve_baseline(
                                                    &db, &task_id, &event.path, None,
                                                )
                                            } else {
                                                rew_core::baseline::Baseline {
                                                    existed: false,
                                                    hash: None,
                                                }
                                            };

                                            let change_type = match event.kind {
                                                FileEventKind::Created => {
                                                    if baseline.existed {
                                                        ChangeType::Modified
                                                    } else {
                                                        ChangeType::Created
                                                    }
                                                }
                                                FileEventKind::Modified => {
                                                    if baseline.existed {
                                                        ChangeType::Modified
                                                    } else {
                                                        ChangeType::Created
                                                    }
                                                }
                                                FileEventKind::Deleted => ChangeType::Deleted,
                                                FileEventKind::Renamed => {
                                                    if event.path.exists() {
                                                        if baseline.existed {
                                                            ChangeType::Renamed
                                                        } else {
                                                            ChangeType::Created
                                                        }
                                                    } else {
                                                        ChangeType::Deleted
                                                    }
                                                }
                                            };

                                            let (old_hash, new_hash) = match change_type {
                                                ChangeType::Created => {
                                                    let h = file_hashes.get(&event.path).cloned();
                                                    (None, h)
                                                }
                                                ChangeType::Modified | ChangeType::Renamed => {
                                                    let new = file_hashes.get(&event.path).cloned();
                                                    (baseline.hash, new)
                                                }
                                                ChangeType::Deleted => {
                                                    if !baseline.existed {
                                                        // Ephemeral: created and deleted within this task.
                                                        // Will be cleaned as net-zero by reconcile.
                                                        (None, None)
                                                    } else {
                                                        let old = baseline.hash
                                                            .or_else(|| file_hashes.get(&event.path).cloned());
                                                        (old, None)
                                                    }
                                                }
                                            };

                                            // Skip no-op events (content identical)
                                            if old_hash.is_some()
                                                && new_hash.is_some()
                                                && old_hash == new_hash
                                            {
                                                continue;
                                            }

                                            if matches!(change_type, ChangeType::Created) && new_hash.is_none() {
                                                continue;
                                            }

                                            // Dedup: skip if hook already recorded this exact
                                            // change (same file + same new_hash) in any active or
                                            // recently completed task (within 90s).
                                            if let Some(ref nh) = new_hash {
                                                let p = event.path.to_string_lossy().to_string();
                                                let already_recorded = if let Ok(db) = state.db.lock() {
                                                    db.is_change_already_recorded(&p, nh).unwrap_or(false)
                                                } else {
                                                    false
                                                };
                                                if already_recorded {
                                                    continue;
                                                }
                                            }

                                            // Compute line-count stats on the fly.
                                            // We read from the object store (already in memory
                                            // as file_hashes); reads are cheap for text files.
                                            let (lines_added, lines_removed) = compute_line_stats(
                                                &obj_store,
                                                old_hash.as_deref(),
                                                new_hash.as_deref(),
                                            );

                                            let change = Change {
                                                id: None,
                                                task_id: task_id.clone(),
                                                file_path: event.path.clone(),
                                                change_type,
                                                old_hash,
                                                new_hash,
                                                diff_text: None,
                                                lines_added,
                                                lines_removed,
                                                attribution: Some(fsevent_attribution.to_string()),
                                                old_file_path: None,
                                            };

                                            // Before upserting, record which new_hash currently
                                            // occupies this (task_id, file_path) slot so we can
                                            // delete it from the ObjectStore if it becomes orphaned.
                                            let prev_new_hash = if let Ok(db) = state.db.lock() {
                                                db.get_change_new_hash(
                                                    &change.task_id,
                                                    &change.file_path,
                                                ).ok().flatten()
                                            } else {
                                                None
                                            };

                                            if let Ok(db) = state.db.lock() {
                                                if db.upsert_change(&change).is_ok() {
                                                    let seen_at = chrono::Utc::now().to_rfc3339();
                                                    let _ = sync_file_index_after_change(&db, &change, fsevent_attribution, &seen_at);
                                                    changes_written += 1;
                                                }
                                            }

                                            // Clean up orphaned object: the previous new_hash is
                                            // only deleted when the hash actually changed AND
                                            // no other change record references it.
                                            if let Some(ref prev_hash) = prev_new_hash {
                                                let new_hash_differs = change.new_hash
                                                    .as_deref()
                                                    .map(|h| h != prev_hash.as_str())
                                                    .unwrap_or(true);
                                                if new_hash_differs {
                                                    let still_referenced = if let Ok(db) = state.db.lock() {
                                                        db.is_hash_referenced(prev_hash).unwrap_or(true)
                                                    } else {
                                                        true
                                                    };
                                                    if !still_referenced {
                                                        if let Some(ref store) = obj_store {
                                                            let _ = store.delete(prev_hash);
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // Phase 4: Remove ghost task rows — ONLY for newly
                                        // created windows. A reused window (is_new_window=false)
                                        // already has changes from earlier batches; a no-op batch
                                        // (metadata touch / Spotlight) must not kill it.
                                        if active_ai_task_id.is_none() && is_new_window && changes_written == 0 {
                                            if let Ok(db) = state.db.lock() {
                                                let _ = db.delete_task(&task_id);
                                            }
                                            if let Ok(mut wg) = state.fs_window_task.lock() {
                                                if wg.as_ref().map(|w| w.task_id == task_id).unwrap_or(false) {
                                                    *wg = None;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let mut metadata = serde_json::json!({});

                        // Include anomaly information
                        if is_anomaly {
                            metadata["anomaly"] = serde_json::json!(true);
                            metadata["alerts"] = serde_json::json!(
                                alerts.iter().map(|a| {
                                    serde_json::json!({
                                        "severity": a.severity.to_string(),
                                        "kind": a.kind_str(),
                                        "description": &a.description,
                                    })
                                }).collect::<Vec<_>>()
                            );
                        }

                        // Include backup information
                        if let Some(backup_res) = &backup_result {
                            metadata["backup"] = serde_json::json!({
                                "files_backed_up": backup_res.files_backed_up,
                                "total_size_bytes": backup_res.total_size_bytes,
                                "failed_count": backup_res.failed_files.len(),
                            });
                        }

                        // === Create APFS snapshot with real tmutil ref ===
                        let os_snapshot_ref = if tmutil.is_available() {
                            match tmutil.create_snapshot() {
                                Ok(date) => {
                                    let ref_str = format!("com.apple.TimeMachine.{}.local", date);
                                    info!("APFS snapshot: {}", ref_str);
                                    ref_str
                                }
                                Err(e) => {
                                    warn!("tmutil snapshot failed (using fallback ref): {}", e);
                                    format!("rew-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
                                }
                            }
                        } else {
                            format!("rew-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
                        };

                        // Store APFS snapshot reference in the task rather than a
                        // separate snapshots table. The task row already tracks all
                        // file changes; os_snapshot_ref is the only extra piece.
                        let snapshot_id_str = snapshot_id.to_string();
                        // Write os_snapshot_ref to the most recently active task
                        if let Some(state) = app.try_state::<AppState>() {
                            if let Ok(db) = state.db.lock() {
                                if let Ok(Some(tid)) = db.get_most_recent_active_task_id() {
                                    match db.set_task_snapshot_ref(&tid, &os_snapshot_ref) {
                                        Ok(_) => {
                                            info!(
                                                "Task {} snapshot ref saved: {} (trigger={:?}, +{}/-{}/~{})",
                                                &tid[..8.min(tid.len())],
                                                &os_snapshot_ref,
                                                trigger,
                                                added, deleted, modified
                                            );
                                        }
                                        Err(e) => {
                                            error!("Failed to save snapshot ref on task: {}", e);
                                        }
                                    }
                                }
                            }
                        }

                        let _ = app.emit("task-updated", serde_json::json!({
                            "task_id": snapshot_id_str,
                            "trigger": trigger.to_string(),
                            "files_added": added,
                            "files_modified": modified,
                            "files_deleted": deleted,
                            "is_anomaly": is_anomaly,
                            "backup_files": backup_result.as_ref().map(|r| r.files_backed_up).unwrap_or(0),
                        }));

                        if is_anomaly {
                            on_anomaly_detected(
                                app,
                                &alerts[0].description,
                                Some(&snapshot_id_str),
                            );
                        }
                    }
                    None => {
                        info!("Pipeline channel closed, restarting...");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(2)) => {
                // Periodic check — just allows the loop to check pause state
            }
        }
    }

    Ok(())
}

/// Called when an anomaly is detected. Updates tray and emits event to frontend.
pub fn on_anomaly_detected(app: &AppHandle, description: &str, snapshot_id: Option<&str>) {
    info!("Anomaly detected: {}", description);

    // Update tray icon to warning
    tray::update_tray_status(app, TrayStatus::Warning);

    // Update app state
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut warning) = state.has_warning.lock() {
            *warning = true;
        }
    }

    // Emit event to frontend
    let _ = app.emit(
        "anomaly-detected",
        serde_json::json!({
            "description": description,
            "snapshot_id": snapshot_id,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }),
    );
}

/// Returns true if the file is an audio or video format.
/// Used together with a size threshold to decide whether to trim old cross-task
/// versions in the ObjectStore (keeping only the last N copies).
fn is_av_file(path: &std::path::Path) -> bool {
    const AV_EXTENSIONS: &[&str] = &[
        // Video
        "mp4", "mov", "avi", "mkv", "wmv", "flv", "webm", "m4v", "mpg", "mpeg",
        "3gp", "3g2", "ts", "mts", "m2ts", "vob", "ogv", "rm", "rmvb", "asf",
        "divx", "xvid", "hevc", "h264", "h265", "f4v", "mxf", "dv", "mpe",
        // Audio
        "mp3", "aac", "wav", "flac", "ogg", "wma", "m4a", "aiff", "aif",
        "opus", "ape", "wv", "mka", "ra", "amr", "ac3", "dts",
        // Raw / professional media
        "raw", "r3d", "braw", "ari", "dpx",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AV_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Returns true if this path is a temporary/system file that should be excluded
/// from the change timeline (e.g. macOS safe-save intermediates, editor temps).
/// Read `~/.rew/.stop_suppress` (written by `rew hook stop`) and return the
/// set of paths that should be suppressed in monitoring-window mode.
///
/// The file is always deleted after reading.  Returns an empty set if the
/// file is absent or its `expires_secs` timestamp is already in the past.
///
/// **IMPORTANT**: This must only be called in the monitoring-window code path.
/// AI-task event processing must NOT consult this set so that a new AI task
/// can still record changes on files touched by the previous task.

/// Compute `(lines_added, lines_removed)` by reading old/new content from the
/// object store and running `rew_core::diff::count_changed_lines`.
///
/// Returns `(0, 0)` when either hash is missing or content is binary/oversized.
fn compute_line_stats(
    obj_store: &Option<rew_core::objects::ObjectStore>,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
) -> (u32, u32) {
    let store = match obj_store {
        Some(s) => s,
        None => return (0, 0),
    };
    rew_core::diff::count_changed_lines_from_store(store, old_hash, new_hash)
}

/// On startup: restore the previous monitoring window from DB if still valid,
/// or clean up leftover NULL completed_at entries from an unclean shutdown.
///
/// Because `completed_at` is written to DB on every batch (even reused windows),
/// On startup: close any AI tasks that were left open because `rew hook stop`
/// never ran (e.g. the user pressed Ctrl-C or force-quit Claude Code).
///
/// Also removes the `.current_task` marker file — after a restart no AI session
/// can be running, so the marker would cause every subsequent file event to be
/// mis-attributed to the now-dead task.
pub fn recover_stale_ai_tasks_on_startup(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let Ok(db) = state.db.lock() else { return };
    let now = chrono::Utc::now();

    // Deactivate all sessions, stop guards, and pre-tool hashes.
    let _ = db.deactivate_all_sessions();
    let _ = db.delete_all_stop_guards();
    let _ = delete_all_pre_tool_hashes();

    // Mark any lingering Active AI tasks as Completed in the DB.
    match db.recover_stale_ai_tasks(now) {
        Ok(0) => {}
        Ok(n) => info!("Startup: closed {} stale AI task(s)", n),
        Err(e) => warn!("Startup: failed to recover stale AI tasks: {}", e),
    }
}

/// the persisted value is always accurate to within ~30 seconds. This means:
///
/// - App restarts within a valid window → resume the same window (no duplicate entry)
/// - App restarts after the window expired → do nothing (completed_at is already correct)
/// - Crash before first batch write → completed_at IS NULL → patch it to now (rare)
pub fn restore_monitoring_window_from_db(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };

    let window_secs = state.config
        .lock()
        .map(|c| c.monitoring_window_secs)
        .unwrap_or(600);

    let now = chrono::Utc::now();

    // Step 1: Read from DB (lock db, release before touching window_guard)
    let latest = {
        let Ok(db) = state.db.lock() else { return };
        // Fix any crash-before-first-batch NULLs
        let _ = db.seal_null_monitoring_windows(now);
        db.get_latest_monitoring_window().ok().flatten()
    };

    // Step 2: Decide — resume or nothing
    if let Some(ref window_task) = latest {
        let age = (now - window_task.started_at).num_seconds() as u64;
        if age < window_secs {
            // Still valid — restore to in-memory state so next batch reuses it
            if let Ok(mut wg) = state.fs_window_task.lock() {
                *wg = Some(crate::state::FsWindowTask {
                    task_id: window_task.id.clone(),
                    started_at: window_task.started_at,
                });
                info!(
                    "Restored monitoring window {} from DB (age={}s, window={}s)",
                    window_task.id, age, window_secs
                );
            }
            return;
        }
        // Window expired — completed_at already has the last-event timestamp from
        // the previous session's batch writes. Nothing to patch.
        info!(
            "Previous monitoring window {} expired (age={}s > {}s), will open fresh on next event",
            window_task.id, age, window_secs
        );
    }
}

/// Clear the warning state (e.g., after user acknowledges).
pub fn clear_warning(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut warning) = state.has_warning.lock() {
            *warning = false;
        }
    }

    let paused = app
        .try_state::<AppState>()
        .and_then(|s| s.paused.lock().ok().map(|p| *p))
        .unwrap_or(false);

    tray::update_tray_status(
        app,
        if paused {
            TrayStatus::Paused
        } else {
            TrayStatus::Normal
        },
    );
}
