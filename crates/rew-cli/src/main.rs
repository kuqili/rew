//! rew CLI — command-line interface for rew file safety net.
//!
//! Usage:
//!   rew status   - Show running status, protected dirs, recent snapshots
//!   rew restore  - Interactive snapshot restore
//!   rew config   - Manage configuration
//!   rew list     - List snapshots with trigger/pin icons
//!   rew pin      - Pin/unpin a snapshot

mod commands;
mod display;

use clap::{Parser, Subcommand};
use rew_core::config::RewConfig;
use rew_core::db::Database;
use rew_core::error::RewResult;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "rew",
    about = "rew — AI 时代的文件安全网",
    version,
    long_about = "rew 自动监控文件变更，检测异常操作，并通过 APFS 快照保护你的重要数据。\n轻松恢复误删、误改的文件，让你的数字资产始终安全。"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show running status, protected directories, and recent snapshots
    Status,

    /// List all snapshots with trigger type and pin status
    List,

    /// Restore files from a snapshot
    Restore {
        /// Snapshot ID to restore from (skip interactive selection)
        #[arg(long)]
        snapshot_id: Option<String>,

        /// Skip confirmation prompt (for scripting)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Manage configuration (watch directories, settings)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Pin or unpin a snapshot for permanent retention
    Pin {
        /// Snapshot ID to pin/unpin
        snapshot_id: String,

        /// Unpin instead of pin
        #[arg(long)]
        unpin: bool,
    },

    /// Show details of a specific task (files changed, diff stats)
    Show {
        /// Task ID to show
        id: String,
    },

    /// Show unified diff for a task's changes
    Diff {
        /// Task ID to show diff for
        id: String,
    },

    /// Undo a task (restore files to pre-task state)
    Undo {
        /// Task ID to undo (if omitted, undo the most recent task)
        id: Option<String>,

        /// Only undo a specific file within the task
        #[arg(long)]
        file: Option<String>,

        /// Preview what would be undone without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Initialize rew (create config and database)
    Init,

    /// Install LaunchAgent for auto-start on login
    Install,

    /// Remove LaunchAgent (disable auto-start)
    Uninstall,

    /// Run the rew daemon in the foreground
    Daemon,

    /// Hook commands (called by AI tools, not for direct use)
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Record user prompt and create a new task (UserPromptSubmit hook)
    Prompt,
    /// Check scope + backup before AI writes (PreToolUse hook)
    PreTool,
    /// Record change after AI operation (PostToolUse hook)
    PostTool,
    /// Close current task (Stop hook)
    Stop,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,

    /// Add a directory to watch
    AddDir {
        /// Path to the directory to add
        path: String,
    },

    /// Remove a directory from watch list
    RemoveDir {
        /// Path to the directory to remove
        path: String,
    },

    /// Reset configuration to defaults
    Reset,
}

/// Shared app context passed to all commands.
struct AppContext {
    db: Database,
    config: RewConfig,
    config_path: PathBuf,
    rew_dir: PathBuf,
}

impl AppContext {
    fn init() -> RewResult<Self> {
        let rew_dir = rew_core::rew_home_dir();
        std::fs::create_dir_all(&rew_dir)?;

        let config_path = rew_dir.join("config.toml");
        if !config_path.exists() {
            let default_config = RewConfig::default();
            default_config.save(&config_path)?;
        }

        let config = RewConfig::load(&config_path)?;

        let db_path = rew_dir.join("snapshots.db");
        let db = Database::open(&db_path)?;
        db.initialize()?;

        Ok(AppContext {
            db,
            config,
            config_path,
            rew_dir,
        })
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{} {}", display::error_prefix(), e);
        std::process::exit(1);
    }
}

fn run() -> RewResult<()> {
    let cli = Cli::parse();

    // These commands don't need AppContext
    match &cli.command {
        Some(Commands::Install) => return commands::install::install(),
        Some(Commands::Uninstall) => return commands::install::uninstall(),
        Some(Commands::Daemon) => return commands::daemon::run(),
        Some(Commands::Hook { action }) => {
            match action {
                HookAction::Prompt => {
                    commands::hook::handle_prompt()?;
                    return Ok(());
                }
                HookAction::PreTool => {
                    let exit_code = commands::hook::handle_pre_tool()?;
                    if exit_code != 0 {
                        std::process::exit(exit_code);
                    }
                    return Ok(());
                }
                HookAction::PostTool => {
                    commands::hook::handle_post_tool()?;
                    return Ok(());
                }
                HookAction::Stop => {
                    commands::hook::handle_stop()?;
                    return Ok(());
                }
            }
        }
        _ => {}
    }

    let ctx = AppContext::init()?;

    match cli.command {
        Some(Commands::Status) | None => commands::status::run(&ctx),
        Some(Commands::List) => commands::list::run(&ctx),
        Some(Commands::Restore { snapshot_id, yes }) => {
            commands::restore::run(&ctx, snapshot_id, yes)
        }
        Some(Commands::Config { action }) => match action {
            ConfigAction::Show => commands::config::show(&ctx),
            ConfigAction::AddDir { path } => commands::config::add_dir(&ctx, &path),
            ConfigAction::RemoveDir { path } => commands::config::remove_dir(&ctx, &path),
            ConfigAction::Reset => commands::config::reset(&ctx),
        },
        Some(Commands::Pin { snapshot_id, unpin }) => {
            commands::pin::run(&ctx, &snapshot_id, unpin)
        }
        Some(Commands::Show { id }) => commands::show::run(&ctx, &id),
        Some(Commands::Diff { id }) => commands::diff::run(&ctx, &id),
        Some(Commands::Undo { id, file, dry_run, yes }) => {
            commands::undo::run(&ctx, id.as_deref(), file.as_deref(), dry_run, yes)
        }
        Some(Commands::Init) => {
            commands::init::run(&ctx);
            Ok(())
        }
        // Already handled above
        Some(Commands::Install)
        | Some(Commands::Uninstall)
        | Some(Commands::Daemon)
        | Some(Commands::Hook { .. }) => {
            unreachable!()
        }
    }
}
