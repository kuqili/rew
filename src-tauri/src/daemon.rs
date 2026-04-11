//! Background daemon that runs the file watching pipeline and anomaly detection.
//!
//! Integrates the real rew-core Pipeline → RuleEngine → snapshot recording,
//! and emits events to the Tauri frontend for live timeline updates.
#![allow(dead_code)]

use crate::state::{self, AppState, DirScanStatus, DirStatus, PipelineCmd};
use crate::tray::{self, TrayStatus};
use rew_core::backup::{BackupEngine, BackupJob};
use rew_core::config::RewConfig;
use rew_core::detector::RuleEngine;
use rew_core::objects::ObjectStore;
use rew_core::pipeline;
use rew_core::scanner::ProgressCallback;
use rew_core::snapshot::tmutil::TmutilWrapper;
use rew_core::traits::AnomalyDetector;
use rew_core::types::{
    Change, ChangeType, FileEventKind, Snapshot, SnapshotTrigger, Task, TaskStatus,
};
use rew_core::rew_home_dir;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{error, info, warn};

/// Start the background protection daemon.
/// Runs file watching, anomaly detection, and snapshot management.
pub fn start_daemon(app: &AppHandle) {
    // On startup: clean up stale AI tasks left behind by interrupted sessions,
    // then restore the previous monitoring window.
    recover_stale_ai_tasks_on_startup(app);
    restore_monitoring_window_from_db(app);

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

        let result = rew_core::scanner::full_scan(
            &scan_config.watch_dirs,
            &scan_patterns,
            &rew_home_dir(),
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
                            .collect();

                        if !real_events.is_empty() {
                            if let Some(state) = app.try_state::<AppState>() {
                                let is_rolling_back = state.rolling_back
                                    .lock()
                                    .map(|g| *g)
                                    .unwrap_or(false);

                                // Path-level suppression: filter out events for
                                // paths recently written by GUI rollback operations.
                                // TTL = 90 s; also cleans up expired entries.
                                //
                                // NOTE: `.stop_suppress` (paths from a recently-ended AI task)
                                // is intentionally NOT consumed here — it must only filter
                                // monitoring-window events, never AI-task events, so that a new
                                // AI task can still record deletions/modifications on files that
                                // were touched by the previous task.
                                let real_events: Vec<_> = {
                                    let now = std::time::Instant::now();
                                    if let Ok(mut sp) = state.suppressed_paths.lock() {
                                        sp.retain(|_, t| now.duration_since(*t).as_secs() < 90);
                                        real_events.into_iter()
                                            .filter(|e| !sp.contains_key(&e.path))
                                            .collect()
                                    } else {
                                        real_events
                                    }
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
                                    let active_ai_task_id = read_active_ai_task_id()
                                        .or_else(read_grace_task_id);

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
                                            }
                                            // Track last-event time for the current window
                                            let _ = db.update_task_completed_at(&task_id, now);
                                        }
                                        task_id
                                    };

                                    // Monitoring-window-only: filter paths from the most recently
                                    // ended AI task (written to .stop_suppress by `rew hook stop`).
                                    // Prevents delayed FSEvents already recorded in the AI task from
                                    // appearing as duplicate monitoring-window entries.
                                    //
                                    // AI-task events intentionally bypass this so a new AI task can
                                    // still record operations on files touched by the previous task.
                                    let real_events: Vec<_> = if active_ai_task_id.is_none() {
                                        let stop_set = take_stop_suppress_set();
                                        if stop_set.is_empty() {
                                            real_events
                                        } else {
                                            real_events.into_iter()
                                                .filter(|e| !stop_set.contains(&e.path))
                                                .collect()
                                        }
                                    } else {
                                        real_events
                                    };

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
                                    } else if let Ok(db) = state.db.lock() {
                                        // If this is a monitoring window batch, ensure the Task row
                                        // exists. For AI task batches the row was created by the hook.
                                        if active_ai_task_id.is_none() {
                                            let real_added = real_events.iter()
                                                .filter(|e| e.kind == FileEventKind::Created).count();
                                            let real_modified = real_events.iter()
                                                .filter(|e| e.kind == FileEventKind::Modified).count();
                                            let real_deleted = real_events.iter()
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
                                            let _ = db.create_task(&task);
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

                                        for event in real_events.iter() {
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
                                                            let old_hashes = db
                                                                .get_old_version_hashes(
                                                                    &event.path,
                                                                    LARGE_FILE_KEEP_VERSIONS,
                                                                )
                                                                .unwrap_or_default();
                                                            for old_hash in old_hashes {
                                                                let still_ref = db
                                                                    .is_hash_referenced(&old_hash)
                                                                    .unwrap_or(true);
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
                                        for event in real_events.iter() {
                                            if !event.path.exists() && !file_hashes.contains_key(&event.path) {
                                                if let Some(h) = rew_core::pipeline::read_shadow_hash(&event.path) {
                                                    file_hashes.insert(event.path.clone(), h);
                                                } else if let Ok(Some(prev)) = db.get_latest_change_for_file(&event.path) {
                                                    if let Some(h) = prev.new_hash.or(prev.old_hash) {
                                                        file_hashes.insert(event.path.clone(), h);
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
                                        for event in &real_events {
                                            let change_type = match event.kind {
                                                FileEventKind::Created => ChangeType::Created,
                                                FileEventKind::Modified => ChangeType::Modified,
                                                FileEventKind::Deleted => ChangeType::Deleted,
                                                // On macOS/APFS, `rm` triggers ModifyKind::Name
                                                // (atomic rename-then-delete) which notify reports
                                                // as Renamed.  If the file no longer exists, it
                                                // was actually deleted — reclassify accordingly.
                                                FileEventKind::Renamed => {
                                                    if event.path.exists() {
                                                        ChangeType::Renamed
                                                    } else {
                                                        ChangeType::Deleted
                                                    }
                                                }
                                            };

                                            let (old_hash, new_hash) = match event.kind {
                                                FileEventKind::Modified => {
                                                    let old = latest_hash_from_db(&db, &event.path)
                                                        .or_else(|| manifest_hash_for_path(&event.path));
                                                    let new = file_hashes.get(&event.path).cloned();
                                                    (old, new)
                                                }
                                                FileEventKind::Deleted => {
                                                    let old = file_hashes.get(&event.path).cloned()
                                                        .or_else(|| rew_core::pipeline::read_shadow_hash(&event.path))
                                                        .or_else(|| latest_hash_from_db(&db, &event.path))
                                                        .or_else(|| manifest_hash_for_path(&event.path));
                                                    (old, None)
                                                }
                                                FileEventKind::Created => {
                                                    let h = file_hashes.get(&event.path).cloned();
                                                    (None, h)
                                                }
                                                FileEventKind::Renamed => {
                                                    let old = file_hashes.get(&event.path).cloned()
                                                        .or_else(|| rew_core::pipeline::read_shadow_hash(&event.path))
                                                        .or_else(|| latest_hash_from_db(&db, &event.path))
                                                        .or_else(|| manifest_hash_for_path(&event.path));
                                                    (old, None)
                                                }
                                            };

                                            // Skip no-op events: file was opened/touched by the OS
                                            // (mtime refresh, xattr update, Spotlight indexing…) but
                                            // the actual byte content is identical. We verify by
                                            // comparing SHA-256 hashes — if both sides are Some and
                                            // equal, there is nothing meaningful to record.
                                            if old_hash.is_some()
                                                && new_hash.is_some()
                                                && old_hash == new_hash
                                            {
                                                continue;
                                            }

                                            // Also skip Created events where we have no content to
                                            // store (e.g. an empty temp file that disappeared before
                                            // we could hash it).
                                            if matches!(event.kind, FileEventKind::Created) && new_hash.is_none() {
                                                continue;
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
                                                restored_at: None,
                                            };

                                            // Before upserting, record which new_hash currently
                                            // occupies this (task_id, file_path) slot so we can
                                            // delete it from the ObjectStore if it becomes orphaned.
                                            let prev_new_hash = db.get_change_new_hash(
                                                &change.task_id,
                                                &change.file_path,
                                            ).ok().flatten();

                                            let _ = db.upsert_change(&change);
                                            changes_written += 1;

                                            // Clean up orphaned object: the previous new_hash is
                                            // only deleted when the hash actually changed AND
                                            // no other change record references it.
                                            if let Some(ref prev_hash) = prev_new_hash {
                                                let new_hash_differs = change.new_hash
                                                    .as_deref()
                                                    .map(|h| h != prev_hash.as_str())
                                                    .unwrap_or(true);
                                                if new_hash_differs {
                                                    let still_referenced = db
                                                        .is_hash_referenced(prev_hash)
                                                        .unwrap_or(true);
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
                                            let _ = db.delete_task(&task_id);
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

                        let snapshot = Snapshot {
                            id: snapshot_id,
                            timestamp: chrono::Utc::now(),
                            trigger: trigger.clone(),
                            os_snapshot_ref,
                            files_added: added as u32,
                            files_modified: modified as u32,
                            files_deleted: deleted as u32,
                            pinned: false,
                            metadata_json: if !metadata.is_null() && metadata.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                                Some(metadata.to_string())
                            } else {
                                None
                            },
                        };

                        // Save to database
                        if let Some(state) = app.try_state::<AppState>() {
                            if let Ok(db) = state.db.lock() {
                                match db.save_snapshot(&snapshot) {
                                    Ok(_) => {
                                        info!(
                                            "Snapshot saved: {} (trigger={:?}, +{}/-{}/~{})",
                                            &snapshot.id.to_string()[..8],
                                            trigger,
                                            added, deleted, modified
                                        );
                                    }
                                    Err(e) => {
                                        error!("Failed to save snapshot: {}", e);
                                    }
                                }
                            }
                        }

                        // Emit event to frontend so the timeline updates in real-time
                        let _ = app.emit("snapshot-created", serde_json::json!({
                            "id": snapshot.id.to_string(),
                            "timestamp": snapshot.timestamp.to_rfc3339(),
                            "trigger": trigger.to_string(),
                            "files_added": added,
                            "files_modified": modified,
                            "files_deleted": deleted,
                            "is_anomaly": is_anomaly,
                            "backup_files": backup_result.as_ref().map(|r| r.files_backed_up).unwrap_or(0),
                        }));

                        // If anomaly, also update tray and send anomaly event
                        if is_anomaly {
                            on_anomaly_detected(
                                app,
                                &alerts[0].description,
                                Some(&snapshot.id.to_string()),
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
fn take_stop_suppress_set() -> std::collections::HashSet<std::path::PathBuf> {
    let file = rew_home_dir().join(".stop_suppress");
    let content = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(_) => return std::collections::HashSet::new(),
    };
    // Always delete after reading — prevents double-consumption.
    let _ = std::fs::remove_file(&file);

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    if let Some(expires) = extract_json_i64(&content, "expires_secs") {
        if expires <= now_unix {
            return std::collections::HashSet::new(); // Already expired
        }
        extract_json_string_array(&content, "paths")
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect()
    } else {
        std::collections::HashSet::new()
    }
}

/// Extract a signed integer value from a flat JSON object string.
/// e.g. `extract_json_i64(r#"{"expires_secs":1234}"#, "expires_secs")` → Some(1234)
fn extract_json_i64(json: &str, key: &str) -> Option<i64> {
    let needle = format!("\"{}\":", key);
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Extract a JSON array of strings from a flat JSON object string.
fn extract_json_string_array(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{}\":[", key);
    let start = match json.find(&needle) {
        Some(p) => p + needle.len(),
        None => return vec![],
    };
    let array_str = &json[start..];
    let end = match array_str.find(']') {
        Some(p) => p,
        None => return vec![],
    };
    let inner = &array_str[..end];
    let mut results = Vec::new();
    let mut remaining = inner.trim();
    while let Some(q1) = remaining.find('"') {
        let after_q1 = &remaining[q1 + 1..];
        // Find closing quote, skip escaped quotes
        let mut pos = 0;
        let mut closed = false;
        while pos < after_q1.len() {
            match after_q1.as_bytes()[pos] {
                b'\\' => pos += 2, // skip escaped char
                b'"' => { closed = true; break; }
                _ => pos += 1,
            }
        }
        if closed {
            let raw = &after_q1[..pos];
            results.push(raw.replace("\\\"", "\"").replace("\\\\", "\\"));
            remaining = &after_q1[pos + 1..];
        } else {
            break;
        }
    }
    results
}

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

    let read_content = |hash: &str| -> Option<Vec<u8>> {
        let path = store.retrieve(hash)?;
        std::fs::read(path).ok()
    };

    let old_bytes = old_hash.and_then(read_content).unwrap_or_default();
    let new_bytes = new_hash.and_then(read_content).unwrap_or_default();

    rew_core::diff::count_changed_lines(&old_bytes, &new_bytes)
}

/// Read the latest known hash for a file from change history.
/// Prefers `new_hash`, then falls back to `old_hash`.
fn latest_hash_from_db(db: &rew_core::db::Database, path: &std::path::Path) -> Option<String> {
    db.get_latest_change_for_file(path)
        .ok()
        .flatten()
        .and_then(|c| c.new_hash.or(c.old_hash))
}

/// Fallback lookup from background scan manifest (`~/.rew/.scan_manifest.json`).
/// Used when a file has no prior change history in DB.
fn manifest_hash_for_path(path: &std::path::Path) -> Option<String> {
    let manifest_path = rew_core::rew_home_dir().join(".scan_manifest.json");
    let manifest_str = std::fs::read_to_string(manifest_path).ok()?;
    let manifest: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(&manifest_str).ok()?;
    manifest
        .get(&path.to_string_lossy().to_string())
        .and_then(|entry| entry.get("hash"))
        .and_then(|h| h.as_str())
        .map(|s| s.to_string())
}

/// Read the active AI task ID, supporting concurrent sessions from multiple
/// AI tools (Claude Code, Cursor, Codebuddy, …).
///
/// Priority order:
///   1. `~/.rew/sessions/` directory — each active session writes one file
///      named by its session ID, containing the task ID.  Multiple tools can
///      run simultaneously without overwriting each other.
///   2. Legacy `~/.rew/.current_task` — plain-text fallback for tools that
///      haven't adopted session-scoped files yet.
///
/// When multiple sessions are active the most recently modified session file
/// is used (highest chance of being the one that triggered the current batch
/// of FSEvents).  Stale session files (>2 h) are silently removed.
fn read_active_ai_task_id() -> Option<String> {
    let rew_dir = rew_home_dir();

    // ── 1. Session directory ──────────────────────────────────────────────
    let sessions_dir = rew_dir.join("sessions");
    if sessions_dir.exists() {
        let mut sessions: Vec<(std::time::SystemTime, String)> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                // Skip hidden/temp files (e.g. .grace_xxx)
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let Ok(modified) = meta.modified() else { continue };

                // Remove sessions older than 2 hours (stale crash leftovers)
                if modified.elapsed().map(|d| d.as_secs() > 7200).unwrap_or(false) {
                    let _ = std::fs::remove_file(entry.path());
                    continue;
                }

                if let Ok(task_id) = std::fs::read_to_string(entry.path()) {
                    let task_id = task_id.trim().to_string();
                    if !task_id.is_empty() {
                        sessions.push((modified, task_id));
                    }
                }
            }
        }

        if !sessions.is_empty() {
            // Pick the most-recently-active session
            sessions.sort_by(|a, b| b.0.cmp(&a.0));
            return Some(sessions.into_iter().next().unwrap().1);
        }
    }

    // ── 2. Legacy .current_task fallback ─────────────────────────────────
    let marker = rew_dir.join(".current_task");
    let content = std::fs::read_to_string(&marker).ok()?;
    let task_id = content.trim().to_string();
    if task_id.is_empty() {
        return None;
    }
    // Staleness check
    if let Ok(meta) = std::fs::metadata(&marker) {
        if let Ok(modified) = meta.modified() {
            if modified.elapsed().map(|d| d.as_secs() > 7200).unwrap_or(false) {
                let _ = std::fs::remove_file(&marker);
                return None;
            }
        }
    }
    Some(task_id)
}

/// Read the grace-period task ID written by `rew hook stop`.
///
/// After an AI session ends, `hook stop` writes `.grace_task` with the task ID
/// and a 15-second TTL.  With hook-level recording (afterFileEdit etc.) as the
/// primary capture path, this grace window only covers Cursor auto-save delays
/// and FSEvents latency (~5 s typical).
/// Read the most recently expired grace task ID from either the new multi-entry
/// `.grace_tasks` file (JSON array) or the legacy single-entry `.grace_task`.
///
/// When multiple concurrent AI sessions each write their own grace entry, the
/// most-recently-added unexpired entry is returned (last in array = most recent).
fn read_grace_task_id() -> Option<String> {
    let rew_dir = rew_home_dir();
    let now_ts = chrono::Utc::now().timestamp();

    // ── New multi-entry format ─────────────────────────────────────────────
    let multi_file = rew_dir.join(".grace_tasks");
    if let Ok(content) = std::fs::read_to_string(&multi_file) {
        if let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
            // Pick the most recently added entry that hasn't expired yet.
            // Entries are appended in chronological order, so iterate in reverse.
            for entry in entries.iter().rev() {
                let expires = entry["expires_secs"].as_i64().unwrap_or(0);
                if now_ts <= expires {
                    if let Some(tid) = entry["task_id"].as_str() {
                        return Some(tid.to_string());
                    }
                }
            }
        }
    }

    // ── Legacy single-entry format (backward-compat) ───────────────────────
    let legacy_file = rew_dir.join(".grace_task");
    if let Ok(content) = std::fs::read_to_string(&legacy_file) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            let expires = json["expires_secs"].as_i64().unwrap_or(0);
            if now_ts <= expires {
                return json["task_id"].as_str().map(|s| s.to_string());
            }
            let _ = std::fs::remove_file(&legacy_file);
        }
    }

    None
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
    // Remove the .current_task marker unconditionally.
    let marker = rew_home_dir().join(".current_task");
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
        info!("Startup: removed stale .current_task marker");
    }
    // Also remove .current_session, grace files, and all session files.
    let rew_dir = rew_home_dir();
    let _ = std::fs::remove_file(rew_dir.join(".current_session"));
    let _ = std::fs::remove_file(rew_dir.join(".grace_task"));   // legacy
    let _ = std::fs::remove_file(rew_dir.join(".grace_tasks"));  // new multi-entry
    // Clear the sessions/ directory — on app restart no AI session can
    // still be running, so all session files are guaranteed stale.
    let sessions_dir = rew_dir.join("sessions");
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }

    // Mark any lingering Active AI tasks as Completed in the DB.
    let Some(state) = app.try_state::<AppState>() else { return };
    let Ok(db) = state.db.lock() else { return };
    let now = chrono::Utc::now();
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
