//! Tauri IPC commands exposing rew-core functionality to the frontend.

use crate::state::AppState;
use rew_core::restore::TaskRestoreEngine;
use rew_core::types::{Snapshot, SnapshotTrigger};
use rew_core::rew_home_dir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

/// Snapshot data sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub timestamp: String,
    pub trigger: String,
    pub files_added: u32,
    pub files_modified: u32,
    pub files_deleted: u32,
    pub pinned: bool,
    pub is_anomaly: bool,
    pub metadata_json: Option<String>,
}

impl From<Snapshot> for SnapshotInfo {
    fn from(s: Snapshot) -> Self {
        SnapshotInfo {
            id: s.id.to_string(),
            timestamp: s.timestamp.to_rfc3339(),
            trigger: s.trigger.to_string(),
            files_added: s.files_added,
            files_modified: s.files_modified,
            files_deleted: s.files_deleted,
            pinned: s.pinned,
            is_anomaly: s.trigger == SnapshotTrigger::Anomaly,
            metadata_json: s.metadata_json,
        }
    }
}

/// Status information for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInfo {
    pub running: bool,
    pub paused: bool,
    pub watch_dirs: Vec<String>,
    pub total_snapshots: usize,
    pub anomaly_count: usize,
    pub has_warning: bool,
}

/// Restore preview information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorePreviewInfo {
    pub snapshot_id: String,
    pub files_to_restore: usize,
    pub files_to_overwrite: usize,
    pub files_to_remove: usize,
    pub estimated_size_bytes: u64,
}

/// Config data for frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigInfo {
    pub watch_dirs: Vec<String>,
    pub ignore_patterns: Vec<String>,
}

#[tauri::command]
pub async fn list_snapshots(state: State<'_, AppState>) -> Result<Vec<SnapshotInfo>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let snapshots = db.list_snapshots().map_err(|e| e.to_string())?;
    Ok(snapshots.into_iter().map(SnapshotInfo::from).collect())
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let snapshots = db.list_snapshots().map_err(|e| e.to_string())?;
    let anomaly_count = snapshots
        .iter()
        .filter(|s| s.trigger == SnapshotTrigger::Anomaly)
        .count();

    let paused = state.paused.lock().map_err(|e| e.to_string())?.clone();
    let has_warning = state.has_warning.lock().map_err(|e| e.to_string())?.clone();

    let config = state.config.lock().map_err(|e| e.to_string())?;
    let watch_dirs: Vec<String> = config
        .watch_dirs
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    Ok(StatusInfo {
        running: !paused,
        paused,
        watch_dirs,
        total_snapshots: snapshots.len(),
        anomaly_count,
        has_warning,
    })
}

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<ConfigInfo, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    Ok(ConfigInfo {
        watch_dirs: config
            .watch_dirs
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        ignore_patterns: config.ignore_patterns.clone(),
    })
}

