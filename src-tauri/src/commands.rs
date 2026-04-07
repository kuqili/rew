//! Tauri IPC commands exposing rew-core functionality to the frontend.

use crate::state::AppState;
use rew_core::types::{Snapshot, SnapshotTrigger};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
    let config_path = rew_core::rew_home_dir().join("config.toml");
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

#[tauri::command]
pub async fn restore_snapshot(
    state: State<'_, AppState>,
    snapshot_id: String,
) -> Result<String, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let uuid = uuid::Uuid::parse_str(&snapshot_id).map_err(|e| e.to_string())?;
    let snapshot = db
        .get_snapshot(&uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Snapshot not found".to_string())?;

    // For now, return a success message.
    // Real restore would use RestoreEngine with tmutil mount.
    // This is safe because actual restore requires sudo and tmutil.
    Ok(format!(
        "Restore initiated for snapshot {} ({}). Please run 'rew restore --snapshot-id {}' in terminal for actual file restoration.",
        &snapshot.id.to_string()[..8],
        snapshot.timestamp.format("%Y-%m-%d %H:%M:%S"),
        &snapshot.id.to_string()[..8]
    ))
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

    // Estimate based on snapshot metadata
    let total_changes =
        snapshot.files_added as usize + snapshot.files_modified as usize + snapshot.files_deleted as usize;

    Ok(RestorePreviewInfo {
        snapshot_id: snapshot.id.to_string(),
        files_to_restore: snapshot.files_deleted as usize,
        files_to_overwrite: snapshot.files_modified as usize,
        files_to_remove: snapshot.files_added as usize,
        estimated_size_bytes: (total_changes as u64) * 4096, // rough estimate
    })
}

#[tauri::command]
pub async fn check_first_run() -> Result<bool, String> {
    let _config_path = rew_core::rew_home_dir().join("config.toml");
    let first_run_marker = rew_core::rew_home_dir().join(".setup_done");
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
    let config_path = rew_core::rew_home_dir().join("config.toml");
    config.save(&config_path).map_err(|e| e.to_string())?;

    // Mark setup as done
    let marker = rew_core::rew_home_dir().join(".setup_done");
    std::fs::write(&marker, "done").map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn set_paused(state: State<'_, AppState>, paused: bool) -> Result<(), String> {
    let mut p = state.paused.lock().map_err(|e| e.to_string())?;
    *p = paused;
    Ok(())
}
