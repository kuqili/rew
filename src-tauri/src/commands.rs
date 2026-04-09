//! Tauri IPC commands exposing rew-core functionality to the frontend.

use crate::state::AppState;
use rew_core::restore::TaskRestoreEngine;
use rew_core::types::{Snapshot, SnapshotTrigger};
use rew_core::rew_home_dir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

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
pub async fn complete_setup(
    state: State<'_, AppState>,
    watch_dirs: Vec<String>,
) -> Result<(), String> {
    // Save config with selected dirs
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.watch_dirs = watch_dirs.into_iter().map(PathBuf::from).collect();
    let config_path = rew_home_dir().join("config.toml");
    config.save(&config_path).map_err(|e| e.to_string())?;

    // Mark setup as done
    let marker = rew_home_dir().join(".setup_done");
    std::fs::write(&marker, "done").map_err(|e| e.to_string())?;

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
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Vec<TaskInfo>, String> {
    let db = state.db.lock().map_err(|e| format!("DB lock error: {}", e))?;
    let tasks = db.list_tasks().map_err(|e| format!("list_tasks error: {}", e))?;

    tracing::info!("list_tasks: found {} tasks in DB", tasks.len());

    let mut result = Vec::new();
    for task in tasks {
        let changes_count = db.get_changes_for_task(&task.id)
            .map(|c| c.len())
            .unwrap_or(0);

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
    })
}

#[tauri::command]
pub async fn get_task_changes(state: State<'_, AppState>, task_id: String) -> Result<Vec<ChangeInfo>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let changes = db.get_changes_for_task(&task_id).map_err(|e| e.to_string())?;

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
    }).collect())
}

#[tauri::command]
pub async fn preview_undo(state: State<'_, AppState>, task_id: String) -> Result<UndoPreviewInfo, String> {
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
pub async fn undo_task_cmd(state: State<'_, AppState>, task_id: String) -> Result<UndoResultInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let objects_root = rew_home_dir().join("objects");
    let engine = TaskRestoreEngine::new(objects_root);

    let result = engine.undo_task(&db, &task_id).map_err(|e| e.to_string())?;

    Ok(UndoResultInfo {
        files_restored: result.files_restored,
        files_deleted: result.files_deleted,
        failures: result.failures.iter().map(|(p, e)| (p.to_string_lossy().to_string(), e.clone())).collect(),
    })
}

#[tauri::command]
pub async fn undo_file_cmd(
    state: State<'_, AppState>,
    task_id: String,
    file_path: String,
) -> Result<UndoResultInfo, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let objects_root = rew_home_dir().join("objects");
    let engine = TaskRestoreEngine::new(objects_root);

    let result = engine.undo_file(&db, &task_id, &PathBuf::from(&file_path))
        .map_err(|e| e.to_string())?;

    Ok(UndoResultInfo {
        files_restored: result.files_restored,
        files_deleted: result.files_deleted,
        failures: result.failures.iter().map(|(p, e)| (p.to_string_lossy().to_string(), e.clone())).collect(),
    })
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
    state: State<'_, AppState>,
    dir_path: String,
) -> Result<(), String> {
    let path = PathBuf::from(&dir_path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("目录不存在: {}", dir_path));
    }

    // Update config
    {
        let mut config = state.config.lock().map_err(|e| e.to_string())?;
        if config.watch_dirs.contains(&path) {
            return Ok(());
        }
        config.watch_dirs.push(path.clone());
        config.save(&rew_home_dir().join("config.toml")).map_err(|e| e.to_string())?;
    }

    // Add to scan progress as "pending"
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
    }

    // Kick off background scan for this directory
    let sp = state.scan_progress.clone();
    let config = state.config.lock().map_err(|e| e.to_string())?.clone();
    std::thread::spawn(move || {
        let sp2 = sp.clone();
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
            }
        });

        rew_core::scanner::full_scan(
            &[path],
            &config.ignore_patterns,
            &rew_home_dir(),
            Some(callback),
            None,
            config.max_file_size_bytes,
        );

        // Mark complete
        if let Ok(mut progress) = sp.lock() {
            if let Some(ds) = progress.dirs.iter_mut().find(|d| d.path == dir_path) {
                ds.status = crate::state::DirStatus::Complete;
                ds.last_completed_at = Some(chrono::Utc::now().to_rfc3339());
            }
            crate::state::save_scan_status(&progress);
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn remove_watch_dir(
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

    // Remove from scan progress
    {
        let mut progress = state.scan_progress.lock().map_err(|e| e.to_string())?;
        progress.dirs.retain(|d| d.path != dir_path);
        crate::state::save_scan_status(&progress);
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

        let mut total_files = 0usize;
        let mut total_bytes = 0u64;
        let mut large_count = 0usize;
        let mut large_bytes = 0u64;

        let mut stack = vec![dir_path.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();

                // Use same filter as scanner
                if filter.should_ignore(&path) { continue; }

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
            categories: vec![], // Simplified — no per-category breakdown needed anymore
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
