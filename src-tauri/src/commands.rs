//! Tauri IPC commands exposing rew-core functionality to the frontend.

use crate::state::{AppState, RestorePhase, SuppressedRestorePath};
use chrono::{Duration, Local, NaiveTime, TimeZone};
use rew_core::db::{RestoreFailureSample, RestoreFileIndexSyncEntry};
use rew_core::restore::{PreparedSuppressionEntry, TaskRestoreEngine, UndoResult};
use rew_core::types::{
    RestoreOperationStatus, RestoreScopeType, RestoreTriggeredBy, Snapshot, SnapshotTrigger,
};
use rew_core::rew_home_dir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

fn emit_restore_progress(app: &AppHandle, state: &AppState) {
    if let Ok(progress) = state.restore_progress.lock() {
        let _ = app.emit("restore-progress", progress.clone());
    }
}

const RESTORE_PROGRESS_EMIT_EVERY_FILES: usize = 250;
const RESTORE_PROGRESS_EMIT_INTERVAL_MS: u128 = 150;
const RESTORE_FAILURE_SAMPLE_LIMIT: usize = 20;

fn record_restore_suppressions(state: &AppState, entries: &[PreparedSuppressionEntry]) {
    if let Ok(mut suppressed) = state.suppressed_restore_paths.lock() {
        let now = std::time::Instant::now();
        for entry in entries {
            suppressed.insert(
                entry.path.clone(),
                SuppressedRestorePath {
                    created_at: now,
                    expected_content_hash: entry.expected_content_hash.clone(),
                    deleted: entry.deleted,
                },
            );
        }
    }
}

fn restore_cleanup_boundaries(state: &AppState, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut boundaries = state
        .config
        .lock()
        .ok()
        .map(|cfg| cfg.watch_dirs.clone())
        .unwrap_or_default();
    boundaries.extend(extra.iter().cloned());
    boundaries.sort();
    boundaries.dedup();
    boundaries
}

fn summarize_restore_status(result: &UndoResult) -> RestoreOperationStatus {
    if result.failures.is_empty() {
        RestoreOperationStatus::Completed
    } else if result.files_restored > 0 || result.files_deleted > 0 {
        RestoreOperationStatus::Partial
    } else {
        RestoreOperationStatus::Failed
    }
}

fn sample_restore_failures(failures: &[(PathBuf, String)]) -> Vec<RestoreFailureSample> {
    failures
        .iter()
        .take(RESTORE_FAILURE_SAMPLE_LIMIT)
        .map(|(path, error)| RestoreFailureSample {
            file_path: path.clone(),
            error: error.clone(),
        })
        .collect()
}