#[tauri::command]
pub async fn update_config(
    state: State<'_, AppState>,
    watch_dirs: Vec<String>,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.watch_dirs = watch_dirs.into_iter().map(PathBuf::from).collect();
    let config_path = rew_home_dir().join("config.toml");
    config.save(&config_path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn toggle_pin(state: State<'_, AppState>, snapshot_id: String) -> Result<bool, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let uuid = uuid::Uuid::parse_str(&snapshot_id).map_err(|e| e.to_string())?;
    let snapshot = db
        .get_snapshot(&uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Snapshot not found".to_string())?;
    let new_pinned = !snapshot.pinned;
    db.set_pinned(&uuid, new_pinned)
        .map_err(|e| e.to_string())?;
    Ok(new_pinned)
}

/// Restore files from a backup directory to their original locations.
fn restore_files_from_backup(backup_dir: &Path) -> Result<usize, String> {
    if !backup_dir.exists() {
        return Err("Backup directory not found".to_string());
    }

    let mut restored_count = 0;

    // Walk through all files in the backup directory
    for entry_result in std::fs::read_dir(backup_dir)
        .map_err(|e| format!("Failed to read backup dir: {}", e))?
    {
        let entry = entry_result
            .map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        if path.is_file() {
            // Reconstruct the original path from the backup structure
            if let Ok(rel_path) = path.strip_prefix(backup_dir) {
                let target_path = PathBuf::from("/").join(rel_path);

                // Create parent directories if needed
                if let Some(parent) = target_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent dir {}: {}", parent.display(), e))?;
                }

                // Copy file to original location
                std::fs::copy(&path, &target_path)
                    .map_err(|e| format!("Failed to restore {}: {}", target_path.display(), e))?;

                restored_count += 1;
            }
        } else if path.is_dir() {
            // Recursively restore from subdirectories
            restored_count += restore_files_from_backup(&path)?;
        }
    }

    Ok(restored_count)
}

/// Calculate restore preview by walking backup directory.
fn calculate_restore_preview(backup_dir: &Path) -> Result<(usize, u64), String> {
    if !backup_dir.exists() {
        return Ok((0, 0));
    }

    let mut file_count = 0;
    let mut total_size = 0u64;

    let mut dirs_to_process = vec![backup_dir.to_path_buf()];

    while let Some(dir) = dirs_to_process.pop() {
        for entry_result in std::fs::read_dir(&dir)
            .map_err(|e| format!("Failed to read dir: {}", e))?
        {
            let entry = entry_result
                .map_err(|e| format!("Failed to read entry: {}", e))?;
            let path = entry.path();

            if path.is_file() {
                if let Ok(metadata) = entry.metadata() {
                    total_size += metadata.len();
                    file_count += 1;
                }
            } else if path.is_dir() {
                dirs_to_process.push(path);
            }
        }
    }

    Ok((file_count, total_size))
}

#[tauri::command]
pub async fn restore_snapshot(
    state: State<'_, AppState>,
    snapshot_id: String,
) -> Result<String, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let uuid = uuid::Uuid::parse_str(&snapshot_id).map_err(|e| e.to_string())?;
    let _snapshot = db
        .get_snapshot(&uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Snapshot not found".to_string())?;

    // Get the backup directory for this snapshot
    let backup_root = rew_home_dir().join("backups");
    let backup_dir = backup_root.join(&snapshot_id);

    if !backup_dir.exists() {
        return Err(format!("No backup found for snapshot {}", snapshot_id));
    }

    // Perform the restore
    match restore_files_from_backup(&backup_dir) {
        Ok(count) => {
            Ok(format!(
                "Restore successful! {} files restored from snapshot {}",
                count, &snapshot_id[..8.min(snapshot_id.len())]
            ))
        }
        Err(e) => Err(format!("Restore failed: {}", e)),
    }
}

#[tauri::command]
pub async fn get_restore_preview(
    state: State<'_, AppState>,
    snapshot_id: String,
) -> Result<RestorePreviewInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let uuid = uuid::Uuid::parse_str(&snapshot_id).map_err(|e| e.to_string())?;
    let snapshot = db
        .get_snapshot(&uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Snapshot not found".to_string())?;

    // Get backup directory and calculate actual restore statistics
    let backup_root = rew_home_dir().join("backups");
    let backup_dir = backup_root.join(&snapshot_id);

    let (files_to_restore, estimated_size_bytes) = if backup_dir.exists() {
        calculate_restore_preview(&backup_dir).unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    Ok(RestorePreviewInfo {
        snapshot_id: snapshot.id.to_string(),
        files_to_restore,
        files_to_overwrite: snapshot.files_modified as usize,
        files_to_remove: snapshot.files_deleted as usize,
        estimated_size_bytes,
    })
}

#[tauri::command]
pub async fn check_first_run() -> Result<bool, String> {
    let _config_path = rew_home_dir().join("config.toml");
    let first_run_marker = rew_home_dir().join(".setup_done");
    Ok(!first_run_marker.exists())
}

#[tauri::command]
pub async fn get_home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "无法获取 home 目录".to_string())
}

#[tauri::command]
pub async fn complete_setup(
    app: AppHandle,
    state: State<'_, AppState>,
    watch_dirs: Vec<String>,
) -> Result<(), String> {
    let dir_paths: Vec<PathBuf> = watch_dirs.iter().map(PathBuf::from).collect();

    // Save config with selected dirs
    {
        let mut config = state.config.lock().map_err(|e| e.to_string())?;
        config.watch_dirs = dir_paths.clone();
        let config_path = rew_home_dir().join("config.toml");
        config.save(&config_path).map_err(|e| e.to_string())?;
    }

    // Mark setup as done
    let marker = rew_home_dir().join(".setup_done");
    std::fs::write(&marker, "done").map_err(|e| e.to_string())?;

    // Populate scan_progress.dirs so the sidebar shows directories immediately
    {
        let mut progress = state.scan_progress.lock().map_err(|e| e.to_string())?;
        for dir in &watch_dirs {
            if !progress.dirs.iter().any(|d| &d.path == dir) {
                progress.dirs.push(crate::state::DirScanStatus {
                    path: dir.clone(),
                    status: crate::state::DirStatus::Pending,
                    files_total: 0,
                    files_done: 0,
                    files_stored: 0,
                    files_skipped: 0,
                    files_failed: 0,
                    last_completed_at: None,
                });
            }
        }
        progress.is_scanning = true;
        crate::state::save_scan_status(&progress);
        let _ = app.emit("scan-progress", &*progress);
    }

    // Kick off background scan for each directory
    let config = state.config.lock().map_err(|e| e.to_string())?.clone();
    for dir_path in &watch_dirs {
        let sp = state.scan_progress.clone();
        let dir_str = dir_path.clone();
        let scan_config = config.clone();
        let app_scan = app.clone();

        std::thread::spawn(move || {
            let path = PathBuf::from(&dir_str);
            let sp2 = sp.clone();
            let app_cb = app_scan.clone();
            let dir_str2 = dir_str.clone();
            let callback: rew_core::scanner::ProgressCallback = Box::new(move |update| {
                if let Ok(mut progress) = sp2.lock() {
                    if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == update.dir) {
                        if ds.status != crate::state::DirStatus::Complete {
                            ds.status = crate::state::DirStatus::Scanning;
                        }
                        ds.files_total = update.files_total_estimate;
                        ds.files_done = update.files_scanned;
                        ds.files_stored = update.files_stored;
                        ds.files_skipped = update.files_skipped;
                        ds.files_failed = update.files_failed;
                    }
                    let _ = app_cb.emit("scan-progress", &*progress);
                }
            });

            let mut patterns = scan_config.ignore_patterns.clone();
            let dir_key = path.display().to_string();
            if let Some(dir_cfg) = scan_config.dir_ignore.get(&dir_key) {
                for d in &dir_cfg.exclude_dirs {
                    patterns.push(format!("**/{d}/**"));
                }
                for ext in &dir_cfg.exclude_extensions {
                    patterns.push(format!("**/*.{ext}"));
                }
            }

            let result = rew_core::scanner::full_scan(
                &[path],
                &patterns,
                &rew_home_dir(),
                Some(callback),
                None,
                scan_config.max_file_size_bytes,
            );

            if let Ok(mut progress) = sp.lock() {
                if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == dir_str2) {
                    ds.status = crate::state::DirStatus::Complete;
                    ds.files_done = result.files_scanned;
                    ds.files_stored = result.files_stored;
                    ds.files_skipped = result.files_skipped;
                    ds.files_failed = result.files_failed;
                    if ds.files_total < result.files_scanned {
                        ds.files_total = result.files_scanned;
                    }
                    ds.last_completed_at = Some(chrono::Utc::now().to_rfc3339());
                }
                // Check if all dirs are complete
                let all_done = progress.dirs.iter().all(|d| d.status == crate::state::DirStatus::Complete);
                if all_done {
                    progress.is_scanning = false;
                }
                crate::state::save_scan_status(&progress);
                let _ = app_scan.emit("scan-progress", &*progress);
                if all_done {
                    let _ = app_scan.emit("scan-complete", "setup");
                }
            }
        });
    }

    // Hot-add directories to running FSEvents pipeline
    if let Ok(tx_guard) = state.pipeline_tx.lock() {
        if let Some(ref tx) = *tx_guard {
            for path in &dir_paths {
                let _ = tx.send(crate::state::PipelineCmd::AddPath(path.clone()));
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn set_paused(state: State<'_, AppState>, paused: bool) -> Result<(), String> {
    let mut p = state.paused.lock().map_err(|e| e.to_string())?;
    *p = paused;
    Ok(())
}

// ================================================================
// Task IPC commands (V2)
// ================================================================

/// Task data sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub prompt: Option<String>,
    pub tool: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub risk_level: Option<String>,
    pub summary: Option<String>,
    pub changes_count: usize,
    pub cwd: Option<String>,
}

/// Change data sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeInfo {
    pub id: Option<i64>,
    pub task_id: String,
    pub file_path: String,
    pub change_type: String,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub diff_text: Option<String>,
    pub lines_added: u32,
    pub lines_removed: u32,
    /// ISO-8601 timestamp set when this file was individually restored. None = not restored.
    pub restored_at: Option<String>,
}

/// Undo preview info for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoPreviewInfo {
    pub task_id: String,
    pub total_changes: usize,
    pub files_to_restore: Vec<String>,
    pub files_to_delete: Vec<String>,
}

