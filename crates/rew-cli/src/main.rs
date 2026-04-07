//! rew CLI — command-line interface for rew file safety net.
//!
//! Usage:
//!   rew status   - Show running status and snapshot list
//!   rew restore  - Interactive snapshot restore
//!   rew config   - Manage configuration
//!   rew list     - List snapshots
//!   rew pin      - Pin/unpin a snapshot

use rew_core::config::RewConfig;
use rew_core::db::Database;

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), rew_core::error::RewError> {
    let rew_dir = rew_core::rew_home_dir();
    std::fs::create_dir_all(&rew_dir)?;

    // Initialize config if needed
    let config_path = rew_dir.join("config.toml");
    if !config_path.exists() {
        let default_config = RewConfig::default();
        default_config.save(&config_path)?;
        println!("Created default config at {}", config_path.display());
    }

    // Initialize database
    let db_path = rew_dir.join("snapshots.db");
    let db = Database::open(&db_path)?;
    db.initialize()?;

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    match command {
        "status" => cmd_status(&db, &config_path)?,
        "list" => cmd_list(&db)?,
        "init" => {
            println!("rew initialized successfully.");
            println!("  Config: {}", config_path.display());
            println!("  Database: {}", db_path.display());
        }
        _ => {
            println!("rew — AI 时代的文件安全网");
            println!();
            println!("Usage:");
            println!("  rew status   Show running status");
            println!("  rew list     List all snapshots");
            println!("  rew init     Initialize rew");
            println!("  rew help     Show this help message");
        }
    }

    Ok(())
}

fn cmd_status(db: &Database, config_path: &std::path::Path) -> Result<(), rew_core::error::RewError> {
    let config = RewConfig::load(config_path)?;

    println!("rew v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Watch directories:");
    for dir in &config.watch_dirs {
        let exists = dir.exists();
        let marker = if exists { "✓" } else { "✗" };
        println!("  {} {}", marker, dir.display());
    }

    let snapshots = db.list_snapshots()?;
    println!();
    println!("Snapshots: {} total", snapshots.len());

    if let Some(latest) = snapshots.first() {
        println!("  Latest: {} ({})", latest.timestamp.format("%Y-%m-%d %H:%M:%S"), latest.trigger);
    }

    Ok(())
}

fn cmd_list(db: &Database) -> Result<(), rew_core::error::RewError> {
    let snapshots = db.list_snapshots()?;

    if snapshots.is_empty() {
        println!("No snapshots yet.");
        return Ok(());
    }

    println!("{:<36}  {:<20}  {:<8}  {:>5} {:>5} {:>5}  {}",
        "ID", "Time", "Trigger", "+Add", "~Mod", "-Del", "Pin");
    println!("{}", "-".repeat(100));

    for s in &snapshots {
        let pin_mark = if s.pinned { "📌" } else { "" };
        println!("{:<36}  {:<20}  {:<8}  {:>5} {:>5} {:>5}  {}",
            s.id,
            s.timestamp.format("%Y-%m-%d %H:%M:%S"),
            s.trigger,
            s.files_added,
            s.files_modified,
            s.files_deleted,
            pin_mark,
        );
    }

    Ok(())
}