fn build_restore_file_index_sync_entries(
    db: &rew_core::db::Database,
    entries: &[PreparedSuppressionEntry],
) -> Vec<RestoreFileIndexSyncEntry> {
    let mut updates = Vec::with_capacity(entries.len());
    for entry in entries {
        let path_key = entry.path.to_string_lossy().to_string();

        if entry.deleted {
            let restore_hash = db
                .get_file_index_entry(&path_key)
                .ok()
                .flatten()
                .and_then(|existing| existing.content_hash.or(Some(existing.fast_hash)));
            if let Some(hash) = restore_hash {
                updates.push(RestoreFileIndexSyncEntry {
                    file_path: entry.path.clone(),
                    mtime_secs: 0,
                    fast_hash: hash.clone(),
                    content_hash: Some(hash),
                    deleted: true,
                });
            }
            continue;
        }

        if !entry.path.exists() {
            continue;
        }

        let mtime_secs = std::fs::metadata(&entry.path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        let existing = db.get_file_index_entry(&path_key).ok().flatten();
        let mut content_hash = entry
            .expected_content_hash
            .clone()
            .or_else(|| existing.as_ref().and_then(|row| row.content_hash.clone()));
        let mut fast_hash = existing.as_ref().map(|row| row.fast_hash.clone());

        if fast_hash.is_none() && content_hash.is_none() {
            content_hash = rew_core::objects::sha256_file(&entry.path).ok();
        }

        if fast_hash.is_none() {
            fast_hash = content_hash.clone();
        }

        if let Some(fast_hash) = fast_hash {
            updates.push(RestoreFileIndexSyncEntry {
                file_path: entry.path.clone(),
                mtime_secs,
                fast_hash,
                content_hash,
                deleted: false,
            });
        }
    }
    updates
}

fn sync_file_index_after_directory_restore(
    db: &mut rew_core::db::Database,
    entries: &[PreparedSuppressionEntry],
) {
    let seen_at = chrono::Utc::now().to_rfc3339();
    let updates = build_restore_file_index_sync_entries(db, entries);
    let _ = db.sync_file_index_after_restore_batch(&updates, &seen_at);
}

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
    let snapshots = db.list_snapshots().unwrap_or_default();
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

            let scan_db = match rew_core::db::Database::open(&rew_home_dir().join("snapshots.db")) {
                Ok(db) => { let _ = db.initialize(); db }
                Err(e) => {
                    tracing::warn!("Scan: failed to open DB: {}", e);
                    return;
                }
            };
            let result = rew_core::scanner::full_scan(
                &[path],
                &patterns,
                &rew_home_dir(),
                &scan_db,
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
    pub finalization_status: Option<String>,
    pub risk_level: Option<String>,
    pub summary: Option<String>,
    pub changes_count: usize,
    pub cwd: Option<String>,
    pub total_lines_added: u32,
    pub total_lines_removed: u32,
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
    pub attribution: Option<String>,
    /// Original file path before rename (only for renamed changes).
    pub old_file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskChangesResultInfo {
    pub changes: Vec<ChangeInfo>,
    pub total_count: usize,
    pub truncated: bool,
    pub deleted_dirs: Vec<DeletedDirSummaryInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedDirSummaryInfo {
    pub dir_path: String,
    pub total_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreProgressInfo {
    pub is_running: bool,
    pub phase: crate::state::RestorePhase,
    pub task_id: Option<String>,
    pub dir_path: Option<String>,
    pub total_files: usize,
    pub processed_files: usize,
    pub restored_files: usize,
    pub deleted_files: usize,
    pub failed_files: usize,
    pub current_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreOperationInfo {
    pub id: String,
    pub source_task_id: String,
    pub scope_type: String,
    pub scope_path: Option<String>,
    pub triggered_by: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub requested_count: u32,
    pub restored_count: u32,
    pub deleted_count: u32,
    pub failed_count: u32,
    pub failure_sample_json: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathFilterMode {
    File,
    Directory,
}

fn infer_path_filter_mode(db: &rew_core::db::Database, path: &str) -> PathFilterMode {
    if db.has_changes_under_dir_prefix(path).unwrap_or(false) {
        PathFilterMode::Directory
    } else if db.has_exact_change_path(path).unwrap_or(false) {
        PathFilterMode::File
    } else if Path::new(path).is_dir() {
        PathFilterMode::Directory
    } else {
        PathFilterMode::File
    }
}

fn task_info_from_parts(
    db: &rew_core::db::Database,
    task: rew_core::types::Task,
    changes_count: u32,
    total_lines_added: u32,
    total_lines_removed: u32,
) -> TaskInfo {
    let finalization_status = db.get_task_finalization_status(&task.id).ok().flatten();
    TaskInfo {
        id: task.id,
        prompt: task.prompt,
        tool: task.tool,
        started_at: task.started_at.to_rfc3339(),
        completed_at: task.completed_at.map(|t| t.to_rfc3339()),
        status: task.status.to_string(),
        finalization_status,
        risk_level: task.risk_level.map(|r| r.to_string()),
        summary: task.summary,
        changes_count: changes_count as usize,
        cwd: task.cwd,
        total_lines_added,
        total_lines_removed,
    }
}

fn active_task_rollup_from_changes(
    db: &rew_core::db::Database,
    task_id: &str,
) -> (u32, u32, u32) {
    match db.get_changes_for_task(task_id) {
        Ok(changes) => (
            changes.len() as u32,
            changes.iter().map(|c| c.lines_added).sum(),
            changes.iter().map(|c| c.lines_removed).sum(),
        ),
        Err(_) => (0, 0, 0),
    }
}

fn should_use_live_rollup(
    task: &rew_core::types::Task,
    finalization_status: Option<&str>,
    stored_changes_count: u32,
) -> bool {
    matches!(task.status, rew_core::types::TaskStatus::Active)
        || matches!(finalization_status, Some("pending") | Some("running") | Some("failed"))
        || (task.tool.as_deref() == Some("文件监听")
            && stored_changes_count == 0
            && finalization_status.is_none())
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

    let mut tasks = if let Some(ref path) = dir_filter {
        if infer_path_filter_mode(&db, path) == PathFilterMode::File {
            db.list_tasks_by_file(path).map_err(|e| format!("list_tasks_by_file error: {}", e))?
        } else {
            db.list_tasks_by_dir(path).map_err(|e| format!("list_tasks_by_dir error: {}", e))?
        }
    } else {
        db.list_tasks().map_err(|e| format!("list_tasks error: {}", e))?
    };

    // Remove the active window so the frontend never sees an in-progress entry
    if let Some(ref id) = active_window_id {
        tasks.retain(|tw| &tw.task.id != id);
    }

    tracing::info!("list_tasks: found {} tasks in DB (dir_filter={:?})", tasks.len(), dir_filter);

    let use_active_fallback = dir_filter.is_none();
    let result: Vec<TaskInfo> = tasks
        .into_iter()
        .map(|tw| {
            let finalization_status = db.get_task_finalization_status(&tw.task.id).ok().flatten();
            let needs_live_rollup =
                should_use_live_rollup(&tw.task, finalization_status.as_deref(), tw.changes_count);
            let (changes_count, total_lines_added, total_lines_removed) =
                if use_active_fallback && needs_live_rollup {
                    active_task_rollup_from_changes(&db, &tw.task.id)
                } else {
                    (tw.changes_count, tw.total_lines_added, tw.total_lines_removed)
                };
            task_info_from_parts(
                &db,
                tw.task,
                changes_count,
                total_lines_added,
                total_lines_removed,
            )
        })
        .collect();

    tracing::info!("list_tasks: returning {} tasks to frontend", result.len());
    Ok(result)
}

#[tauri::command]
pub async fn get_task(state: State<'_, AppState>, task_id: String) -> Result<TaskInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task = db.get_task(&task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task '{}' not found", task_id))?;
    let finalization_status = db.get_task_finalization_status(&task.id).map_err(|e| e.to_string())?;
    let stored_rollup = db.get_task_rollup(&task.id).map_err(|e| e.to_string())?;
    let (changes_count, total_lines_added, total_lines_removed) =
        if should_use_live_rollup(&task, finalization_status.as_deref(), stored_rollup.0) {
            active_task_rollup_from_changes(&db, &task.id)
        } else {
            stored_rollup
        };

    Ok(task_info_from_parts(
        &db,
        task,
        changes_count,
        total_lines_added,
        total_lines_removed,
    ))
}

#[tauri::command]
pub async fn get_task_changes(
    state: State<'_, AppState>,
    task_id: String,
    dir_filter: Option<String>,
) -> Result<TaskChangesResultInfo, String> {
    const DEFAULT_RETURNED_CHANGES: usize = 1000;
    const LARGE_TASK_RETURNED_CHANGES: usize = 100;
    const LARGE_TASK_THRESHOLD: usize = 5000;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let (changes, total_count) = if let Some(ref path) = dir_filter {
        if infer_path_filter_mode(&db, path) == PathFilterMode::File {
            let total_count = db.count_changes_for_file(&task_id, path).map_err(|e| e.to_string())?;
            let limit = if total_count > LARGE_TASK_THRESHOLD {
                LARGE_TASK_RETURNED_CHANGES
            } else {
                DEFAULT_RETURNED_CHANGES
            };
            (
                db.get_changes_for_task_by_file_limited(&task_id, path, limit)
                    .map_err(|e| e.to_string())?,
                total_count,
            )
        } else {
            let total_count = db.count_changes_in_dir(&task_id, path).map_err(|e| e.to_string())?;
            let limit = if total_count > LARGE_TASK_THRESHOLD {
                LARGE_TASK_RETURNED_CHANGES
            } else {
                DEFAULT_RETURNED_CHANGES
            };
            (
                db.get_changes_for_task_in_dir_limited(&task_id, path, limit)
                    .map_err(|e| e.to_string())?,
                total_count,
            )
        }
    } else {
        let total_count = db.count_changes_for_task(&task_id).map_err(|e| e.to_string())?;
        let limit = if total_count > LARGE_TASK_THRESHOLD {
            LARGE_TASK_RETURNED_CHANGES
        } else {
            DEFAULT_RETURNED_CHANGES
        };
        (
            db.get_changes_for_task_limited(&task_id, limit)
                .map_err(|e| e.to_string())?,
            total_count,
        )
    };

    Ok(TaskChangesResultInfo {
        truncated: total_count > changes.len(),
        total_count,
        deleted_dirs: db
            .list_task_deleted_dirs(&task_id)
            .map(|rows| {
                rows.into_iter()
                    .map(|row| DeletedDirSummaryInfo {
                        dir_path: row.dir_path.to_string_lossy().to_string(),
                        total_files: row.file_count,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        changes: changes.into_iter().map(|c| ChangeInfo {
            id: c.id,
            task_id: c.task_id,
            file_path: c.file_path.to_string_lossy().to_string(),
            change_type: c.change_type.to_string(),
            old_hash: c.old_hash,
            new_hash: c.new_hash,
            diff_text: c.diff_text,
            lines_added: c.lines_added,
            lines_removed: c.lines_removed,
            attribution: c.attribution,
            old_file_path: c.old_file_path.map(|p| p.to_string_lossy().to_string()),
        }).collect(),
    })
}

// ----------------------------------------------------------------
// Task statistics
// ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatsInfo {
    pub task_id: String,
    pub model: Option<String>,
    pub duration_secs: Option<f64>,
    pub tool_calls: i32,
    pub files_changed: i32,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightsToolStatInfo {
    pub tool_key: String,
    pub task_count: usize,
    pub total_duration_secs: f64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub duration_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightsDailyPointInfo {
    pub date: String,
    pub date_iso: String,
    pub task_count: usize,
    pub total_duration_secs: f64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightsTopTaskInfo {
    pub id: String,
    pub prompt: String,
    pub tool: Option<String>,
    pub duration_secs: f64,
    pub changes_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightsDataInfo {
    pub period: String,
    pub total_tokens: i64,
    pub total_duration_secs: f64,
    pub total_cost_usd: f64,
    pub total_tasks: usize,
    pub total_files_changed: usize,
    pub tool_stats: Vec<InsightsToolStatInfo>,
    pub daily_points: Vec<InsightsDailyPointInfo>,
    pub top_tasks: Vec<InsightsTopTaskInfo>,
}

#[tauri::command]
pub async fn get_task_stats(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Option<TaskStatsInfo>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let stats = db.get_task_stats(&task_id).map_err(|e| e.to_string())?;
    Ok(stats.map(|s| TaskStatsInfo {
        task_id: s.task_id,
        model: s.model,
        duration_secs: s.duration_secs,
        tool_calls: s.tool_calls,
        files_changed: s.files_changed,
        input_tokens: s.input_tokens,
        output_tokens: s.output_tokens,
        total_cost_usd: s.total_cost_usd,
        session_id: s.session_id,
        conversation_id: s.conversation_id,
    }))
}

fn insights_period_bounds(period: &str) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>), String> {
    let now = Local::now();
    let end_date = now.date_naive();
    let start_date = match period {
        "day" => end_date,
        "week" => end_date - Duration::days(6),
        "month" => end_date - Duration::days(29),
        _ => return Err(format!("Unsupported insights period: {}", period)),
    };

    let start_local = Local
        .from_local_datetime(&start_date.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap()))
        .single()
        .ok_or_else(|| "Failed to compute local start time".to_string())?;
    let end_local = Local
        .from_local_datetime(&end_date.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap()))
        .single()
        .ok_or_else(|| "Failed to compute local end time".to_string())?;
    Ok((start_local.with_timezone(&chrono::Utc), end_local.with_timezone(&chrono::Utc)))
}

#[tauri::command]
pub async fn get_insights(
    state: State<'_, AppState>,
    period: String,
) -> Result<InsightsDataInfo, String> {
    let (start_at, end_at) = insights_period_bounds(&period)?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let rows = db
        .list_insight_tasks(&start_at.to_rfc3339(), &end_at.to_rfc3339())
        .map_err(|e| e.to_string())?;

    let mut total_tokens = 0i64;
    let mut total_duration_secs = 0f64;
    let mut total_cost_usd = 0f64;
    let mut total_files_changed = 0usize;
    let mut total_tasks = 0usize;

    let mut tool_map: std::collections::BTreeMap<String, (usize, f64, i64, f64)> =
        std::collections::BTreeMap::new();
    let mut day_map: std::collections::BTreeMap<String, InsightsDailyPointInfo> =
        std::collections::BTreeMap::new();
    let mut top_tasks = Vec::new();

    let mut cursor = start_at.with_timezone(&Local).date_naive();
    let end_date = end_at.with_timezone(&Local).date_naive();
    while cursor <= end_date {
        let iso = cursor.format("%Y-%m-%d").to_string();
        day_map.insert(
            iso.clone(),
            InsightsDailyPointInfo {
                date: cursor.format("%m/%d").to_string(),
                date_iso: iso,
                task_count: 0,
                total_duration_secs: 0.0,
                total_tokens: 0,
            },
        );
        cursor += Duration::days(1);
    }

    for row in rows {
        total_tasks += 1;
        let local_started = row.started_at.with_timezone(&Local);
        let duration_secs = row.duration_secs.unwrap_or_else(|| {
            row.completed_at
                .map(|end| ((end - row.started_at).num_milliseconds().max(0) as f64) / 1000.0)
                .unwrap_or(0.0)
        });
        let files_changed = row
            .stat_files_changed
            .filter(|value| *value > 0 || row.task_changes_count == 0)
            .unwrap_or(row.task_changes_count) as usize;
        let tokens = row.input_tokens.unwrap_or(0) + row.output_tokens.unwrap_or(0);
        let cost = row.total_cost_usd.unwrap_or(0.0);
        let tool_key = row
            .tool
            .as_deref()
            .unwrap_or("unknown")
            .to_lowercase()
            .replace('_', "-");

        total_tokens += tokens;
        total_duration_secs += duration_secs;
        total_cost_usd += cost;
        total_files_changed += files_changed;

        let tool_entry = tool_map.entry(tool_key.clone()).or_insert((0, 0.0, 0, 0.0));
        tool_entry.0 += 1;
        tool_entry.1 += duration_secs;
        tool_entry.2 += tokens;
        tool_entry.3 += cost;

        let date_iso = local_started.format("%Y-%m-%d").to_string();
        if let Some(point) = day_map.get_mut(&date_iso) {
            point.task_count += 1;
            point.total_duration_secs += duration_secs;
            point.total_tokens += tokens;
        }

        top_tasks.push(InsightsTopTaskInfo {
            id: row.task_id,
            prompt: row
                .prompt
                .or(row.summary)
                .unwrap_or_else(|| "未命名任务".to_string()),
            tool: row.tool,
            duration_secs,
            changes_count: files_changed,
        });
    }

    let mut tool_stats: Vec<InsightsToolStatInfo> = tool_map
        .into_iter()
        .map(|(tool_key, (task_count, duration, tokens, cost))| InsightsToolStatInfo {
            tool_key,
            task_count,
            total_duration_secs: duration,
            total_tokens: tokens,
            total_cost_usd: cost,
            duration_percent: if total_duration_secs > 0.0 {
                (duration / total_duration_secs) * 100.0
            } else {
                0.0
            },
        })
        .collect();
    tool_stats.sort_by(|a, b| {
        b.total_duration_secs
            .partial_cmp(&a.total_duration_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    top_tasks.sort_by(|a, b| {
        b.duration_secs
            .partial_cmp(&a.duration_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_tasks.truncate(5);

    Ok(InsightsDataInfo {
        period,
        total_tokens,
        total_duration_secs,
        total_cost_usd,
        total_tasks,
        total_files_changed,
        tool_stats,
        daily_points: day_map.into_values().collect(),
        top_tasks,
    })
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

    // Short display name: just the filename for the diff header
    let name = std::path::Path::new(&file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.clone());

    let result = rew_core::diff::compute_diff_from_store(
        &obj_store,
        old_hash.as_deref(),
        new_hash.as_deref(),
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
    let engine = TaskRestoreEngine::new(objects_root)
        .with_cleanup_boundaries(restore_cleanup_boundaries(&state, &[]));

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

    let restore_op_id = Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now();
    let requested_count = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let count = db.count_changes_for_task(&task_id).map_err(|e| e.to_string())?;
        db.insert_restore_operation_started(
            &restore_op_id,
            &task_id,
            &RestoreScopeType::Task,
            None,
            &RestoreTriggeredBy::Ui,
            started_at,
            count,
            None,
        )
        .map_err(|e| e.to_string())?;
        count
    };

    let result = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let objects_root = rew_home_dir().join("objects");
        let engine = TaskRestoreEngine::new(objects_root)
            .with_cleanup_boundaries(restore_cleanup_boundaries(&state, &[]));
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

    let completed_at = chrono::Utc::now();
    if let Ok(db) = state.db.lock() {
        match &result {
            Ok(outcome) => {
                let _ = db.complete_restore_operation(
                    &restore_op_id,
                    &summarize_restore_status(outcome),
                    completed_at,
                    outcome.files_restored,
                    outcome.files_deleted,
                    outcome.failures.len(),
                    &sample_restore_failures(&outcome.failures),
                );
            }
            Err(err) => {
                let failure = vec![RestoreFailureSample {
                    file_path: PathBuf::from(format!("<task:{task_id}>")),
                    error: err.clone(),
                }];
                let _ = db.complete_restore_operation(
                    &restore_op_id,
                    &RestoreOperationStatus::Failed,
                    completed_at,
                    0,
                    0,
                    requested_count.max(1),
                    &failure,
                );
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

    let restore_op_id = Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now();
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.insert_restore_operation_started(
            &restore_op_id,
            &task_id,
            &RestoreScopeType::File,
            Some(Path::new(&file_path)),
            &RestoreTriggeredBy::Ui,
            started_at,
            1,
            None,
        )
        .map_err(|e| e.to_string())?;
    }

    let result = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let objects_root = rew_home_dir().join("objects");
        let engine = TaskRestoreEngine::new(objects_root)
            .with_cleanup_boundaries(restore_cleanup_boundaries(&state, &[]));
        engine.undo_file(&db, &task_id, &PathBuf::from(&file_path))
            .map_err(|e| e.to_string())
    };

    // On success: add path to suppressed_paths
    if result.is_ok() {
        // Suppress FSEvents for this specific path for 60 s so async FSEvent
        // delivery after the write doesn't create spurious timeline entries.
        if let Ok(mut sp) = state.suppressed_paths.lock() {
            sp.insert(PathBuf::from(&file_path), std::time::Instant::now());
        }
    }

    let completed_at = chrono::Utc::now();
    if let Ok(db) = state.db.lock() {
        match &result {
            Ok(outcome) => {
                let _ = db.complete_restore_operation(
                    &restore_op_id,
                    &summarize_restore_status(outcome),
                    completed_at,
                    outcome.files_restored,
                    outcome.files_deleted,
                    outcome.failures.len(),
                    &sample_restore_failures(&outcome.failures),
                );
            }
            Err(err) => {
                let failure = vec![RestoreFailureSample {
                    file_path: PathBuf::from(&file_path),
                    error: err.clone(),
                }];
                let _ = db.complete_restore_operation(
                    &restore_op_id,
                    &RestoreOperationStatus::Failed,
                    completed_at,
                    0,
                    0,
                    1,
                    &failure,
                );
            }
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

#[tauri::command]
pub async fn restore_directory_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    dir_path: String,
) -> Result<UndoResultInfo, String> {
    if let Ok(mut guard) = state.rolling_back.lock() {
        *guard = true;
    }

    let dir_path_buf = PathBuf::from(&dir_path);
    let objects_root = rew_home_dir().join("objects");
    let engine = TaskRestoreEngine::new(objects_root).with_cleanup_boundaries(
        restore_cleanup_boundaries(&state, std::slice::from_ref(&dir_path_buf)),
    );

    let restore_op_id = Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now();
    let requested_count = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let count = db
            .count_changes_matching_dir_scope(&task_id, &dir_path)
            .map_err(|e| e.to_string())?;
        db.insert_restore_operation_started(
            &restore_op_id,
            &task_id,
            &RestoreScopeType::Directory,
            Some(dir_path_buf.as_path()),
            &RestoreTriggeredBy::Ui,
            started_at,
            count,
            None,
        )
        .map_err(|e| e.to_string())?;
        count
    };

    let plan = match {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        engine
            .prepare_directory_undo(&db, &task_id, &dir_path_buf)
            .map_err(|e| e.to_string())
    } {
        Ok(plan) => plan,
        Err(err) => {
            if let Ok(db) = state.db.lock() {
                let failure = vec![RestoreFailureSample {
                    file_path: dir_path_buf.clone(),
                    error: err.clone(),
                }];
                let _ = db.complete_restore_operation(
                    &restore_op_id,
                    &RestoreOperationStatus::Failed,
                    chrono::Utc::now(),
                    0,
                    0,
                    requested_count.max(1),
                    &failure,
                );
            }
            return Err(err);
        }
    };

    if let Ok(mut progress) = state.restore_progress.lock() {
        *progress = crate::state::RestoreProgress {
            is_running: true,
            phase: RestorePhase::RestoringFiles,
            task_id: Some(task_id.clone()),
            dir_path: Some(dir_path.clone()),
            total_files: plan.scoped_change_paths.len(),
            processed_files: 0,
            restored_files: 0,
            deleted_files: 0,
            failed_files: 0,
            current_path: None,
        };
    }
    emit_restore_progress(&app, &state);

    let app_handle_for_progress = app.clone();
    let mut last_emitted_processed = 0usize;
    let mut last_emitted_at = std::time::Instant::now();
    let outcome = engine.execute_prepared_directory_undo_with_progress(&plan, |progress| {
        let should_emit = progress.processed_files == 0
            || progress.processed_files == progress.total_files
            || progress.processed_files.saturating_sub(last_emitted_processed)
                >= RESTORE_PROGRESS_EMIT_EVERY_FILES
            || last_emitted_at.elapsed().as_millis() >= RESTORE_PROGRESS_EMIT_INTERVAL_MS;

        if !should_emit {
            return;
        }

        last_emitted_processed = progress.processed_files;
        last_emitted_at = std::time::Instant::now();
        if let Ok(mut shared) = state.restore_progress.lock() {
            shared.is_running = true;
            shared.phase = RestorePhase::RestoringFiles;
            shared.task_id = Some(task_id.clone());
            shared.dir_path = Some(dir_path.clone());
            shared.total_files = progress.total_files;
            shared.processed_files = progress.processed_files;
            shared.restored_files = progress.restored_files;
            shared.deleted_files = progress.deleted_files;
            shared.failed_files = progress.failed_files;
            shared.current_path = progress
                .current_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string());
        }
        emit_restore_progress(&app_handle_for_progress, &state);
    });

    if let Ok(mut progress) = state.restore_progress.lock() {
        progress.is_running = true;
        progress.phase = RestorePhase::SyncingDatabase;
        progress.task_id = Some(task_id.clone());
        progress.dir_path = Some(dir_path.clone());
        progress.total_files = plan.scoped_change_paths.len();
        progress.processed_files = plan.scoped_change_paths.len();
        progress.restored_files = outcome.result.files_restored;
        progress.deleted_files = outcome.result.files_deleted;
        progress.failed_files = outcome.result.failures.len();
        progress.current_path = None;
    }
    emit_restore_progress(&app, &state);

    let finalize_result: Result<(), String> = (|| {
        let mut db = state.db.lock().map_err(|e| e.to_string())?;
        if !outcome.suppression_entries.is_empty() {
            sync_file_index_after_directory_restore(&mut db, &outcome.suppression_entries);
        }

        if let Ok(mut progress) = state.restore_progress.lock() {
            progress.phase = RestorePhase::Finalizing;
        }
        emit_restore_progress(&app, &state);

        if outcome.result.failures.is_empty() {
            let new_status = if plan.scoped_change_paths.len() == plan.total_task_changes {
                rew_core::types::TaskStatus::RolledBack
            } else {
                rew_core::types::TaskStatus::PartialRolledBack
            };
            db.update_task_status(&task_id, &new_status, None)
                .map_err(|e| e.to_string())?;
        } else if outcome.result.files_restored > 0 || outcome.result.files_deleted > 0 {
            db.update_task_status(
                &task_id,
                &rew_core::types::TaskStatus::PartialRolledBack,
                None,
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();

    if !outcome.suppression_entries.is_empty() {
        record_restore_suppressions(&state, &outcome.suppression_entries);
    }

    let completed_at = chrono::Utc::now();
    if let Ok(db) = state.db.lock() {
        let status = if finalize_result.is_ok() {
            summarize_restore_status(&outcome.result)
        } else if outcome.result.files_restored > 0 || outcome.result.files_deleted > 0 {
            RestoreOperationStatus::Partial
        } else {
            RestoreOperationStatus::Failed
        };
        let mut failures = sample_restore_failures(&outcome.result.failures);
        if let Err(err) = &finalize_result {
            failures.push(RestoreFailureSample {
                file_path: dir_path_buf.clone(),
                error: err.clone(),
            });
        }
        let _ = db.complete_restore_operation(
            &restore_op_id,
            &status,
            completed_at,
            outcome.result.files_restored,
            outcome.result.files_deleted,
            outcome.result.failures.len() + usize::from(finalize_result.is_err()),
            &failures,
        );
    }

    if let Err(err) = finalize_result {
        return Err(err);
    }

    if let Ok(mut progress) = state.restore_progress.lock() {
        progress.is_running = false;
        progress.phase = RestorePhase::Done;
        progress.task_id = Some(task_id.clone());
        progress.dir_path = Some(dir_path.clone());
        progress.total_files = plan.scoped_change_paths.len();
        progress.processed_files = plan.scoped_change_paths.len();
        progress.restored_files = outcome.result.files_restored;
        progress.deleted_files = outcome.result.files_deleted;
        progress.failed_files = outcome.result.failures.len();
        progress.current_path = None;
    }
    emit_restore_progress(&app, &state);

    let app_handle = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        if let Some(s) = app_handle.try_state::<AppState>() {
            if let Ok(mut g) = s.rolling_back.lock() {
                *g = false;
            }
        }
    });

    Ok(UndoResultInfo {
        files_restored: outcome.result.files_restored,
        files_deleted: outcome.result.files_deleted,
        failures: outcome.result
            .failures
            .iter()
            .map(|(p, e)| (p.to_string_lossy().to_string(), e.clone()))
            .collect(),
    })
}

#[tauri::command]
pub async fn get_restore_progress(state: State<'_, AppState>) -> Result<RestoreProgressInfo, String> {
    let progress = state.restore_progress.lock().map_err(|e| e.to_string())?;
    Ok(RestoreProgressInfo {
        is_running: progress.is_running,
        phase: progress.phase,
        task_id: progress.task_id.clone(),
        dir_path: progress.dir_path.clone(),
        total_files: progress.total_files,
        processed_files: progress.processed_files,
        restored_files: progress.restored_files,
        deleted_files: progress.deleted_files,
        failed_files: progress.failed_files,
        current_path: progress.current_path.clone(),
    })
}

#[tauri::command]
pub async fn list_restore_operations(
    state: State<'_, AppState>,
    task_id: String,
    limit: Option<usize>,
) -> Result<Vec<RestoreOperationInfo>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let operations = db
        .list_restore_operations_for_task(&task_id, limit.unwrap_or(20))
        .map_err(|e| e.to_string())?;
    Ok(operations
        .into_iter()
        .map(|op| RestoreOperationInfo {
            id: op.id,
            source_task_id: op.source_task_id,
            scope_type: op.scope_type.to_string(),
            scope_path: op.scope_path.map(|path| path.to_string_lossy().to_string()),
            triggered_by: op.triggered_by.to_string(),
            started_at: op.started_at.to_rfc3339(),
            completed_at: op.completed_at.map(|value| value.to_rfc3339()),
            status: op.status.to_string(),
            requested_count: op.requested_count,
            restored_count: op.restored_count,
            deleted_count: op.deleted_count,
            failed_count: op.failed_count,
            failure_sample_json: op.failure_sample_json,
        })
        .collect())
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
            let objs = rew_home_dir().join("objects");
            let _ = rew_core::reconcile::reconcile_task(&db, &task_id, &objs);
            let _ = db.refresh_task_rollup_from_changes(&task_id);
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

        let scan_db = match rew_core::db::Database::open(&rew_home_dir().join("snapshots.db")) {
            Ok(db) => { let _ = db.initialize(); db }
            Err(e) => {
                tracing::warn!("Scan dir: failed to open DB: {}", e);
                return;
            }
        };
        let result = rew_core::scanner::full_scan(
            &[path],
            &patterns,
            &rew_home_dir(),
            &scan_db,
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

        let scan_db = match rew_core::db::Database::open(&rew_home_dir().join("snapshots.db")) {
            Ok(db) => { let _ = db.initialize(); db }
            Err(e) => {
                tracing::warn!("Rescan: failed to open DB: {}", e);
                return;
            }
        };
        rew_core::scanner::full_scan(
            &[path],
            &patterns,
            &rew_home_dir(),
            &scan_db,
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
                let objs = rew_home_dir().join("objects");
                let _ = rew_core::reconcile::reconcile_task(&db, &old_window.task_id, &objs);
                let _ = db.refresh_task_rollup_from_changes(&old_window.task_id);
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

#[cfg(test)]
mod tests {
    use super::{
        infer_path_filter_mode, sync_file_index_after_directory_restore, PathFilterMode,
    };
    use chrono::Utc;
    use rew_core::restore::PreparedSuppressionEntry;
    use rew_core::db::Database;
    use rew_core::objects::sha256_file;
    use rew_core::types::{Change, ChangeType, Task, TaskStatus};
    use std::path::PathBuf;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        (dir, db)
    }

    fn seed_task(db: &Database, id: &str) {
        db.create_task(&Task {
            id: id.to_string(),
            prompt: None,
            tool: Some("文件监听".into()),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        })
        .unwrap();
    }

    #[test]
    fn infer_path_filter_mode_prefers_directory_when_descendants_exist() {
        let (_dir, db) = temp_db();
        seed_task(&db, "t_dir");

        db.insert_change(&Change {
            id: None,
            task_id: "t_dir".to_string(),
            file_path: PathBuf::from("/tmp/project/go/main.rs"),
            change_type: ChangeType::Deleted,
            old_hash: Some("sha-old".into()),
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();

        assert_eq!(
            infer_path_filter_mode(&db, "/tmp/project/go"),
            PathFilterMode::Directory
        );
    }

    #[test]
    fn infer_path_filter_mode_uses_exact_file_match_without_live_fs_hint() {
        let (_dir, db) = temp_db();
        seed_task(&db, "t_file");

        db.insert_change(&Change {
            id: None,
            task_id: "t_file".to_string(),
            file_path: PathBuf::from("/tmp/project/file.txt"),
            change_type: ChangeType::Modified,
            old_hash: Some("sha-old".into()),
            new_hash: Some("sha-new".into()),
            diff_text: None,
            lines_added: 1,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();

        assert_eq!(
            infer_path_filter_mode(&db, "/tmp/project/file.txt"),
            PathFilterMode::File
        );
    }

    #[test]
    fn directory_restore_rehydrates_live_file_index_entries() {
        let (dir, mut db) = temp_db();
        let file_path = dir.path().join("go/pkg/mod/example.txt");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, "original").unwrap();
        let original_hash = sha256_file(&file_path).unwrap();

        db.upsert_live_file_index_entry(
            &file_path.to_string_lossy(),
            1,
            "fast-original",
            Some(&original_hash),
            "test",
            "scan_seen",
            &Utc::now().to_rfc3339(),
            Some(1),
        )
        .unwrap();
        db.mark_file_index_deleted(
            &file_path.to_string_lossy(),
            Some(&original_hash),
            "test",
            "deleted",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();

        std::fs::write(&file_path, "original").unwrap();

        sync_file_index_after_directory_restore(
            &mut db,
            &[PreparedSuppressionEntry {
                path: file_path.clone(),
                expected_content_hash: Some(original_hash.clone()),
                deleted: false,
            }],
        );

        let entry = db
            .get_file_index_entry(&file_path.to_string_lossy())
            .unwrap()
            .unwrap();
        assert!(entry.exists_now);
        assert_eq!(entry.content_hash.as_deref(), Some(original_hash.as_str()));
        assert_eq!(
            db.list_live_files_under_dir(&dir.path().join("go"))
                .unwrap()
                .len(),
            1
        );
    }
}