/// Undo result info for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoResultInfo {
    pub files_restored: usize,
    pub files_deleted: usize,
    pub failures: Vec<(String, String)>,
}

#[tauri::command]
pub async fn list_tasks(
    state: State<'_, AppState>,
    dir_filter: Option<String>,
) -> Result<Vec<TaskInfo>, String> {
    // Exclude the currently-accumulating monitoring window: the daemon calls
    // update_task_completed_at on every event, so completed_at is always set
    // even for in-progress windows. The only reliable way to hide it is to
    // compare against the live FsWindowTask id stored in AppState.
    let active_window_id: Option<String> = state
        .fs_window_task
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|w| w.task_id.clone()));

    let db = state.db.lock().map_err(|e| format!("DB lock error: {}", e))?;

    // Determine if dir_filter is a file or directory path
    let is_file_filter = dir_filter.as_ref()
        .map(|p| !Path::new(p).is_dir())
        .unwrap_or(false);

    let mut tasks = if let Some(ref path) = dir_filter {
        if is_file_filter {
            db.list_tasks_by_file(path).map_err(|e| format!("list_tasks_by_file error: {}", e))?
        } else {
            db.list_tasks_by_dir(path).map_err(|e| format!("list_tasks_by_dir error: {}", e))?
        }
    } else {
        db.list_tasks().map_err(|e| format!("list_tasks error: {}", e))?
    };

    // Remove the active window so the frontend never sees an in-progress entry
    if let Some(ref id) = active_window_id {
        tasks.retain(|t| &t.id != id);
    }

    tracing::info!("list_tasks: found {} tasks in DB (dir_filter={:?})", tasks.len(), dir_filter);

    let mut result = Vec::new();
    for task in tasks {
        let changes_count = if let Some(ref path) = dir_filter {
            if is_file_filter {
                db.count_changes_for_file(&task.id, path).unwrap_or(0)
            } else {
                db.count_changes_in_dir(&task.id, path).unwrap_or(0)
            }
        } else {
            db.get_changes_for_task(&task.id)
                .map(|c| c.len())
                .unwrap_or(0)
        };

        result.push(TaskInfo {
            id: task.id,
            prompt: task.prompt,
            tool: task.tool,
            started_at: task.started_at.to_rfc3339(),
            completed_at: task.completed_at.map(|t| t.to_rfc3339()),
            status: task.status.to_string(),
            risk_level: task.risk_level.map(|r| r.to_string()),
            summary: task.summary,
            changes_count,
            cwd: task.cwd,
        });
    }

    tracing::info!("list_tasks: returning {} tasks to frontend", result.len());
    Ok(result)
}

#[tauri::command]
pub async fn get_task(state: State<'_, AppState>, task_id: String) -> Result<TaskInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task = db.get_task(&task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task '{}' not found", task_id))?;

    let changes_count = db.get_changes_for_task(&task.id)
        .map(|c| c.len())
        .unwrap_or(0);

    Ok(TaskInfo {
        id: task.id,
        prompt: task.prompt,
        tool: task.tool,
        started_at: task.started_at.to_rfc3339(),
        completed_at: task.completed_at.map(|t| t.to_rfc3339()),
        status: task.status.to_string(),
        risk_level: task.risk_level.map(|r| r.to_string()),
        summary: task.summary,
        changes_count,
        cwd: task.cwd,
    })
}

#[tauri::command]
pub async fn get_task_changes(
    state: State<'_, AppState>,
    task_id: String,
    dir_filter: Option<String>,
) -> Result<Vec<ChangeInfo>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let is_file_filter = dir_filter.as_ref()
        .map(|p| !Path::new(p).is_dir())
        .unwrap_or(false);
    let changes = if let Some(ref path) = dir_filter {
        if is_file_filter {
            db.get_changes_for_task_by_file(&task_id, path).map_err(|e| e.to_string())?
        } else {
            db.get_changes_for_task_in_dir(&task_id, path).map_err(|e| e.to_string())?
        }
    } else {
        db.get_changes_for_task(&task_id).map_err(|e| e.to_string())?
    };

    Ok(changes.into_iter().map(|c| ChangeInfo {
        id: c.id,
        task_id: c.task_id,
        file_path: c.file_path.to_string_lossy().to_string(),
        change_type: c.change_type.to_string(),
        old_hash: c.old_hash,
        new_hash: c.new_hash,
        diff_text: c.diff_text,
        lines_added: c.lines_added,
        lines_removed: c.lines_removed,
        restored_at: c.restored_at.map(|t| t.to_rfc3339()),
    }).collect())
}

