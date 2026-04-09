//! `rew show <id>` — Show details of a specific task.

use crate::display;
use crate::AppContext;
use rew_core::error::RewResult;

pub fn run(ctx: &AppContext, task_id: &str) -> RewResult<()> {
    let task = ctx.db.get_task(task_id)?
        .ok_or_else(|| rew_core::error::RewError::Config(format!("Task '{}' not found", task_id)))?;

    let changes = ctx.db.get_changes_for_task(task_id)?;

    // Header
    println!("{} Task {}", display::info_prefix(), task.id);
    println!();

    if let Some(ref prompt) = task.prompt {
        println!("  Prompt:  {}", prompt);
    }
    if let Some(ref tool) = task.tool {
        println!("  Tool:    {}", tool);
    }
    println!("  Started: {}", task.started_at.format("%Y-%m-%d %H:%M:%S"));
    if let Some(ref completed) = task.completed_at {
        println!("  Ended:   {}", completed.format("%Y-%m-%d %H:%M:%S"));
    }
    println!("  Status:  {}", task.status);
    if let Some(ref risk) = task.risk_level {
        println!("  Risk:    {}", risk);
    }
    if let Some(ref summary) = task.summary {
        println!("  Summary: {}", summary);
    }
    println!();

    if changes.is_empty() {
        println!("  No file changes recorded.");
        return Ok(());
    }

    println!("  {} file change(s):", changes.len());
    println!();

    for change in &changes {
        let icon = match change.change_type {
            rew_core::types::ChangeType::Created => "\x1b[32m+\x1b[0m",  // green +
            rew_core::types::ChangeType::Modified => "\x1b[33m~\x1b[0m",  // yellow ~
            rew_core::types::ChangeType::Deleted => "\x1b[31m-\x1b[0m",   // red -
            rew_core::types::ChangeType::Renamed => "\x1b[36m>\x1b[0m",   // cyan >
        };

        let stats = if change.lines_added > 0 || change.lines_removed > 0 {
            format!("  +{} -{}", change.lines_added, change.lines_removed)
        } else {
            String::new()
        };

        println!("    {} {}{}", icon, change.file_path.display(), stats);
    }

    Ok(())
}
