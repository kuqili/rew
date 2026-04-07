//! `rew pin` — pin or unpin a snapshot for permanent retention.

use crate::display;
use crate::AppContext;

use rew_core::error::{RewError, RewResult};
use uuid::Uuid;

pub fn run(ctx: &AppContext, snapshot_id: &str, unpin: bool) -> RewResult<()> {
    // Support both full UUID and short prefix
    let resolved_id = resolve_snapshot_id(ctx, snapshot_id)?;

    let snapshot = ctx.db.get_snapshot(&resolved_id)?.ok_or_else(|| {
        RewError::Config(format!("Snapshot '{}' not found", snapshot_id))
    })?;

    let new_pinned = !unpin;

    if snapshot.pinned == new_pinned {
        println!();
        if new_pinned {
            println!("  {} Snapshot {} is already pinned.", display::dim("—"), &snapshot_id[..8.min(snapshot_id.len())]);
        } else {
            println!("  {} Snapshot {} is already unpinned.", display::dim("—"), &snapshot_id[..8.min(snapshot_id.len())]);
        }
        println!();
        return Ok(());
    }

    ctx.db.set_pinned(&resolved_id, new_pinned)?;

    println!();
    if new_pinned {
        println!("  {} Snapshot {} pinned 📌", display::success_prefix(),
            &resolved_id.to_string()[..8]);
        println!("  This snapshot will be retained permanently.");
    } else {
        println!("  {} Snapshot {} unpinned", display::success_prefix(),
            &resolved_id.to_string()[..8]);
        println!("  This snapshot is now subject to normal retention policy.");
    }
    println!();

    Ok(())
}

/// Resolve a snapshot ID — supports full UUID or short prefix match.
fn resolve_snapshot_id(ctx: &AppContext, id_str: &str) -> RewResult<Uuid> {
    // Try full UUID first
    if let Ok(id) = Uuid::parse_str(id_str) {
        return Ok(id);
    }

    // Try prefix match
    let snapshots = ctx.db.list_snapshots()?;
    let matches: Vec<_> = snapshots
        .iter()
        .filter(|s| s.id.to_string().starts_with(id_str))
        .collect();

    match matches.len() {
        0 => Err(RewError::Config(format!("No snapshot matching '{}' found", id_str))),
        1 => Ok(matches[0].id),
        n => Err(RewError::Config(format!(
            "Ambiguous ID '{}' matches {} snapshots. Provide more characters.", id_str, n
        ))),
    }
}
