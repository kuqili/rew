//! `rew status` — show protected directories and recent snapshots.

use crate::display;
use crate::AppContext;
use colored::*;
use rew_core::error::RewResult;

pub fn run(ctx: &AppContext) -> RewResult<()> {
    println!();
    println!("  {} v{}", "rew".bold().cyan(), env!("CARGO_PKG_VERSION"));
    println!("  {}", display::dim("AI 时代的文件安全网"));
    println!();

    // Protected directories
    println!("  {}", display::section("Protected Directories:"));
    if ctx.config.watch_dirs.is_empty() {
        println!("    {} No directories configured. Use `rew config add-dir <path>` to add one.",
            display::warning_prefix());
    } else {
        for dir in &ctx.config.watch_dirs {
            let exists = dir.exists();
            let (marker, color_dir) = if exists {
                ("✓".green().to_string(), dir.display().to_string().normal())
            } else {
                ("✗".red().to_string(), dir.display().to_string().dimmed())
            };
            println!("    {} {}", marker, color_dir);
        }
    }
    println!();

    // Recent snapshots (last 5)
    let snapshots = ctx.db.list_snapshots()?;
    let total = snapshots.len();
    let recent: Vec<_> = snapshots.into_iter().take(5).collect();

    println!("  {} ({} total)", display::section("Recent Snapshots:"), total);
    if recent.is_empty() {
        println!("    {} No snapshots yet.", display::dim("—"));
    } else {
        for s in &recent {
            let time = s.timestamp.format("%Y-%m-%d %H:%M:%S");
            let ago = display::time_ago(&s.timestamp);
            let trigger = display::trigger_label(&s.trigger);
            let pin = display::pin_icon(s.pinned);
            let changes = format!("+{}  ~{}  -{}",
                s.files_added.to_string().green(),
                s.files_modified.to_string().yellow(),
                s.files_deleted.to_string().red());
            println!("    {} {} {} {}  {} {}",
                display::dim(&s.id.to_string()[..8]),
                time.to_string().white(),
                display::dim(&format!("({})", ago)),
                trigger,
                changes,
                pin);
        }
        if total > 5 {
            println!("    {} Use `rew list` to see all {} snapshots.",
                display::dim("..."), total);
        }
    }
    println!();

    // Storage usage
    let db_path = ctx.rew_dir.join("snapshots.db");
    if db_path.exists() {
        if let Ok(meta) = std::fs::metadata(&db_path) {
            println!("  {} Database: {}", display::section("Storage:"),
                display::human_size(meta.len()));
        }
    }

    println!();
    Ok(())
}
