//! `rew config` — manage protected directories and configuration.

use crate::display;
use crate::AppContext;
use colored::*;
use rew_core::config::RewConfig;
use rew_core::error::RewResult;
use std::path::PathBuf;

/// Show current configuration.
pub fn show(ctx: &AppContext) -> RewResult<()> {
    println!();
    println!("  {}", display::section("Configuration"));
    println!("  {} {}", "File:".bold(), ctx.config_path.display().to_string().dimmed());
    println!();

    // Watch directories
    println!("  {}", "Watch Directories:".bold());
    if ctx.config.watch_dirs.is_empty() {
        println!("    {} (none)", display::dim("—"));
    } else {
        for dir in &ctx.config.watch_dirs {
            let exists = dir.exists();
            let marker = if exists { "✓".green() } else { "✗".red() };
            println!("    {} {}", marker, dir.display());
        }
    }
    println!();

    // Ignore patterns
    println!("  {}", "Ignore Patterns:".bold());
    for pat in &ctx.config.ignore_patterns {
        println!("    {} {}", display::dim("·"), pat);
    }
    println!();

    // Anomaly rules
    println!("  {}", "Anomaly Thresholds:".bold());
    println!("    Bulk delete (HIGH):     {} files", ctx.config.anomaly_rules.bulk_delete_high);
    println!("    Bulk delete (MEDIUM):   {} files", ctx.config.anomaly_rules.bulk_delete_medium);
    println!("    Delete size (HIGH):     {}", display::human_size(ctx.config.anomaly_rules.delete_size_high_bytes));
    println!("    Bulk modify (MEDIUM):   {} files", ctx.config.anomaly_rules.bulk_modify_medium);
    println!("    Non-pkg modify (MED):   {} files", ctx.config.anomaly_rules.non_package_modify_medium);
    println!("    Window:                 {} sec", ctx.config.anomaly_rules.window_secs);
    println!();

    // Retention policy
    println!("  {}", "Retention Policy:".bold());
    println!("    Keep all:    {} hour(s)", ctx.config.retention_policy.keep_all_secs / 3600);
    println!("    Keep hourly: {} hour(s)", ctx.config.retention_policy.keep_hourly_secs / 3600);
    println!("    Keep daily:  {} day(s)", ctx.config.retention_policy.keep_daily_secs / 86400);
    println!("    Anomaly multiplier: {}x", ctx.config.retention_policy.anomaly_retention_multiplier);
    println!();

    Ok(())
}

/// Add a directory to the watch list.
pub fn add_dir(ctx: &AppContext, path: &str) -> RewResult<()> {
    let expanded = expand_path(path);
    let canonical = if expanded.exists() {
        expanded.canonicalize().unwrap_or(expanded)
    } else {
        expanded
    };

    // Check if already in the list
    let mut config = ctx.config.clone();
    if config.watch_dirs.iter().any(|d| d == &canonical) {
        println!();
        println!("  {} Directory already in watch list: {}", display::warning_prefix(), canonical.display());
        println!();
        return Ok(());
    }

    // Check if directory exists
    if !canonical.exists() {
        println!("  {} Directory does not exist: {}", display::warning_prefix(), canonical.display());
        println!("  Adding anyway — it will be monitored when created.");
    }

    config.watch_dirs.push(canonical.clone());
    config.save(&ctx.config_path)?;

    println!();
    println!("  {} Added directory: {}", display::success_prefix(), canonical.display().to_string().bold());
    println!("  The daemon will pick up this change on next restart.");
    println!();

    Ok(())
}

/// Remove a directory from the watch list.
pub fn remove_dir(ctx: &AppContext, path: &str) -> RewResult<()> {
    let expanded = expand_path(path);
    let canonical = if expanded.exists() {
        expanded.canonicalize().unwrap_or(expanded.clone())
    } else {
        expanded.clone()
    };

    let mut config = ctx.config.clone();
    let before_len = config.watch_dirs.len();

    // Try both the canonical and expanded forms
    config.watch_dirs.retain(|d| d != &canonical && d != &expanded);

    if config.watch_dirs.len() == before_len {
        println!();
        println!("  {} Directory not in watch list: {}", display::warning_prefix(), path);
        println!();
        println!("  Current watch directories:");
        for dir in &config.watch_dirs {
            println!("    · {}", dir.display());
        }
        println!();
        return Ok(());
    }

    config.save(&ctx.config_path)?;

    println!();
    println!("  {} Removed directory: {}", display::success_prefix(), path.bold());
    println!();

    Ok(())
}

/// Reset configuration to defaults.
pub fn reset(ctx: &AppContext) -> RewResult<()> {
    let default_config = RewConfig::default();
    default_config.save(&ctx.config_path)?;

    println!();
    println!("  {} Configuration reset to defaults.", display::success_prefix());
    println!("  Config file: {}", ctx.config_path.display());
    println!();

    Ok(())
}

/// Expand ~ and make path absolute.
fn expand_path(path: &str) -> PathBuf {
    if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            let suffix: &str = &path[1..];
            return home.join(suffix.trim_start_matches('/'));
        }
    }
    let p = PathBuf::from(path);
    if p.is_relative() {
        std::env::current_dir().unwrap_or_default().join(p)
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_path_absolute() {
        let result = expand_path("/Users/test/Documents");
        assert_eq!(result, PathBuf::from("/Users/test/Documents"));
    }

    #[test]
    fn test_expand_path_tilde() {
        let result = expand_path("~/Projects");
        let home = dirs::home_dir().unwrap();
        assert_eq!(result, home.join("Projects"));
    }

    #[test]
    fn test_expand_path_relative() {
        let result = expand_path("relative/path");
        assert!(result.is_absolute());
    }
}
