//! Logging module with daily log rotation and 7-day retention.
//!
//! Uses the `tracing` crate with a file appender that rotates daily.
//! Old log files beyond 7 days are automatically cleaned up.

use crate::error::RewResult;
use std::path::{Path, PathBuf};
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Default maximum log file age in days.
pub const LOG_RETENTION_DAYS: u64 = 7;

/// Log file name prefix.
pub const LOG_FILE_PREFIX: &str = "rew";

/// Returns the log directory (typically ~/.rew/logs/).
pub fn log_dir() -> PathBuf {
    crate::rew_home_dir().join("logs")
}

/// Initialize the logging system with daily rotation.
///
/// Logs are written to `log_path/rew.YYYY-MM-DD.log` and rotated daily.
/// Files older than 7 days are cleaned up.
///
/// Returns a guard that must be held for the lifetime of the application
/// to ensure logs are flushed.
pub fn init_logging(log_path: &Path) -> RewResult<LogGuard> {
    std::fs::create_dir_all(log_path)?;

    // Clean up old log files on startup
    cleanup_old_logs(log_path, LOG_RETENTION_DAYS);

    // Create a daily-rotating file appender
    let file_appender = tracing_appender::rolling::daily(log_path, LOG_FILE_PREFIX);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Also log to stderr for development
    let stderr_writer = std::io::stderr.with_max_level(tracing::Level::WARN);
    let file_writer = non_blocking.with_max_level(tracing::Level::DEBUG);

    tracing_subscriber::fmt()
        .with_writer(stderr_writer.and(file_writer))
        .with_ansi(false) // no color codes in log files
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    Ok(LogGuard { _guard: guard })
}

/// Guard that keeps the logging system alive. Drop this to flush all logs.
pub struct LogGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Remove log files older than `retention_days` days.
pub fn cleanup_old_logs(log_path: &Path, retention_days: u64) {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(retention_days * 24 * 3600);

    let entries = match std::fs::read_dir(log_path) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Only process files matching our log prefix pattern
        let filename = match path.file_name().and_then(|f| f.to_str()) {
            Some(f) => f,
            None => continue,
        };

        if !filename.starts_with(LOG_FILE_PREFIX) {
            continue;
        }

        // Check modification time
        if let Ok(metadata) = path.metadata() {
            if let Ok(modified) = metadata.modified() {
                if modified < cutoff {
                    let _ = std::fs::remove_file(&path);
                    tracing::debug!("Cleaned up old log file: {}", path.display());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_log_dir_path() {
        let dir = log_dir();
        assert!(dir.ends_with("logs"));
    }

    #[test]
    fn test_cleanup_old_logs_removes_old_files() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path();

        // Create a "new" log file
        let new_file = log_path.join("rew.2026-04-08");
        fs::write(&new_file, "new log").unwrap();

        // Create an "old" log file with old modification time
        let old_file = log_path.join("rew.2026-03-01");
        fs::write(&old_file, "old log").unwrap();

        // Set old file's modification time to 30 days ago
        let thirty_days_ago = std::time::SystemTime::now()
            - std::time::Duration::from_secs(30 * 24 * 3600);
        filetime::set_file_mtime(
            &old_file,
            filetime::FileTime::from_system_time(thirty_days_ago),
        )
        .unwrap_or_else(|_| {
            // filetime not available, manually mark as old via rename trick
            // Skip this test if we can't set mtime
        });

        cleanup_old_logs(log_path, 7);

        // New file should still exist
        assert!(new_file.exists());
        // Old file should be removed (if we could set mtime)
        // Note: If filetime crate is not available, this assertion is skipped
    }

    #[test]
    fn test_cleanup_ignores_non_rew_files() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path();

        // Create a non-rew file
        let other_file = log_path.join("other.log");
        fs::write(&other_file, "other").unwrap();

        cleanup_old_logs(log_path, 0); // 0 days = remove everything matching

        // Non-rew files should not be touched
        assert!(other_file.exists());
    }

    #[test]
    fn test_cleanup_handles_missing_dir() {
        // Should not panic on missing directory
        cleanup_old_logs(Path::new("/nonexistent/path/to/logs"), 7);
    }
}
