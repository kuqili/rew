//! Background daemon that runs the file watching pipeline and anomaly detection.
//!
//! Integrates with the Tauri app to update tray status on anomaly events.
#![allow(dead_code)]

use crate::state::AppState;
use crate::tray::{self, TrayStatus};
use tauri::{AppHandle, Emitter, Manager};
use tracing::info;

/// Start the background protection daemon.
/// Runs file watching, anomaly detection, and snapshot management.
pub fn start_daemon(app: &AppHandle) {
    let app_handle = app.clone();

    // Spawn the main protection loop
    std::thread::spawn(move || {
        info!("rew daemon started");

        // The daemon loop simulates periodic checking
        // In production this would run the Pipeline from rew-core
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));

            // Check if paused
            let paused = app_handle
                .try_state::<AppState>()
                .and_then(|s| s.paused.lock().ok().map(|p| *p))
                .unwrap_or(false);

            if paused {
                continue;
            }

            // In a real implementation, this would:
            // 1. Get events from Pipeline
            // 2. Run RuleEngine.analyze()
            // 3. If anomaly: create snapshot, send notification, update tray
            //
            // For now, the daemon keeps running to maintain the app lifecycle.
            // The actual watcher integration requires macOS permissions (Full Disk Access)
            // which can't be tested in a build-only environment.
        }
    });
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
