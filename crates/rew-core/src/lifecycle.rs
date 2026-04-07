//! Daemon lifecycle management: graceful shutdown, signal handling, DB integrity checks.

use crate::db::Database;
use crate::error::RewResult;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Global shutdown flag, set by signal handlers.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Returns whether a shutdown has been requested.
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Requests a graceful shutdown.
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Resets the shutdown flag (for testing).
#[cfg(test)]
pub fn reset_shutdown() {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
}

/// Creates an Arc<AtomicBool> shutdown flag and installs signal handlers.
///
/// The returned flag will be set to `true` when SIGTERM or SIGINT is received.
/// Use this with tokio to create a graceful shutdown mechanism.
pub fn create_shutdown_signal() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let flag_clone = flag.clone();

    // Install signal handlers
    ctrlc::set_handler(move || {
        tracing::info!("Shutdown signal received, finishing current work...");
        flag_clone.store(true, Ordering::SeqCst);
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    })
    .expect("Failed to set signal handler");

    flag
}

/// Performs database integrity check on startup.
///
/// Runs SQLite PRAGMA integrity_check and verifies the schema is correct.
/// Returns Ok(()) if the database is healthy.
pub fn check_db_integrity(db_path: &Path) -> RewResult<()> {
    let db = Database::open(db_path)?;

    // Run SQLite integrity check
    let conn = db.connection();
    let result: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|e| crate::error::RewError::Database(e))?;

    if result != "ok" {
        tracing::error!("Database integrity check failed: {}", result);
        return Err(crate::error::RewError::Config(format!(
            "Database integrity check failed: {}",
            result
        )));
    }

    // Verify schema exists
    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='snapshots'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| crate::error::RewError::Database(e))?;

    if table_count == 0 {
        tracing::warn!("Snapshots table not found, will re-initialize");
        db.initialize()?;
    }

    tracing::info!("Database integrity check passed");
    Ok(())
}

/// Writes a PID file for the daemon process.
pub fn write_pid_file(rew_dir: &Path) -> RewResult<()> {
    let pid = std::process::id();
    let pid_path = rew_dir.join("rew.pid");
    std::fs::write(&pid_path, pid.to_string())?;
    tracing::debug!("PID file written: {} (pid={})", pid_path.display(), pid);
    Ok(())
}

/// Reads the PID from the PID file, if it exists.
pub fn read_pid_file(rew_dir: &Path) -> Option<u32> {
    let pid_path = rew_dir.join("rew.pid");
    std::fs::read_to_string(&pid_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

/// Removes the PID file on shutdown.
pub fn remove_pid_file(rew_dir: &Path) {
    let pid_path = rew_dir.join("rew.pid");
    let _ = std::fs::remove_file(&pid_path);
}

/// Checks if a process with the given PID is alive.
pub fn is_process_alive(pid: u32) -> bool {
    // Use kill(pid, 0) to check if process exists
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shutdown_flag() {
        reset_shutdown();
        assert!(!is_shutdown_requested());
        request_shutdown();
        assert!(is_shutdown_requested());
        reset_shutdown();
        assert!(!is_shutdown_requested());
    }

    #[test]
    fn test_db_integrity_check() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        // Create a valid database
        let db = Database::open(&db_path).unwrap();
        db.initialize().unwrap();
        drop(db);

        // Integrity check should pass
        assert!(check_db_integrity(&db_path).is_ok());
    }

    #[test]
    fn test_db_integrity_check_missing_table() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("empty.db");

        // Create an empty database (no tables)
        let _conn = rusqlite::Connection::open(&db_path).unwrap();
        drop(_conn);

        // Should succeed (re-initializes the missing table)
        assert!(check_db_integrity(&db_path).is_ok());
    }

    #[test]
    fn test_pid_file_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();

        // No PID file initially
        assert!(read_pid_file(tmp.path()).is_none());

        // Write PID file
        write_pid_file(tmp.path()).unwrap();

        // Read should return current PID
        let pid = read_pid_file(tmp.path()).unwrap();
        assert_eq!(pid, std::process::id());

        // Remove PID file
        remove_pid_file(tmp.path());
        assert!(read_pid_file(tmp.path()).is_none());
    }

    #[test]
    fn test_is_process_alive_self() {
        // Current process should be alive
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn test_is_process_alive_nonexistent() {
        // PID 99999 is very unlikely to exist
        // (This is a best-effort test)
        let result = is_process_alive(99999);
        // We can't assert false because the PID might exist, but it shouldn't crash
        let _ = result;
    }
}
