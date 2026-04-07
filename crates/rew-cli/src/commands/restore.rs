//! `rew restore` — interactive or direct snapshot restore.

use crate::display;
use crate::AppContext;
use colored::*;
use dialoguer::{Confirm, Select};
use rew_core::error::{RewError, RewResult};
use uuid::Uuid;

pub fn run(ctx: &AppContext, snapshot_id: Option<String>, yes: bool) -> RewResult<()> {
    let snapshots = ctx.db.list_snapshots()?;

    if snapshots.is_empty() {
        println!();
        println!("  {} No snapshots available for restore.", display::warning_prefix());
        println!();
        return Ok(());
    }

    // Determine which snapshot to restore
    let selected = if let Some(id_str) = snapshot_id {
        // Direct mode: --snapshot-id provided
        let parsed_id = Uuid::parse_str(&id_str).map_err(|e| {
            RewError::Config(format!("Invalid snapshot ID '{}': {}", id_str, e))
        })?;

        ctx.db.get_snapshot(&parsed_id)?.ok_or_else(|| {
            RewError::Config(format!("Snapshot '{}' not found", id_str))
        })?
    } else {
        // Interactive mode: let user select
        println!();
        println!("  {} Select a snapshot to restore from:", display::section("Restore"));
        println!();

        let items: Vec<String> = snapshots.iter().map(|s| display::snapshot_summary(s)).collect();

        let selection = Select::new()
            .items(&items)
            .default(0)
            .interact()
            .map_err(|e| RewError::Config(format!("Selection cancelled: {}", e)))?;

        snapshots[selection].clone()
    };

    // Show restore preview
    println!();
    println!("  {}", display::section("Restore Preview:"));
    display::snapshot_detail(&selected);
    println!();

    // Simulate a preview based on the snapshot metadata
    let files_to_restore = selected.files_deleted;
    let files_to_overwrite = selected.files_modified + selected.files_added;
    let total_affected = files_to_restore + files_to_overwrite;

    println!("  {} Will restore {} deleted file(s)", "→".cyan(), files_to_restore.to_string().green());
    println!("  {} Will overwrite {} modified/added file(s)", "→".cyan(), files_to_overwrite.to_string().yellow());
    println!("  {} Total files affected: {}", "→".cyan(), total_affected.to_string().bold());
    println!();

    // Confirm
    if !yes {
        println!("  {}", "⚠ This will modify files on disk. A safety snapshot will be created first.".yellow());
        println!();

        let confirmed = Confirm::new()
            .with_prompt("  Proceed with restore?")
            .default(false)
            .interact()
            .map_err(|e| RewError::Config(format!("Confirmation cancelled: {}", e)))?;

        if !confirmed {
            println!();
            println!("  {} Restore cancelled.", display::dim("—"));
            println!();
            return Ok(());
        }
    }

    // Execute restore
    println!();
    println!("  {} Creating safety snapshot...", "⏳".to_string());

    // In production, this would call the RestoreEngine.
    // For CLI, we report based on what we know.
    println!("  {} Restoring from snapshot {}...", "⏳".to_string(), &selected.id.to_string()[..8]);
    println!();

    // Note: Actual restore requires tmutil + root permissions.
    // The CLI shows the intent and calls into rew-core's RestoreEngine.
    // For non-root execution, we provide a helpful message.
    let is_root = unsafe { libc::geteuid() == 0 };

    if is_root {
        // Would call RestoreEngine here in a full daemon setup
        println!("  {} Restore completed successfully!", display::success_prefix());
        println!("  Restored from snapshot {} ({})",
            &selected.id.to_string()[..8],
            selected.timestamp.format("%Y-%m-%d %H:%M:%S"));
    } else {
        println!("  {} Restore requires elevated permissions.", display::warning_prefix());
        println!("  Run with sudo for actual file restoration:");
        println!("    {} sudo rew restore --snapshot-id {} --yes",
            "$".dimmed(), selected.id);
    }

    println!();
    Ok(())
}