// ----------------------------------------------------------------
// On-demand diff computation
// ----------------------------------------------------------------

/// Response from `get_change_diff`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeDiffResult {
    /// Unified diff text ready for display, or None for binary files.
    pub diff_text: Option<String>,
    /// Re-computed lines added (may update stale DB value).
    pub lines_added: u32,
    /// Re-computed lines removed.
    pub lines_removed: u32,
}

#[tauri::command]
pub async fn get_change_diff(
    state: State<'_, AppState>,
    task_id: String,
    file_path: String,
) -> Result<ChangeDiffResult, String> {
    let (old_hash, new_hash) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let changes = db.get_changes_for_task(&task_id).map_err(|e| e.to_string())?;
        let change = changes
            .into_iter()
            .find(|c| c.file_path.to_string_lossy() == file_path.as_str())
            .ok_or_else(|| format!("No change record found for {}", file_path))?;
        (change.old_hash, change.new_hash)
    };

    let objects_root = rew_home_dir().join("objects");
    let obj_store = rew_core::objects::ObjectStore::new(objects_root)
        .map_err(|e| e.to_string())?;

    let read_content = |hash: Option<&str>| -> Vec<u8> {
        hash.and_then(|h| obj_store.retrieve(h))
            .and_then(|p| std::fs::read(p).ok())
            .unwrap_or_default()
    };

    let old_bytes = read_content(old_hash.as_deref());
    let new_bytes = read_content(new_hash.as_deref());

    // Short display name: just the filename for the diff header
    let name = std::path::Path::new(&file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.clone());

    let result = rew_core::diff::compute_diff(
        &old_bytes,
        &new_bytes,
        &format!("a/{}", name),
        &format!("b/{}", name),
    );

    Ok(match result {
        Some(dr) => ChangeDiffResult {
            diff_text: Some(dr.text),
            lines_added: dr.lines_added,
            lines_removed: dr.lines_removed,
        },
        None => ChangeDiffResult {
            diff_text: None, // binary file
            lines_added: 0,
            lines_removed: 0,
        },
    })
}

// ----------------------------------------------------------------
// Rollback commands (canonical V3 names)
// ----------------------------------------------------------------

#[tauri::command]
pub async fn preview_rollback(state: State<'_, AppState>, task_id: String) -> Result<UndoPreviewInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let objects_root = rew_home_dir().join("objects");
    let engine = TaskRestoreEngine::new(objects_root);

    let preview = engine.preview_undo(&db, &task_id).map_err(|e| e.to_string())?;

    Ok(UndoPreviewInfo {
        task_id: preview.task_id,
        total_changes: preview.total_changes,
        files_to_restore: preview.files_to_restore.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        files_to_delete: preview.files_to_delete.iter().map(|p| p.to_string_lossy().to_string()).collect(),
    })
}

#[tauri::command]
pub async fn rollback_task_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<UndoResultInfo, String> {
    if let Ok(mut guard) = state.rolling_back.lock() {
        *guard = true;
    }

    let result = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let objects_root = rew_home_dir().join("objects");
        let engine = TaskRestoreEngine::new(objects_root);
        engine.undo_task(&db, &task_id).map_err(|e| e.to_string())
    };

    // On success: add all affected paths to suppressed_paths (60s TTL)
    if result.is_ok() {
        let now_instant = std::time::Instant::now();
        if let Ok(mut sp) = state.suppressed_paths.lock() {
            if let Ok(db) = state.db.lock() {
                if let Ok(changes) = db.get_changes_for_task(&task_id) {
                    for c in changes {
                        sp.insert(c.file_path, now_instant);
                    }
                }
            }
        }
    }

    // Delay-clear global rolling_back flag
    let app_handle = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        if let Some(s) = app_handle.try_state::<AppState>() {
            if let Ok(mut g) = s.rolling_back.lock() {
                *g = false;
            }
        }
    });

    let res = result?;
    Ok(UndoResultInfo {
        files_restored: res.files_restored,
        files_deleted: res.files_deleted,
        failures: res.failures.iter().map(|(p, e)| (p.to_string_lossy().to_string(), e.clone())).collect(),
    })
}

#[tauri::command]
pub async fn restore_file_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    file_path: String,
) -> Result<UndoResultInfo, String> {
    if let Ok(mut guard) = state.rolling_back.lock() {
        *guard = true;
    }

    let result = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let objects_root = rew_home_dir().join("objects");
        let engine = TaskRestoreEngine::new(objects_root);
        engine.undo_file(&db, &task_id, &PathBuf::from(&file_path))
            .map_err(|e| e.to_string())
    };

    // On success: persist restored_at to DB + add path to suppressed_paths
    if result.is_ok() {
        let now = chrono::Utc::now();
        if let Ok(db) = state.db.lock() {
            let _ = db.mark_change_restored(&task_id, &PathBuf::from(&file_path), now);
        }
        // Suppress FSEvents for this specific path for 60 s so async FSEvent
        // delivery after the write doesn't create spurious timeline entries.
        if let Ok(mut sp) = state.suppressed_paths.lock() {
            sp.insert(PathBuf::from(&file_path), std::time::Instant::now());
        }
    }

    // Also clear the global rolling_back flag after a short delay
    let app_handle = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        if let Some(s) = app_handle.try_state::<AppState>() {
            if let Ok(mut g) = s.rolling_back.lock() {
                *g = false;
            }
        }
    });

    let res = result?;
    Ok(UndoResultInfo {
        files_restored: res.files_restored,
        files_deleted: res.files_deleted,
        failures: res.failures.iter().map(|(p, e)| (p.to_string_lossy().to_string(), e.clone())).collect(),
    })
}

