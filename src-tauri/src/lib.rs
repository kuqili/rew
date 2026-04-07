use rew_core::config::RewConfig;
use rew_core::db::Database;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize rew core on startup
    if let Err(e) = initialize_rew() {
        eprintln!("Failed to initialize rew: {}", e);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn initialize_rew() -> Result<(), rew_core::error::RewError> {
    // Ensure ~/.rew/ directory exists
    let rew_dir = rew_core::rew_home_dir();
    std::fs::create_dir_all(&rew_dir).map_err(|e| {
        rew_core::error::RewError::Io(e)
    })?;

    // Initialize default config if not present
    let config_path = rew_dir.join("config.toml");
    if !config_path.exists() {
        let default_config = RewConfig::default();
        default_config.save(&config_path)?;
    }

    // Initialize database
    let db_path = rew_dir.join("snapshots.db");
    let db = Database::open(&db_path)?;
    db.initialize()?;

    Ok(())
}
