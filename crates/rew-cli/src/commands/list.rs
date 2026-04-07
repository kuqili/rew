//! `rew list` — list all snapshots with trigger type icons and pin status.

use crate::display;
use crate::AppContext;
use colored::*;
use rew_core::error::RewResult;

pub fn run(ctx: &AppContext) -> RewResult<()> {
    let snapshots = ctx.db.list_snapshots()?;

    if snapshots.is_empty() {
        println!();
        println!("  No snapshots yet.");
        println!("  Snapshots are created automatically when file changes are detected.");
        println!();
        return Ok(());
    }

    println!();
    println!("  {} ({} total)", display::section("Snapshots"), snapshots.len());
    println!();

    // Table header
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
    println!("  {} 🔵 auto  🔴 anomaly  🟢 manual  📌 pinned", display::dim("Legend:"));
    println!();

    Ok(())
}
