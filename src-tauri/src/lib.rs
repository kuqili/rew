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
            commands::complete_setup,
            commands::set_paused,
            // V2: Task commands
            commands::list_tasks,
            commands::get_task,
            commands::get_task_changes,
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
        ])
        .setup(|app| {
            // Set up system tray
            tray::setup_tray(app.handle())?;

            // Start background daemon
            daemon::start_daemon(app.handle());

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
