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
    // On startup: restore previous monitoring window from DB (or clean up stale NULLs)
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

        let result = rew_core::scanner::full_scan(
            &scan_config.watch_dirs,
            &scan_config.ignore_patterns,
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
        // Fall back to polling loop — wait for config changes
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            // Check if paused
            let paused = app
                .try_state::<AppState>()
                .and_then(|s| s.paused.lock().ok().map(|p| *p))
                .unwrap_or(false);
            if paused {
                continue;
            }
        }
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
                            .filter(|e| !is_temp_path(&e.path))
                            .collect();

                        if !real_events.is_empty() {
                            if let Some(state) = app.try_state::<AppState>() {
                                let is_rolling_back = state.rolling_back
                                    .lock()
                                    .map(|g| *g)
                                    .unwrap_or(false);

                                // Path-level suppression: filter out events for
                                // paths recently written by GUI operations.
                                // TTL = 60 s; also cleans up expired entries.
                                let real_events: Vec<_> = {
                                    let now = std::time::Instant::now();
                                    if let Ok(mut sp) = state.suppressed_paths.lock() {
                                        sp.retain(|_, t| now.duration_since(*t).as_secs() < 60);
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
                                    let active_ai_task_id = read_active_ai_task_id();

                                    let now = chrono::Utc::now();

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
                                            *wg = None; // clear so next non-AI batch opens a fresh window
                                        }
                                        ai_id.clone()
                                    } else {
                                        // ── Monitoring window path ────────────────────────────
                                        let window_secs = state.config
                                            .lock()
                                            .map(|c| c.monitoring_window_secs)
                                            .unwrap_or(600);

                                        // Collect (old_task_id_to_seal, new_task_id) before locking DB
                                        let (task_id, old_to_seal) = {
                                            let mut window_guard = state.fs_window_task.lock().unwrap();
                                            let use_existing = window_guard.as_ref().map(|w| {
                                                let elapsed = (now - w.started_at).num_seconds() as u64;
                                                elapsed < window_secs
                                            }).unwrap_or(false);

                                            if use_existing {
                                                // Reuse: update in-memory last-event time for display
                                                let existing_id = window_guard.as_ref().unwrap().task_id.clone();
                                                (existing_id, None)
                                            } else {
                                                // Expire: record old window id for sealing, open new one
                                                let old_id = window_guard.as_ref().map(|w| w.task_id.clone());
                                                let new_id = format!("fs_{}", now.format("%m%d%H%M%S%3f"));
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

                                    if let Ok(db) = state.db.lock() {
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
                                            };
                                            // Silently ignore duplicate-key — means we're in an existing window.
                                            let _ = db.create_task(&task);
                                        }

                                        let obj_store = ObjectStore::new(rew_home_dir().join("objects")).ok();

                                        // Phase 1: Store current content for existing files
                                        let mut file_hashes: std::collections::HashMap<std::path::PathBuf, String> =
                                            std::collections::HashMap::new();

                                        for event in real_events.iter() {
                                            if event.path.exists() {
                                                if let Some(ref store) = obj_store {
                                                    if let Ok(hash) = store.store(&event.path) {
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
                                        for event in &real_events {
                                            let change_type = match event.kind {
                                                FileEventKind::Created => ChangeType::Created,
                                                FileEventKind::Modified => ChangeType::Modified,
                                                FileEventKind::Deleted => ChangeType::Deleted,
                                                FileEventKind::Renamed => ChangeType::Renamed,
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

                                            let _ = db.upsert_change(&change);
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

/// Returns true if this path is a temporary/system file that should be excluded
/// from the change timeline (e.g. macOS safe-save intermediates, editor temps).
fn is_temp_path(path: &std::path::Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    // macOS safe-save: "original.sb-XXXXXXXX-YYYYYY"
    if name.contains(".sb-") {
        return true;
    }
    if name.ends_with(".tmp") || name.ends_with(".temp") {
        return true;
    }
    if name.starts_with(".#") {
        return true;
    }
    false
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

/// Read the active AI task ID from the `.current_task` marker file.
///
/// Returns `Some(task_id)` while an AI tool session is open,
/// `None` when no AI task is running.
///
/// The marker is written by `rew hook prompt` and removed by `rew hook stop`.
/// It is also treated as stale if it is older than 2 hours (covers the case
/// where `hook stop` was never called because the AI session crashed).
fn read_active_ai_task_id() -> Option<String> {
    let marker = rew_home_dir().join(".current_task");

    let content = std::fs::read_to_string(&marker).ok()?;
    let task_id = content.trim().to_string();
    if task_id.is_empty() {
        return None;
    }

    // Staleness check: if the marker is older than 2 hours, ignore it.
    if let Ok(meta) = std::fs::metadata(&marker) {
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = modified.elapsed() {
                if age.as_secs() > 7200 {
                    // Stale — clean it up silently
                    let _ = std::fs::remove_file(&marker);
                    return None;
                }
            }
        }
    }

    Some(task_id)
}

/// On startup: restore the previous monitoring window from DB if still valid,
/// or clean up leftover NULL completed_at entries from an unclean shutdown.
///
/// Because `completed_at` is written to DB on every batch (even reused windows),
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
