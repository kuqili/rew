//! rew Tauri application — system tray + timeline GUI + setup wizard.

mod commands;
mod daemon;
mod state;
mod tray;

use rew_core::config::RewConfig;
use rew_core::db::Database;
use state::AppState;
use tauri::Manager;

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
        ])
        .setup(|app| {
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
                            // Write the precise seal timestamp to DB
                            if let Ok(db) = state.db.lock() {
                                let _ = db.update_task_completed_at(&task_id, seal_time);
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
