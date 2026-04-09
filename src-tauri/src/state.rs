//! Shared application state managed by Tauri.

use rew_core::config::RewConfig;
use rew_core::db::Database;
use std::sync::{Arc, Mutex};

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
