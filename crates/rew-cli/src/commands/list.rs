//! `rew list` — list all tasks and snapshots.

use crate::display;
use crate::AppContext;
use colored::*;
use rew_core::error::RewResult;

pub fn run(ctx: &AppContext) -> RewResult<()> {
    // Show tasks first (V2 primary view)
    let tasks = ctx.db.list_tasks()?;

    if !tasks.is_empty() {
        println!();
        println!("  {} ({} total)", display::section("Tasks"), tasks.len());
        println!();

        println!(
            "  {:<12} {:<20} {:<14} {:<12} {}",
            "ID".bold().underline(),
            "Time".bold().underline(),
            "Status".bold().underline(),
            "Tool".bold().underline(),
            "Prompt".bold().underline(),
        );

        for tw in &tasks {
            let t = &tw.task;
            let time = t.started_at.format("%Y-%m-%d %H:%M");
            let status = match t.status {
                rew_core::types::TaskStatus::Active => "active".yellow(),
                rew_core::types::TaskStatus::Completed => "completed".green(),
                rew_core::types::TaskStatus::RolledBack => "rolled-back".red(),
                rew_core::types::TaskStatus::PartialRolledBack => "partial".magenta(),
            };
            let tool = t.tool.as_deref().unwrap_or("-");
            let prompt = t
                .prompt
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(40)
                .collect::<String>();

            let change_summary = if tw.changes_count == 0 {
                String::new()
            } else {
                format!(" ({})", t.summary.as_deref().unwrap_or(&format!("{} files", tw.changes_count)))
            };

            println!(
                "  {:<12} {:<20} {:<14} {:<12} {}{}",
                display::dim(&t.id),
                time,
                status,
                tool,
                prompt,
                display::dim(&change_summary),
            );
        }
        println!();
    }

    // Show snapshots
    let snapshots = ctx.db.list_snapshots()?;

    if snapshots.is_empty() && tasks.is_empty() {
        println!();
        println!("  No tasks or snapshots yet.");
        println!("  Tasks are created when AI tools run with rew hooks installed.");
        println!("  Snapshots are created automatically when file changes are detected.");
        println!();
        return Ok(());
    }

    if !snapshots.is_empty() {
        println!("  {} ({} total)", display::section("Snapshots"), snapshots.len());
        println!();

        println!("  {:<10} {:<20} {:<12} {:>5} {:>5} {:>5}  {}",
            "ID".bold().underline(),
            "Time".bold().underline(),
            "Trigger".bold().underline(),
            "+Add".bold().underline(),
            "~Mod".bold().underline(),
            "-Del".bold().underline(),
            "Pin".bold().underline(),
        );

        for s in &snapshots {
            let short_id = &s.id.to_string()[..8];
            let time = s.timestamp.format("%Y-%m-%d %H:%M:%S");
            let trigger = display::trigger_label(&s.trigger);
            let pin = display::pin_icon(s.pinned);

            println!("  {:<10} {:<20} {:<12} {:>5} {:>5} {:>5}  {}",
                display::dim(short_id),
                time,
                trigger,
                s.files_added.to_string().green(),
                s.files_modified.to_string().yellow(),
                s.files_deleted.to_string().red(),
                pin,
            );
        }

        println!();
    }

    println!("  Use {} to see task details", "rew show <id>".cyan());
    println!("  Use {} to undo a task", "rew undo <id>".cyan());
    println!();

    Ok(())
}
