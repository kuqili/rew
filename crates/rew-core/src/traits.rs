//! Core trait definitions for rew.
//!
//! These traits define the cross-platform abstractions that allow rew to work
//! on both macOS (APFS/FSEvents) and Windows (VSS/ReadDirectoryChanges).

use crate::error::RewResult;
use crate::processor::BatchStats;
use crate::types::{AnomalySignal, EventBatch, FileEvent, RestoreJob, Snapshot};
use std::path::Path;

/// Watches file system events on target directories.
///
/// Platform implementations:
/// - macOS: FSEvents via `notify` crate
/// - Windows: ReadDirectoryChangesW via `notify` crate
pub trait FileWatcher: Send + Sync {
    /// Start watching the given directories. Events are sent to the provided callback.
    fn start<F>(&mut self, callback: F) -> RewResult<()>
    where
        F: Fn(FileEvent) + Send + Sync + 'static;

    /// Stop watching and release resources.
    fn stop(&mut self) -> RewResult<()>;

    /// Add a directory to the watch list at runtime.
    fn add_path(&mut self, path: &Path) -> RewResult<()>;

    /// Remove a directory from the watch list at runtime.
    fn remove_path(&mut self, path: &Path) -> RewResult<()>;
}

/// Creates, lists, mounts, restores, and deletes OS-level snapshots.
///
/// Platform implementations:
/// - macOS: APFS snapshots via `tmutil` CLI
/// - Windows: VSS (Volume Shadow Copy Service)
pub trait SnapshotEngine: Send + Sync {
    /// Create a new snapshot. Returns the OS-level snapshot reference.
    fn create_snapshot(&self) -> RewResult<String>;

    /// List all snapshots known to the OS.
    fn list_os_snapshots(&self) -> RewResult<Vec<String>>;

    /// Delete an OS-level snapshot by its reference.
    fn delete_snapshot(&self, os_ref: &str) -> RewResult<()>;

    /// Restore files from a snapshot.
    fn restore(&self, job: &RestoreJob, os_ref: &str) -> RewResult<()>;

    /// Check if the snapshot engine is available on this platform.
    fn is_available(&self) -> bool;
}

/// Persistent storage backend for snapshot metadata.
///
/// The primary implementation uses SQLite via rusqlite.
pub trait StorageBackend: Send + Sync {
    /// Save a snapshot record.
    fn save_snapshot(&self, snapshot: &Snapshot) -> RewResult<()>;

    /// Get a snapshot by its ID.
    fn get_snapshot(&self, id: &uuid::Uuid) -> RewResult<Option<Snapshot>>;

    /// List all snapshots, most recent first.
    fn list_snapshots(&self) -> RewResult<Vec<Snapshot>>;

    /// Delete a snapshot record by its ID.
    fn delete_snapshot(&self, id: &uuid::Uuid) -> RewResult<()>;

    /// Pin/unpin a snapshot (pinned snapshots are never auto-deleted).
    fn set_pinned(&self, id: &uuid::Uuid, pinned: bool) -> RewResult<()>;

    /// Get snapshots that are candidates for cleanup based on retention policy.
    fn get_cleanup_candidates(
        &self,
        max_age_secs: i64,
    ) -> RewResult<Vec<Snapshot>>;
}

/// Detects anomalous file operation patterns from event batches.
pub trait AnomalyDetector: Send + Sync {
    /// Analyze an event batch and return any detected anomalies.
    /// `stats` contains pre-aggregated counts that allow O(1) short-circuit
    /// checks in rules before iterating over individual events.
    fn analyze(&self, batch: &EventBatch, stats: &BatchStats) -> Vec<AnomalySignal>;
}

/// Sends notifications to the user.
///
/// Platform implementations:
/// - macOS: NSUserNotification / UNUserNotificationCenter
/// - Windows: Windows Toast Notifications
pub trait Notifier: Send + Sync {
    /// Send a notification about an anomaly.
    fn notify_anomaly(&self, signal: &AnomalySignal) -> RewResult<()>;

    /// Send a general notification.
    fn notify(&self, title: &str, body: &str) -> RewResult<()>;
}