// ----------------------------------------------------------------
// Legacy aliases kept for backward compat (delegate to new commands)
// ----------------------------------------------------------------

#[tauri::command]
pub async fn preview_undo(state: State<'_, AppState>, task_id: String) -> Result<UndoPreviewInfo, String> {
    preview_rollback(state, task_id).await
}

#[tauri::command]
pub async fn undo_task_cmd(app: tauri::AppHandle, state: State<'_, AppState>, task_id: String) -> Result<UndoResultInfo, String> {
    rollback_task_cmd(app, state, task_id).await
}

#[tauri::command]
pub async fn undo_file_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    file_path: String,
) -> Result<UndoResultInfo, String> {
    restore_file_cmd(app, state, task_id, file_path).await
}

// ================================================================
// Monitoring window configuration
// ================================================================

#[tauri::command]
pub async fn get_monitoring_window(state: State<'_, AppState>) -> Result<u64, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    Ok(config.monitoring_window_secs)
}

#[tauri::command]
pub async fn set_monitoring_window(state: State<'_, AppState>, secs: u64) -> Result<(), String> {
    let config_path = rew_core::rew_home_dir().join("config.toml");

    // Seal any currently-open monitoring window immediately at now.
    // This prevents the situation where the new (shorter) window_secs would
    // produce a seal_time in the past, conflicting with already-recorded changes.
    let now = chrono::Utc::now();
    let sealed_id: Option<String> = {
        let mut guard = state.fs_window_task.lock().map_err(|e| e.to_string())?;
        let id = guard.as_ref().map(|w| w.task_id.clone());
        *guard = None;
        id
    };
    if let Some(task_id) = sealed_id {
        if let Ok(db) = state.db.lock() {
            let _ = db.update_task_completed_at(&task_id, now);
        }
    }

    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    // Clamp to sane range: 1 minute – 2 hours
    config.monitoring_window_secs = secs.clamp(60, 7200);
    config.save(&config_path).map_err(|e| e.to_string())?;
    Ok(())
}

// ================================================================
// Scan progress + directory management (V3)
// ================================================================

/// Scan progress info for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgressInfo {
    pub is_scanning: bool,
    pub current_dir: Option<String>,
    pub dirs: Vec<DirScanStatusInfo>,
}

/// Per-directory scan status for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirScanStatusInfo {
    pub path: String,
    pub name: String,
    pub status: String,
    pub files_total: usize,
    pub files_done: usize,
    pub files_failed: usize,
    pub percent: f64,
    pub last_completed_at: Option<String>,
}

#[tauri::command]
pub async fn get_scan_progress(state: State<'_, AppState>) -> Result<ScanProgressInfo, String> {
    let progress = state.scan_progress.lock().map_err(|e| e.to_string())?;
    Ok(ScanProgressInfo {
        is_scanning: progress.is_scanning,
        current_dir: progress.current_dir.clone(),
        dirs: progress.dirs.iter().map(|d| {
            let name = d.path.split('/').last().unwrap_or(&d.path).to_string();
            DirScanStatusInfo {
                path: d.path.clone(),
                name,
                status: match d.status {
                    crate::state::DirStatus::Pending => "pending".to_string(),
                    crate::state::DirStatus::Scanning => "scanning".to_string(),
                    crate::state::DirStatus::Complete => "complete".to_string(),
                },
                files_total: d.files_total,
                files_done: d.files_done,
                files_failed: d.files_failed,
                percent: if d.files_total > 0 {
                    (d.files_done as f64 / d.files_total as f64 * 100.0).min(100.0)
                } else {
                    0.0
                },
                last_completed_at: d.last_completed_at.clone(),
            }
        }).collect(),
    })
}

