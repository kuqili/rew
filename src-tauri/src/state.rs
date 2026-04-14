//! Shared application state managed by Tauri.

use chrono::{DateTime, Utc};
use rew_core::config::RewConfig;
use rew_core::db::Database;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Commands sent from Tauri commands → running daemon pipeline.
pub enum PipelineCmd {
    AddPath(PathBuf),
    RemovePath(PathBuf),
}

/// Tracks the currently-open file-monitoring time window.
///
/// Multiple 30-second event batches are merged into a single timeline Task
/// until the window expires (`now - started_at > monitoring_window_secs`).
#[derive(Debug, Clone)]
pub struct FsWindowTask {
    /// Task ID of the open window in the DB
    pub task_id: String,
    /// When the first event in this window arrived
    pub started_at: DateTime<Utc>,
}

/// Per-directory scan completion status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DirStatus {
    Pending,
    Scanning,
    Complete,
}

/// Per-directory scan statistics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirScanStatus {
    pub path: String,
    pub status: DirStatus,
    /// Estimated total files in this directory (from pre-count).
    pub files_total: usize,
    /// Files processed so far.
    pub files_done: usize,
    pub files_stored: usize,
    pub files_skipped: usize,
    pub files_failed: usize,
    /// ISO8601 timestamp of last completed scan, None if never finished.
    pub last_completed_at: Option<String>,
}

/// Overall scan progress — shared between scanner thread and IPC handlers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ScanProgress {
    pub is_scanning: bool,
    pub current_dir: Option<String>,
    pub dirs: Vec<DirScanStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RestorePhase {
    #[default]
    Idle,
    RestoringFiles,
    SyncingDatabase,
    Finalizing,
    Done,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct RestoreProgress {
    pub is_running: bool,
    pub phase: RestorePhase,
    pub task_id: Option<String>,
    pub dir_path: Option<String>,
    pub total_files: usize,
    pub processed_files: usize,
    pub restored_files: usize,
    pub deleted_files: usize,
    pub failed_files: usize,
    pub current_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SuppressedRestorePath {
    pub created_at: Instant,
    pub expected_content_hash: Option<String>,
    pub deleted: bool,
}

/// Application state shared across Tauri commands and the tray.
pub struct AppState {
    /// SQLite database handle.
    pub db: Mutex<Database>,
    /// Current configuration.
    pub config: Mutex<RewConfig>,
    /// Whether protection is paused.
    pub paused: Mutex<bool>,
    /// Whether there's an active warning (anomaly detected).
    pub has_warning: Mutex<bool>,
    /// Live scan progress — Arc so scanner thread can update independently.
    pub scan_progress: Arc<Mutex<ScanProgress>>,
    /// Currently-open file-monitoring time window (None if no events yet).
    pub fs_window_task: Mutex<Option<FsWindowTask>>,
    /// Set to true while a rollback is executing to suppress the file events
    /// it generates from being recorded as new timeline entries.
    pub rolling_back: Mutex<bool>,
    /// Path-level suppression: file paths recently written by GUI operations
    /// (single-file restore or task rollback). The daemon skips FSEvents for
    /// these paths for 60 seconds after the GUI write, regardless of the global
    /// `rolling_back` flag. This handles async FSEvent delivery that arrives
    /// after `rolling_back` has already been cleared.
    pub suppressed_paths: Mutex<HashMap<PathBuf, Instant>>,
    /// Restore-result suppression: used by directory rollback to suppress only
    /// the exact restored/deleted paths instead of blanketing an entire prefix.
    /// For restored files, we keep the expected post-restore content hash so
    /// later real user edits can pass through once the content diverges.
    pub suppressed_restore_paths: Mutex<HashMap<PathBuf, SuppressedRestorePath>>,
    /// Live directory-restore progress for large rollback operations.
    pub restore_progress: Arc<Mutex<RestoreProgress>>,
    /// Channel to send hot-update commands (add/remove path) to the running daemon pipeline.
    /// Set by the daemon after the pipeline starts; None if daemon not yet running.
    pub pipeline_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<PipelineCmd>>>,
}

impl AppState {
    pub fn new(db: Database, config: RewConfig) -> Self {
        // Try to load persisted scan status from disk
        let scan_progress = load_scan_status();

        Self {
            db: Mutex::new(db),
            config: Mutex::new(config),
            paused: Mutex::new(false),
            has_warning: Mutex::new(false),
            scan_progress: Arc::new(Mutex::new(scan_progress)),
            fs_window_task: Mutex::new(None),
            rolling_back: Mutex::new(false),
            suppressed_paths: Mutex::new(HashMap::new()),
            suppressed_restore_paths: Mutex::new(HashMap::new()),
            restore_progress: Arc::new(Mutex::new(RestoreProgress::default())),
            pipeline_tx: Mutex::new(None),
        }
    }
}

/// Load persisted scan status from `~/.rew/.scan_status.json`.
fn load_scan_status() -> ScanProgress {
    let path = rew_core::rew_home_dir().join(".scan_status.json");
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => ScanProgress::default(),
    }
}

/// Save scan status to `~/.rew/.scan_status.json`.
pub fn save_scan_status(progress: &ScanProgress) {
    let path = rew_core::rew_home_dir().join(".scan_status.json");
    if let Ok(json) = serde_json::to_string_pretty(progress) {
        let _ = std::fs::write(&path, json);
    }
}
