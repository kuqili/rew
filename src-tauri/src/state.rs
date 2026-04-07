//! Shared application state managed by Tauri.

use rew_core::config::RewConfig;
use rew_core::db::Database;
use std::sync::Mutex;

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
}

impl AppState {
    pub fn new(db: Database, config: RewConfig) -> Self {
        Self {
            db: Mutex::new(db),
            config: Mutex::new(config),
            paused: Mutex::new(false),
            has_warning: Mutex::new(false),
        }
    }
}
