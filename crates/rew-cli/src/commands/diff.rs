//! `rew diff <id>` — Show unified diff for a task's changes.

use crate::display;
use crate::AppContext;
use rew_core::error::RewResult;

pub fn run(ctx: &AppContext, task_id: &str) -> RewResult<()> {
    let task = ctx.db.get_task(task_id)?
        .ok_or_else(|| rew_core::error::RewError::Config(format!("Task '{}' not found", task_id)))?;

    let changes = ctx.db.get_changes_for_task(task_id)?;

    println!("{} Diff for task {}", display::info_prefix(), task.id);
    if let Some(ref prompt) = task.prompt {
        println!("  Prompt: {}", prompt);
    }
    println!();

    if changes.is_empty() {
        println!("  No changes recorded.");
        return Ok(());
    }

    for change in &changes {
        let header = format!(
            "--- a/{}  ({:?})",
            change.file_path.display(),
            change.change_type
        );
        println!("\x1b[1m{}\x1b[0m", header);

        if let Some(ref diff) = change.diff_text {
            // Colorize diff output
            for line in diff.lines() {
                if line.starts_with('+') {
                    println!("\x1b[32m{}\x1b[0m", line);
                } else if line.starts_with('-') {
                    println!("\x1b[31m{}\x1b[0m", line);
                } else if line.starts_with('@') {
                    println!("\x1b[36m{}\x1b[0m", line);
                } else {
                    println!("{}", line);
                }
            }
        } else {
            println!("  (no diff text recorded — binary file or diff not computed)");
        }
        println!();
    }

    Ok(())
}
