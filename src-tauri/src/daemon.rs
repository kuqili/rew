//! Background daemon that runs the file watching pipeline and anomaly detection.
//!
//! Integrates the real rew-core Pipeline → RuleEngine → snapshot recording,
//! and emits events to the Tauri frontend for live timeline updates.
#![allow(dead_code)]

use crate::state::{self, AppState, DirScanStatus, DirStatus};
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

                        // === V2: Create Task + Changes so file monitoring events appear in GUI timeline ===
                        if total > 0 {
                            if let Some(state) = app.try_state::<AppState>() {
                                if let Ok(db) = state.db.lock() {
                                    let task_id = format!(
                                        "fs_{}",
                                        chrono::Utc::now().format("%m%d%H%M%S")
                                    );

                                    // Build a human-readable summary
                                    let mut parts = Vec::new();
                                    if added > 0 { parts.push(format!("{} 个文件新增", added)); }
                                    if modified > 0 { parts.push(format!("{} 个文件修改", modified)); }
                                    if deleted > 0 { parts.push(format!("{} 个文件删除", deleted)); }
                                    let summary = parts.join("，");

                                    let prompt = if is_anomaly {
                                        Some(format!("⚠️ 异常操作检测：{}", summary))
                                    } else {
                                        Some(format!("文件变更：{}", summary))
                                    };

                                    let task = Task {
                                        id: task_id.clone(),
                                        prompt,
                                        tool: Some("文件监听".to_string()),
                                        started_at: chrono::Utc::now(),
                                        completed_at: Some(chrono::Utc::now()),
                                        status: TaskStatus::Completed,
                                        risk_level: None,
                                        summary: Some(summary),
                                    };

                                    if let Err(e) = db.create_task(&task) {
                                        warn!("Failed to create task for file event: {}", e);
                                    } else {
                                        let _obj_store_shadow = ObjectStore::new(rew_home_dir().join("objects")).ok();

                                        // === Shadow mechanism ===
                                        // The pipeline's shadow layer already stored file contents
                                        // in objects when Created/Modified events arrived (file still existed).
                                        // Now we just need to look up the hashes.

                                        let obj_store = ObjectStore::new(rew_home_dir().join("objects")).ok();

                                        // Phase 1: For files that STILL exist, store current content
                                        // (shadow may have stored pre-modification version, this stores post-mod)
                                        let mut file_hashes: std::collections::HashMap<std::path::PathBuf, String> =
                                            std::collections::HashMap::new();

                                        for event in &event_batch.events {
                                            if event.path.exists() {
                                                if let Some(ref store) = obj_store {
                                                    if let Ok(hash) = store.store(&event.path) {
                                                        file_hashes.insert(event.path.clone(), hash);
                                                    }
                                                }
                                            }
                                        }

                                        // Phase 2: For files that DON'T exist (Deleted), look up
                                        // the shadow backup hash
                                        for event in &event_batch.events {
                                            if !event.path.exists() && !file_hashes.contains_key(&event.path) {
                                                // First: check shadow layer (pipeline backed it up at event time)
                                                if let Some(h) = rew_core::pipeline::read_shadow_hash(&event.path) {
                                                    file_hashes.insert(event.path.clone(), h);
                                                }
                                                // Second: check DB for previous change records
                                                else if let Ok(Some(prev)) = db.get_latest_change_for_file(&event.path) {
                                                    if let Some(h) = prev.new_hash.or(prev.old_hash) {
                                                        file_hashes.insert(event.path.clone(), h);
                                                    }
                                                }
                                            }
                                        }

                                        // Phase 3: Create Change records
                                        for event in &event_batch.events {
                                            let change_type = match event.kind {
                                                FileEventKind::Created => ChangeType::Created,
                                                FileEventKind::Modified => ChangeType::Modified,
                                                FileEventKind::Deleted => ChangeType::Deleted,
                                                FileEventKind::Renamed => ChangeType::Renamed,
                                            };

                                            // For Modified: the hash we stored IS the old version
                                            // (we stored it before the batch, so it's pre-modification)
                                            // For Deleted: check if we captured it during Modified phase
                                            // For Created: new_hash = stored hash
                                            let (old_hash, new_hash) = match event.kind {
                                                FileEventKind::Modified => {
                                                    // old_hash = what we stored (pre-mod version from backup engine)
                                                    // new_hash = current file content
                                                    let h = file_hashes.get(&event.path).cloned();
                                                    (h.clone(), h)
                                                }
                                                FileEventKind::Deleted => {
                                                    // old_hash = last known hash from DB or from backup
                                                    let old = file_hashes.get(&event.path).cloned().or_else(|| {
                                                        db.get_latest_change_for_file(&event.path)
                                                            .ok()
                                                            .flatten()
                                                            .and_then(|c| c.new_hash)
                                                    });
                                                    (old, None)
                                                }
                                                FileEventKind::Created => {
                                                    let h = file_hashes.get(&event.path).cloned();
                                                    (None, h)
                                                }
                                                FileEventKind::Renamed => {
                                                    // macOS trash = rename. Treat like Deleted for undo.
                                                    let old = file_hashes.get(&event.path).cloned().or_else(|| {
                                                        rew_core::pipeline::read_shadow_hash(&event.path)
                                                    }).or_else(|| {
                                                        db.get_latest_change_for_file(&event.path)
                                                            .ok()
                                                            .flatten()
                                                            .and_then(|c| c.new_hash)
                                                    });
                                                    (old, None)
                                                }
                                            };

                                            let change = Change {
                                                id: None,
                                                task_id: task_id.clone(),
                                                file_path: event.path.clone(),
                                                change_type,
                                                old_hash,
                                                new_hash,
                                                diff_text: None,
                                                lines_added: 0,
                                                lines_removed: 0,
                                            };

                                            let _ = db.insert_change(&change);
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
