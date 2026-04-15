//! rew Tauri application — system tray + timeline GUI + setup wizard.

mod commands;
mod daemon;
mod state;
mod tray;

use rew_core::config::RewConfig;
use rew_core::db::Database;
use state::AppState;
use tauri::Manager;
use tracing::{info, warn};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logging to ~/.rew/logs/ with daily rotation
    let log_dir = rew_core::logging::log_dir();
    let _log_guard = match rew_core::logging::init_logging(&log_dir) {
        Ok(guard) => Some(guard),
        Err(_) => {
            // Fallback: basic stderr logging
            tracing_subscriber::fmt().init();
            None
        }
    };

    // Initialize rew core
    let (db, config) = match initialize_rew() {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to initialize rew: {}", e);
            // Fallback to defaults
            let db = Database::open(&rew_core::rew_home_dir().join("snapshots.db"))
                .expect("Failed to open database");
            db.initialize().expect("Failed to initialize database");
            (db, RewConfig::default())
        }
    };

    let app_state = AppState::new(db, config);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::list_snapshots,
            commands::get_status,
            commands::get_config,
            commands::update_config,
            commands::toggle_pin,
            commands::restore_snapshot,
            commands::get_restore_preview,
            commands::check_first_run,
            commands::get_home_dir,
            commands::complete_setup,
            commands::set_paused,
            // V2: Task commands
            commands::list_tasks,
            commands::get_task,
            commands::get_task_changes,
            commands::get_change_diff,
            // Rollback (V3 canonical names)
            commands::preview_rollback,
            commands::rollback_task_cmd,
            commands::restore_file_cmd,
            commands::restore_directory_cmd,
            commands::get_restore_progress,
            commands::list_restore_operations,
            // Legacy aliases
            commands::preview_undo,
            commands::undo_task_cmd,
            commands::undo_file_cmd,
            // V3: Scan progress + directory management
            commands::get_scan_progress,
            commands::add_watch_dir,
            commands::remove_watch_dir,
            commands::update_ignore_config,
            commands::get_ignore_config,
            commands::get_storage_info,
            commands::analyze_directories,
            // Per-directory ignore config
            commands::get_dir_ignore_config,
            commands::update_dir_ignore_config,
            commands::list_subdirs,
            commands::list_dir_contents,
            commands::rescan_watch_dir,
            // Monitoring window config
            commands::get_monitoring_window,
            commands::set_monitoring_window,
            // Manual snapshot
            commands::create_manual_snapshot,
            // AI tool hook management
            commands::detect_ai_tools,
            commands::install_tool_hook,
            commands::uninstall_tool_hook,
            // Task statistics
            commands::get_task_stats,
            commands::get_insights,
        ])
        .setup(|app| {
            // Install CLI binary from bundled resources to ~/.rew/bin/rew
            install_cli_binary(app.handle());

            // Set up system tray
            tray::setup_tray(app.handle())?;

            // Start background daemon
            daemon::start_daemon(app.handle());

            // Background timer: proactively seal monitoring windows that have
            // exceeded their configured duration, instead of waiting for the
            // next file event to trigger the check. Checks every 10 seconds;
            // seals the window at exactly started_at + window_secs.
            {
                let handle_for_sealer = app.handle().clone();
                std::thread::spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(10));

                        let state: tauri::State<AppState> = handle_for_sealer.state();
                        let now = chrono::Utc::now();

                        let window_secs = state.config
                            .lock()
                            .map(|c| c.monitoring_window_secs)
                            .unwrap_or(600);

                        // Determine if the current window has expired
                        let expired = {
                            let guard = match state.fs_window_task.lock() {
                                Ok(g) => g,
                                Err(_) => continue,
                            };
                            guard.as_ref().and_then(|w| {
                                let elapsed = (now - w.started_at).num_seconds() as u64;
                                if elapsed >= window_secs {
                                    // Ideal seal time = started_at + window_secs.
                                    // Clamp to `now` in case window_secs was reduced
                                    // after the window opened (would produce a past ts).
                                    let ideal = w.started_at
                                        + chrono::Duration::seconds(window_secs as i64);
                                    let seal_time = ideal.min(now);
                                    Some((w.task_id.clone(), seal_time))
                                } else {
                                    None
                                }
                            })
                        };

                        if let Some((task_id, seal_time)) = expired {
                            // Clear the active window so the next event starts fresh
                            if let Ok(mut guard) = state.fs_window_task.lock() {
                                if guard.as_ref().map(|w| w.task_id == task_id).unwrap_or(false) {
                                    *guard = None;
                                }
                            }
                            // Write the precise seal timestamp to DB + reconcile
                            if let Ok(db) = state.db.lock() {
                                let _ = db.update_task_completed_at(&task_id, seal_time);
                                let objs = rew_core::rew_home_dir().join("objects");
                                let _ = rew_core::reconcile::reconcile_task(&db, &task_id, &objs);
                            }
                        }

                        // Periodically clean up stale AI tasks.
                        // This handles cases where Claude Code / Cursor crashed or
                        // the system slept while an AI task was active, leaving
                        // `.current_task` on disk and the DB row stuck in "active".
                        // We check the mtime of the marker file: if it hasn't been
                        // touched for more than 30 minutes, the session is gone.
                        {
                            let rew_dir = rew_core::rew_home_dir();
                            let marker = rew_dir.join(".current_task");
                            if marker.exists() {
                                let stale = std::fs::metadata(&marker)
                                    .and_then(|m| m.modified())
                                    .map(|t| t.elapsed().unwrap_or_default().as_secs() > 1800)
                                    .unwrap_or(false);
                                if stale {
                                    tracing::info!(
                                        "Stale .current_task detected (>30 min old), cleaning up"
                                    );
                                    let _ = std::fs::remove_file(&marker);
                                    let _ = std::fs::remove_file(rew_dir.join(".current_session"));
                                    if let Ok(db) = state.db.lock() {
                                        let _ = db.recover_stale_ai_tasks(now);
                                    }
                                }
                            }
                        }
                    }
                });
            }

            // Window starts hidden (set in tauri.conf.json),
            // frontend will show it after checking first-run state
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Minimize to tray instead of closing
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running rew");
}