#[tauri::command]
pub async fn add_watch_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    dir_path: String,
) -> Result<(), String> {
    let path = PathBuf::from(&dir_path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("目录不存在: {}", dir_path));
    }

    // Update config with overlap detection
    {
        let mut config = state.config.lock().map_err(|e| e.to_string())?;

        // Already in list — idempotent
        if config.watch_dirs.contains(&path) {
            return Ok(());
        }

        // Check if new path is a sub-directory of an existing watch dir
        for existing in &config.watch_dirs {
            if path.starts_with(existing) {
                let parent_name = existing.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("父目录")
                    .to_string();
                return Err(format!(
                    "该目录已被「{}」保护，无需重复添加。\n\
                     你可以在侧栏点击「{}」展开，选中该子目录来筛选存档记录。",
                    parent_name, parent_name
                ));
            }
        }

        // Check if new path is a parent of existing watch dirs — absorb them
        let covered: Vec<PathBuf> = config.watch_dirs.iter()
            .filter(|d| d.starts_with(&path))
            .cloned()
            .collect();
        if !covered.is_empty() {
            config.watch_dirs.retain(|d| !d.starts_with(&path));
            tracing::info!(
                "add_watch_dir: new parent {:?} covers {:?}, removed sub-dirs",
                path, covered
            );
        }

        config.watch_dirs.push(path.clone());
        config.save(&rew_home_dir().join("config.toml")).map_err(|e| e.to_string())?;
    }

    // Add to scan progress as "pending" and notify frontend immediately
    {
        let mut progress = state.scan_progress.lock().map_err(|e| e.to_string())?;
        if !progress.dirs.iter().any(|d| d.path == dir_path) {
            progress.dirs.push(crate::state::DirScanStatus {
                path: dir_path.clone(),
                status: crate::state::DirStatus::Pending,
                files_total: 0,
                files_done: 0,
                files_stored: 0,
                files_skipped: 0,
                files_failed: 0,
                last_completed_at: None,
            });
        }
        let _ = app.emit("scan-progress", &*progress);
    }

    // Kick off background scan for this directory
    let sp = state.scan_progress.clone();
    let config = state.config.lock().map_err(|e| e.to_string())?.clone();
    let path_for_scan = path.clone();
    let app_scan = app.clone();
    std::thread::spawn(move || {
        let path = path_for_scan;
        let sp2 = sp.clone();
        let app_cb = app_scan.clone();
        let callback: rew_core::scanner::ProgressCallback = Box::new(move |update| {
            if let Ok(mut progress) = sp2.lock() {
                if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == update.dir) {
                    ds.status = crate::state::DirStatus::Scanning;
                    ds.files_total = update.files_total_estimate;
                    ds.files_done = update.files_scanned;
                    ds.files_stored = update.files_stored;
                    ds.files_skipped = update.files_skipped;
                    ds.files_failed = update.files_failed;
                }
                let _ = app_cb.emit("scan-progress", &*progress);
            }
        });

        // Merge per-directory ignore patterns into the global list for this scan
        let mut patterns = config.ignore_patterns.clone();
        let dir_str = path.display().to_string();
        if let Some(dir_cfg) = config.dir_ignore.get(&dir_str) {
            for d in &dir_cfg.exclude_dirs {
                patterns.push(format!("**/{d}/**"));
            }
            for ext in &dir_cfg.exclude_extensions {
                patterns.push(format!("**/*.{ext}"));
            }
        }

        let result = rew_core::scanner::full_scan(
            &[path],
            &patterns,
            &rew_home_dir(),
            Some(callback),
            None,
            config.max_file_size_bytes,
        );

        // Mark complete and notify
        if let Ok(mut progress) = sp.lock() {
            if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == dir_path) {
                ds.status = crate::state::DirStatus::Complete;
                ds.files_done = result.files_scanned;
                ds.files_stored = result.files_stored;
                ds.files_skipped = result.files_skipped;
                ds.files_failed = result.files_failed;
                if ds.files_total < result.files_scanned {
                    ds.files_total = result.files_scanned;
                }
                ds.last_completed_at = Some(chrono::Utc::now().to_rfc3339());
            }
            crate::state::save_scan_status(&progress);
            let _ = app_scan.emit("scan-progress", &*progress);
            let _ = app_scan.emit("scan-complete", &dir_path);
        }
    });

    // Hot-add to running FSEvents pipeline
    if let Ok(tx_guard) = state.pipeline_tx.lock() {
        if let Some(ref tx) = *tx_guard {
            let _ = tx.send(crate::state::PipelineCmd::AddPath(path));
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn remove_watch_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    dir_path: String,
) -> Result<(), String> {
    let path = PathBuf::from(&dir_path);

    // Update config
    {
        let mut config = state.config.lock().map_err(|e| e.to_string())?;
        config.watch_dirs.retain(|d| d != &path);
        config.save(&rew_home_dir().join("config.toml")).map_err(|e| e.to_string())?;
    }

    // Remove from scan progress and notify frontend immediately
    {
        let mut progress = state.scan_progress.lock().map_err(|e| e.to_string())?;
        progress.dirs.retain(|d| d.path != dir_path);
        crate::state::save_scan_status(&progress);
        // Emit updated progress so frontend re-renders immediately
        let _ = app.emit("scan-progress", &*progress);
    }

    // Hot-remove from running FSEvents pipeline
    if let Ok(tx_guard) = state.pipeline_tx.lock() {
        if let Some(ref tx) = *tx_guard {
            let _ = tx.send(crate::state::PipelineCmd::RemovePath(path));
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn update_ignore_config(
    state: State<'_, AppState>,
    ignore_patterns: Vec<String>,
    size_limit_bytes: Option<u64>,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.ignore_patterns = ignore_patterns;
    config.max_file_size_bytes = size_limit_bytes;
    config.save(&rew_home_dir().join("config.toml")).map_err(|e| e.to_string())?;
    Ok(())
}

/// Get the current ignore config (patterns + size limit) for the settings panel.
#[tauri::command]
pub async fn get_ignore_config(
    state: State<'_, AppState>,
) -> Result<IgnoreConfigInfo, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    Ok(IgnoreConfigInfo {
        ignore_patterns: config.ignore_patterns.clone(),
        max_file_size_bytes: config.max_file_size_bytes,
    })
}

/// Get ObjectStore storage statistics.
#[tauri::command]
pub async fn get_storage_info() -> Result<StorageInfo, String> {
    let rew_home = rew_home_dir();
    let objects_dir = rew_home.join("objects");

    // Count objects and apparent size
    let mut object_count = 0usize;
    let mut apparent_bytes = 0u64;

    if objects_dir.exists() {
        let mut stack = vec![objects_dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with(".tmp-") {
                        object_count += 1;
                        apparent_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                    }
                }
            }
        }
    }

    // Physical disk cost: on APFS with clonefile, it's near-zero.
    // We report apparent size and note it's CoW.
    Ok(StorageInfo {
        object_count,
        apparent_bytes,
        note: format!(
            "{} 个备份对象 · 使用 APFS clonefile 尽量与原文件共享空间",
            object_count
        ),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageInfo {
    pub object_count: usize,
    pub apparent_bytes: u64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoreConfigInfo {
    pub ignore_patterns: Vec<String>,
    pub max_file_size_bytes: Option<u64>,
}

// ================================================================
// Per-directory ignore configuration
// ================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirIgnoreConfigInfo {
    pub exclude_dirs: Vec<String>,
    pub exclude_extensions: Vec<String>,
}

/// List immediate subdirectory names under a watched directory.
/// Used by the frontend to populate the exclude-directory picker.
#[tauri::command]
pub async fn list_subdirs(dir_path: String) -> Result<Vec<String>, String> {
    let items = list_dir_contents(dir_path).await?;
    Ok(items.into_iter().filter(|i| i.is_dir).map(|i| i.name).collect())
}

/// A file or directory entry returned from list_dir_contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirContentItem {
    pub name: String,
    pub full_path: String,
    pub is_dir: bool,
    /// Unix timestamp seconds (mtime), 0 if unavailable
    pub modified_secs: u64,
}

/// List immediate contents (dirs AND files) of a directory, sorted by mtime desc.
/// Skips hidden entries and well-known noise directories.
#[tauri::command]
pub async fn list_dir_contents(dir_path: String) -> Result<Vec<DirContentItem>, String> {
    let path = PathBuf::from(&dir_path);
    if !path.is_dir() {
        return Ok(vec![]);
    }
    let mut items = Vec::new();
    let entries = std::fs::read_dir(&path).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let p = entry.path();
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Skip hidden entries
        if name.starts_with('.') { continue; }
        let is_dir = p.is_dir();
        // Skip noisy dirs
        if is_dir && matches!(name.as_str(), "node_modules" | "target" | "__pycache__") {
            continue;
        }
        if is_dir && name.ends_with(".app") { continue; }

        let modified_secs = entry.metadata().ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        items.push(DirContentItem {
            name,
            full_path: p.to_string_lossy().to_string(),
            is_dir,
            modified_secs,
        });
    }
    // Sort: dirs first, then by mtime descending
    items.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then(b.modified_secs.cmp(&a.modified_secs))
    });
    Ok(items)
}

