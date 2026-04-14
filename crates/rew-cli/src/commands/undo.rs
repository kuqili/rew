//! `rew undo [id]` — Undo a task (restore files to pre-task state).

use crate::display;
use crate::AppContext;
use rew_core::db::RestoreFailureSample;
use rew_core::error::RewResult;
use rew_core::restore::TaskRestoreEngine;
use rew_core::types::{RestoreOperationStatus, RestoreScopeType, RestoreTriggeredBy};
use std::path::PathBuf;
use uuid::Uuid;

pub fn run(
    ctx: &AppContext,
    task_id: Option<&str>,
    file_path: Option<&str>,
    dry_run: bool,
    yes: bool,
) -> RewResult<()> {
    let objects_root = ctx.rew_dir.join("objects");
    let engine = TaskRestoreEngine::new(objects_root)
        .with_cleanup_boundaries(ctx.config.watch_dirs.clone());

    // If no task ID given, use the most recent task
    let id = match task_id {
        Some(id) => id.to_string(),
        None => {
            let tasks = ctx.db.list_tasks()?;
            let recent = tasks.first().ok_or_else(|| {
                rew_core::error::RewError::Config("No tasks found. Nothing to undo.".to_string())
            })?;
            recent.task.id.clone()
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
    let restore_op_id = Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now();
    let scope_type = if file_path.is_some() {
        RestoreScopeType::File
    } else {
        RestoreScopeType::Task
    };
    let scope_path = file_path.map(PathBuf::from);
    ctx.db.insert_restore_operation_started(
        &restore_op_id,
        &id,
        &scope_type,
        scope_path.as_deref(),
        &RestoreTriggeredBy::Cli,
        started_at,
        preview.total_changes,
        None,
    )?;

    let result = if let Some(fp) = file_path {
        engine.undo_file(&ctx.db, &id, &PathBuf::from(fp))
    } else {
        engine.undo_task(&ctx.db, &id)
    };
    match &result {
        Ok(outcome) => {
            let status = if outcome.failures.is_empty() {
                RestoreOperationStatus::Completed
            } else if outcome.files_restored > 0 || outcome.files_deleted > 0 {
                RestoreOperationStatus::Partial
            } else {
                RestoreOperationStatus::Failed
            };
            let failure_samples: Vec<RestoreFailureSample> = outcome
                .failures
                .iter()
                .take(20)
                .map(|(path, error)| RestoreFailureSample {
                    file_path: path.clone(),
                    error: error.clone(),
                })
                .collect();
            ctx.db.complete_restore_operation(
                &restore_op_id,
                &status,
                chrono::Utc::now(),
                outcome.files_restored,
                outcome.files_deleted,
                outcome.failures.len(),
                &failure_samples,
            )?;
        }
        Err(err) => {
            let failure = vec![RestoreFailureSample {
                file_path: scope_path.unwrap_or_else(|| PathBuf::from(format!("<task:{id}>"))),
                error: err.to_string(),
            }];
            ctx.db.complete_restore_operation(
                &restore_op_id,
                &RestoreOperationStatus::Failed,
                chrono::Utc::now(),
                0,
                0,
                preview.total_changes.max(1),
                &failure,
            )?;
        }
    }
    let result = result?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use rew_core::config::RewConfig;
    use rew_core::db::Database;
    use rew_core::objects::ObjectStore;
    use rew_core::types::{Change, ChangeType, Task, TaskStatus};

    struct TestDir(std::path::PathBuf);

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn test_context() -> (TestDir, AppContext) {
        let dir = std::env::temp_dir().join(format!("rew-cli-undo-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rew_dir = dir.join(".rew");
        std::fs::create_dir_all(rew_dir.join("objects")).unwrap();
        let db = Database::open(&rew_dir.join("snapshots.db")).unwrap();
        db.initialize().unwrap();
        let mut config = RewConfig::default();
        config.watch_dirs = vec![dir.clone()];
        let ctx = AppContext {
            db,
            config,
            config_path: rew_dir.join("config.toml"),
            rew_dir,
        };
        (TestDir(dir), ctx)
    }

    #[test]
    fn cli_undo_writes_restore_operation_for_file_scope() {
        let (dir, ctx) = test_context();
        let task_id = "cli_restore_task";
        let task = Task {
            id: task_id.to_string(),
            prompt: Some("restore file".into()),
            tool: Some("cli".into()),
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        ctx.db.create_task(&task).unwrap();

        let file_path = dir.0.join("src/lib.rs");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, "before").unwrap();
        let old_hash = ObjectStore::new(ctx.rew_dir.join("objects"))
            .unwrap()
            .store(&file_path)
            .unwrap();
        std::fs::write(&file_path, "after").unwrap();

        ctx.db
            .insert_change(&Change {
                id: None,
                task_id: task_id.to_string(),
                file_path: file_path.clone(),
                change_type: ChangeType::Modified,
                old_hash: Some(old_hash),
                new_hash: Some("new-hash".into()),
                diff_text: None,
                lines_added: 1,
                lines_removed: 1,
                attribution: Some("test".into()),
                old_file_path: None,
            })
            .unwrap();

        run(&ctx, Some(task_id), Some(file_path.to_string_lossy().as_ref()), false, true).unwrap();

        let operations = ctx.db.list_restore_operations_for_task(task_id, 10).unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].triggered_by, RestoreTriggeredBy::Cli);
        assert_eq!(operations[0].scope_type, RestoreScopeType::File);
        assert_eq!(operations[0].status, RestoreOperationStatus::Completed);
        assert_eq!(operations[0].requested_count, 1);
        assert_eq!(operations[0].restored_count, 1);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "before"
        );
    }
}