/// Copy the bundled `rew` CLI binary from the .app resources to ~/.rew/bin/rew.
/// In dev mode (resource not found), falls back to the cargo build output.
fn install_cli_binary(app: &tauri::AppHandle) {
    let dest = rew_core::rew_cli_bin_path();

    // Resolve source: try Tauri resource first, then dev-mode fallback
    let source = app
        .path()
        .resource_dir()
        .ok()
        .map(|d| d.join("rew"))
        .filter(|p| p.exists())
        .or_else(|| {
            // Dev-mode fallback: look in cargo target directory
            let candidates = [
                std::env::current_dir().ok().map(|d| d.join("target/release/rew")),
                std::env::current_dir().ok().map(|d| d.join("target/debug/rew")),
            ];
            candidates.into_iter().flatten().find(|p| p.exists())
        });

    let source = match source {
        Some(s) => s,
        None => {
            warn!("rew CLI binary not found in resources or target/, skipping install");
            return;
        }
    };

    // Skip if dest exists and size matches AND content matches.
    // We compare sizes first as a fast pre-check, then fall back to full content
    // comparison only when sizes are equal (catches same-size but different-code binaries).
    if dest.exists() {
        let src_size = std::fs::metadata(&source).map(|m| m.len()).ok();
        let dst_size = std::fs::metadata(&dest).map(|m| m.len()).ok();
        if src_size != dst_size {
            // sizes differ → definitely need to update, fall through
        } else {
            // same size → compare content to catch code-only changes
            let src_bytes = std::fs::read(&source).ok();
            let dst_bytes = std::fs::read(&dest).ok();
            if src_bytes.is_some() && src_bytes == dst_bytes {
                return; // identical, skip
            }
        }
    }

    // Ensure ~/.rew/bin/ exists
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!("Failed to create {:?}: {}", parent, e);
            return;
        }
    }

    match std::fs::copy(&source, &dest) {
        Ok(_) => {
            // Ensure executable permission
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
            }
            info!("Installed rew CLI to {:?}", dest);
        }
        Err(e) => {
            warn!("Failed to install rew CLI: {}", e);
        }
    }
}


fn initialize_rew() -> Result<(Database, RewConfig), rew_core::error::RewError> {
    let rew_dir = rew_core::rew_home_dir();
    std::fs::create_dir_all(&rew_dir)?;

    // Load or create config
    let config_path = rew_dir.join("config.toml");
    let config = if config_path.exists() {
        RewConfig::load(&config_path)?
    } else {
        let default_config = RewConfig::default();
        default_config.save(&config_path)?;
        default_config
    };

    // Initialize database
    let db_path = rew_dir.join("snapshots.db");
    let db = Database::open(&db_path)?;
    db.initialize()?;

    Ok((db, config))
}
