//! `rew undo [id]` — Undo a task (restore files to pre-task state).

use crate::display;
use crate::AppContext;
use rew_core::error::RewResult;
use rew_core::restore::TaskRestoreEngine;
use std::path::PathBuf;

pub fn run(
    ctx: &AppContext,
    task_id: Option<&str>,
    file_path: Option<&str>,
    dry_run: bool,
    yes: bool,
) -> RewResult<()> {
    let objects_root = ctx.rew_dir.join("objects");
    let engine = TaskRestoreEngine::new(objects_root);

    // If no task ID given, use the most recent task
    let id = match task_id {
        Some(id) => id.to_string(),
        None => {
            let tasks = ctx.db.list_tasks()?;
            let recent = tasks.first().ok_or_else(|| {
                rew_core::error::RewError::Config("No tasks found. Nothing to undo.".to_string())
            })?;
            recent.id.clone()
        }
    };

    // Preview first
    let preview = engine.preview_undo(&ctx.db, &id)?;

    println!("{} Undo preview for task {}", display::info_prefix(), id);

    // Show task info
    if let Ok(Some(task)) = ctx.db.get_task(&id) {
        if let Some(ref prompt) = task.prompt {
            println!("  Prompt: {}", prompt);
        }
        println!("  Status: {}", task.status);
    }
    println!();

    if preview.total_changes == 0 {
        println!("  No changes to undo.");
        return Ok(());
    }

    if !preview.files_to_restore.is_empty() {
        println!("  Files to restore ({}):", preview.files_to_restore.len());
        for f in &preview.files_to_restore {
            println!("    \x1b[33m~\x1b[0m {}", f.display());
        }
    }
    if !preview.files_to_delete.is_empty() {
        println!("  Files to delete ({}):", preview.files_to_delete.len());
        for f in &preview.files_to_delete {
            println!("    \x1b[31m-\x1b[0m {}", f.display());
        }
    }
    println!();

    if dry_run {
        println!("  (dry-run mode — no changes made)");
        return Ok(());
    }

    // Confirm
    if !yes {
        println!(
            "  This will undo {} change(s). Continue? [y/N] ",
            preview.total_changes
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("  Cancelled.");
            return Ok(());
        }
    }

    // Execute undo
    let result = if let Some(fp) = file_path {
        engine.undo_file(&ctx.db, &id, &PathBuf::from(fp))?
    } else {
        engine.undo_task(&ctx.db, &id)?
    };

    // Report
    println!(
        "{} Undo complete: {} restored, {} deleted",
        display::success_prefix(),
        result.files_restored,
        result.files_deleted
    );

    if !result.failures.is_empty() {
        println!();
        println!("  Failures ({}):", result.failures.len());
        for (path, err) in &result.failures {
            println!("    \x1b[31m!\x1b[0m {} — {}", path.display(), err);
        }
    }

    Ok(())
}