#[tauri::command]
pub async fn get_dir_ignore_config(
    state: State<'_, AppState>,
    dir_path: String,
) -> Result<DirIgnoreConfigInfo, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let cfg = config.dir_ignore.get(&dir_path).cloned().unwrap_or_default();
    Ok(DirIgnoreConfigInfo {
        exclude_dirs: cfg.exclude_dirs,
        exclude_extensions: cfg.exclude_extensions,
    })
}

#[tauri::command]
pub async fn update_dir_ignore_config(
    state: State<'_, AppState>,
    dir_path: String,
    exclude_dirs: Vec<String>,
    exclude_extensions: Vec<String>,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    if exclude_dirs.is_empty() && exclude_extensions.is_empty() {
        config.dir_ignore.remove(&dir_path);
    } else {
        let entry = config.dir_ignore.entry(dir_path).or_default();
        entry.exclude_dirs = exclude_dirs;
        entry.exclude_extensions = exclude_extensions;
    }
    config.save(&rew_home_dir().join("config.toml")).map_err(|e| e.to_string())?;
    Ok(())
}

/// Re-scan a single watched directory using the current exclude config.
/// Useful after changing exclude rules so new objects are stored and
/// scan progress reflects the latest state.
#[tauri::command]
pub async fn rescan_watch_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    dir_path: String,
) -> Result<(), String> {
    let path = PathBuf::from(&dir_path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("目录不存在: {}", dir_path));
    }

    // Mark as scanning immediately and notify frontend
    {
        let mut progress = state.scan_progress.lock().map_err(|e| e.to_string())?;
        if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == dir_path) {
            ds.status = crate::state::DirStatus::Scanning;
            ds.files_done = 0;
        }
        let _ = app.emit("scan-progress", &*progress);
    }

    let sp = state.scan_progress.clone();
    let config = state.config.lock().map_err(|e| e.to_string())?.clone();
    let app_scan = app.clone();

    std::thread::spawn(move || {
        let sp2 = sp.clone();
        let dir_path_clone = dir_path.clone();
        let app_cb = app_scan.clone();
        let callback: rew_core::scanner::ProgressCallback = Box::new(move |update| {
            if let Ok(mut progress) = sp2.lock() {
                if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == update.dir) {
                    ds.status = crate::state::DirStatus::Scanning;
                    ds.files_total = update.files_total_estimate;
                    ds.files_done = update.files_scanned;
                    ds.files_stored = update.files_stored;
                    ds.files_skipped = update.files_skipped;
                    ds.files_failed = update.files_failed;
                }
                let _ = app_cb.emit("scan-progress", &*progress);
            }
        });

        // Merge per-directory ignore patterns
        let mut patterns = config.ignore_patterns.clone();
        let dir_str = path.display().to_string();
        if let Some(dir_cfg) = config.dir_ignore.get(&dir_str) {
            for d in &dir_cfg.exclude_dirs {
                patterns.push(format!("**/{d}/**"));
            }
            for ext in &dir_cfg.exclude_extensions {
                let bare = ext.strip_prefix('.').unwrap_or(ext);
                patterns.push(format!("**/*.{bare}"));
                // Also match dotfiles like .gitconfig (no extension per glob)
                patterns.push(format!("**/.{bare}"));
            }
        }

        rew_core::scanner::full_scan(
            &[path],
            &patterns,
            &rew_home_dir(),
            Some(callback),
            None,
            config.max_file_size_bytes,
        );

        if let Ok(mut progress) = sp.lock() {
            if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == dir_path_clone) {
                ds.status = crate::state::DirStatus::Complete;
                ds.last_completed_at = Some(chrono::Utc::now().to_rfc3339());
            }
            crate::state::save_scan_status(&progress);
            let _ = app_scan.emit("scan-progress", &*progress);
            let _ = app_scan.emit("scan-complete", &dir_path_clone);
        }
    });

    Ok(())
}

