//! `rew init` — initialize rew configuration and database.

use crate::display;
use crate::AppContext;
use colored::*;

pub fn run(ctx: &AppContext) {
    println!();
    println!("  {} rew initialized successfully!", display::success_prefix());
    println!();
    println!("  {} {}", "Config:".bold(), ctx.config_path.display());
    println!("  {} {}", "Database:".bold(), ctx.rew_dir.join("snapshots.db").display());
    println!();
    println!("  Default watch directories:");
    for dir in &ctx.config.watch_dirs {
        println!("    · {}", dir.display());
    }
    println!();
    println!("  Use `rew config add-dir <path>` to add more directories.");
    println!("  Use `rew status` to check the current state.");
    println!();
}
