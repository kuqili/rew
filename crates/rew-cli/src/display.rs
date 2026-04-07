//! Display helpers for consistent CLI output formatting.

use colored::*;
use rew_core::types::{Snapshot, SnapshotTrigger};

/// Colored error prefix.
pub fn error_prefix() -> ColoredString {
    "error:".red().bold()
}

/// Colored success prefix.
pub fn success_prefix() -> ColoredString {
    "✓".green().bold()
}

/// Colored warning prefix.
pub fn warning_prefix() -> ColoredString {
    "⚠".yellow().bold()
}

/// Section header.
pub fn section(title: &str) -> ColoredString {
    title.cyan().bold()
}

/// Dim text for secondary info.
pub fn dim(text: &str) -> ColoredString {
    text.dimmed()
}

/// Format a snapshot trigger with icon.
pub fn trigger_icon(trigger: &SnapshotTrigger) -> &'static str {
    match trigger {
        SnapshotTrigger::Auto => "🔵",
        SnapshotTrigger::Anomaly => "🔴",
        SnapshotTrigger::Manual => "🟢",
    }
}

/// Format a snapshot trigger with icon and label.
pub fn trigger_label(trigger: &SnapshotTrigger) -> String {
    match trigger {
        SnapshotTrigger::Auto => "🔵 auto".to_string(),
        SnapshotTrigger::Anomaly => "🔴 anomaly".to_string(),
        SnapshotTrigger::Manual => "🟢 manual".to_string(),
    }
}

/// Format pin status.
pub fn pin_icon(pinned: bool) -> &'static str {
    if pinned { "📌" } else { "" }
}

/// Format a snapshot as a one-line summary for selection menus.
pub fn snapshot_summary(s: &Snapshot) -> String {
    let time = s.timestamp.format("%Y-%m-%d %H:%M:%S");
    let trigger = trigger_icon(&s.trigger);
    let pin = if s.pinned { " 📌" } else { "" };
    let changes = format!("+{}  ~{}  -{}", s.files_added, s.files_modified, s.files_deleted);
    format!("{} {} [{}]{}", trigger, time, changes, pin)
}

/// Format a snapshot as a detailed block.
pub fn snapshot_detail(s: &Snapshot) {
    println!("  {} {}", "ID:".bold(), s.id);
    println!("  {} {}", "Time:".bold(), s.timestamp.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("  {} {}", "Trigger:".bold(), trigger_label(&s.trigger));
    println!("  {} +{} added, ~{} modified, -{} deleted",
        "Changes:".bold(), s.files_added, s.files_modified, s.files_deleted);
    println!("  {} {}", "OS Ref:".bold(), dim(&s.os_snapshot_ref));
    if s.pinned {
        println!("  {} 📌 Pinned (permanent retention)", "Status:".bold());
    }
}

/// Format file size in human-readable form.
pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format a relative time description.
pub fn time_ago(timestamp: &chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(*timestamp);

    if duration.num_seconds() < 60 {
        "just now".to_string()
    } else if duration.num_minutes() < 60 {
        format!("{} min ago", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{} hours ago", duration.num_hours())
    } else if duration.num_days() < 30 {
        format!("{} days ago", duration.num_days())
    } else {
        format!("{} months ago", duration.num_days() / 30)
    }
}