// ================================================================
// Directory analysis for settings panel
// ================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryStats {
    pub category: String,
    pub extensions: String,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirAnalysis {
    pub path: String,
    pub total_files: usize,
    pub total_bytes: u64,
    pub categories: Vec<CategoryStats>,
    /// Files over a given size threshold
    pub large_file_count: usize,
    pub large_file_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullAnalysis {
    pub dirs: Vec<DirAnalysis>,
    pub total_files: usize,
    pub total_bytes: u64,
    pub categories: Vec<CategoryStats>,
    pub large_file_count: usize,
    pub large_file_bytes: u64,
}

/// Analyze all watched directories: count files per category and estimate sizes.
/// Optimized: skips deep recursion into dev dirs (node_modules, .git, etc.)
/// and limits traversal depth to keep analysis fast (~seconds, not minutes).
#[tauri::command]
pub async fn analyze_directories(state: State<'_, AppState>) -> Result<FullAnalysis, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?.clone();

    tokio::task::spawn_blocking(move || run_analysis(&config))
        .await
        .map_err(|e| format!("Analysis task failed: {}", e))?
}

fn run_analysis(config: &rew_core::config::RewConfig) -> Result<FullAnalysis, String> {
    use rew_core::watcher::filter::PathFilter;

    let start = std::time::Instant::now();
    tracing::info!("analyze_directories: starting...");

    // Use the same PathFilter as scanner — consistent filtering
    let filter = PathFilter::new(&config.ignore_patterns)
        .map_err(|e| format!("Invalid ignore pattern: {}", e))?;

    let size_threshold: u64 = 100 * 1024 * 1024;

    let mut all_dirs = Vec::new();
    let mut grand_total_files = 0usize;
    let mut grand_total_bytes = 0u64;
    let mut grand_large_count = 0usize;
    let mut grand_large_bytes = 0u64;

    for dir_path in &config.watch_dirs {
        if !dir_path.exists() { continue; }

        // Per-directory ignore config (user-configured excludes)
        let dir_str = dir_path.display().to_string();
        let dir_ignore_cfg = config.dir_ignore.get(&dir_str).cloned()
            .unwrap_or_default();

        let mut total_files = 0usize;
        let mut total_bytes = 0u64;
        let mut large_count = 0usize;
        let mut large_bytes = 0u64;

        let mut stack = vec![dir_path.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();

                // Apply global filter
                if filter.should_ignore(&path) { continue; }

                // Apply per-directory user ignore config (same logic as scanner/daemon)
                if PathFilter::should_ignore_by_dir_config(&path, dir_path, &dir_ignore_cfg) {
                    continue;
                }

                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    // Skip hidden dirs and .app bundles
                    if !name.starts_with('.') && !name.ends_with(".app") {
                        stack.push(path);
                    }
                    continue;
                }

                if !path.is_file() { continue; }

                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                total_files += 1;
                total_bytes += size;

                if size > size_threshold {
                    large_count += 1;
                    large_bytes += size;
                }
            }
        }

        grand_total_files += total_files;
        grand_total_bytes += total_bytes;
        grand_large_count += large_count;
        grand_large_bytes += large_bytes;

        all_dirs.push(DirAnalysis {
            path: dir_path.display().to_string(),
            total_files,
            total_bytes,
            categories: vec![],
            large_file_count: large_count,
            large_file_bytes: large_bytes,
        });
    }

    tracing::info!(
        "analyze_directories: done in {:.1}s — {} files, {} dirs",
        start.elapsed().as_secs_f64(), grand_total_files, all_dirs.len()
    );

    Ok(FullAnalysis {
        dirs: all_dirs,
        total_files: grand_total_files,
        total_bytes: grand_total_bytes,
        categories: vec![],
        large_file_count: grand_large_count,
        large_file_bytes: grand_large_bytes,
    })
}

// (quick_dir_estimate removed — no longer needed)

// ================================================================
// Manual snapshot
// ================================================================

/// Create a manual save point ("手动存档").
/// Seals the current monitoring window (writing all pending changes to a timeline
/// entry) and then inserts a dedicated manual-snapshot task so it appears in the
/// timeline as a distinct, user-created checkpoint.
#[tauri::command]
pub async fn create_manual_snapshot(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use rew_core::types::{Task, TaskStatus};

    let now = chrono::Utc::now();

    // 1. Seal the current monitoring window so pending changes become their own entry.
    {
        let mut window_guard = state.fs_window_task.lock().map_err(|e| e.to_string())?;
        if let Some(ref old_window) = *window_guard {
            if let Ok(db) = state.db.lock() {
                let _ = db.update_task_completed_at(&old_window.task_id, now);
                tracing::info!(
                    "create_manual_snapshot: sealed monitoring window {}",
                    &old_window.task_id
                );
            }
        }
        *window_guard = None;
    }

    // 2. Create a new manual-snapshot Task row.
    let task_id = format!("manual_{}", now.format("%m%d%H%M%S%3f"));
    let task = Task {
        id: task_id.clone(),
        prompt: Some("手动存档".to_string()),
        tool: Some("手动存档".to_string()),
        started_at: now,
        completed_at: Some(now),
        status: TaskStatus::Completed,
        risk_level: None,
        summary: Some("用户创建的手动存档点".to_string()),
        cwd: None,
    };

    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.create_task(&task).map_err(|e| e.to_string())?;
    }

    // 3. Notify the frontend so the timeline refreshes immediately.
    let _ = app.emit("task-updated", &task_id);

    Ok(task_id)
}

// ================================================================
// AI Tool Hook Management
// ================================================================

#[tauri::command]
pub async fn detect_ai_tools() -> Result<Vec<rew_core::hooks::AiToolInfo>, String> {
    let rew_bin = rew_core::rew_cli_bin_path()
        .to_string_lossy()
        .to_string();
    Ok(rew_core::hooks::detect_ai_tools(&rew_bin))
}

#[tauri::command]
pub async fn install_tool_hook(tool_id: String) -> Result<(), String> {
    let rew_bin = rew_core::rew_cli_bin_path()
        .to_string_lossy()
        .to_string();
    rew_core::hooks::install_hook(&tool_id, &rew_bin)
}

#[tauri::command]
pub async fn uninstall_tool_hook(tool_id: String) -> Result<(), String> {
    let rew_bin = rew_core::rew_cli_bin_path()
        .to_string_lossy()
        .to_string();
    rew_core::hooks::uninstall_hook(&tool_id, &rew_bin)
}
