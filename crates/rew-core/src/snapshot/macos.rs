//! macOS APFS snapshot engine implementation.
//!
//! Implements the `SnapshotEngine` trait by delegating to `TmutilWrapper` for
//! OS-level operations and the `Database` for metadata persistence. Handles
//! concurrent snapshot request serialization via a `Mutex`.

use crate::db::Database;
use crate::backup::engine::delete_backup_dir;
use crate::error::{RewError, RewResult};
use crate::types::{EventBatch, FileEventKind, Snapshot, SnapshotTrigger};
use chrono::Utc;
use std::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::tmutil::TmutilWrapper;

/// macOS APFS snapshot engine.
///
/// Coordinates between:
/// - `TmutilWrapper` for actual APFS snapshot operations
/// - `Database` for snapshot metadata persistence
/// - A `Mutex` to serialize concurrent snapshot requests (tmutil is not thread-safe)
pub struct MacOSSnapshotEngine {
    tmutil: TmutilWrapper,
    db: Database,
    /// Serializes snapshot creation requests to avoid concurrent tmutil calls.
    create_lock: Mutex<()>,
}

impl MacOSSnapshotEngine {
    /// Create a new engine with the given database and default volume ("/").
    pub fn new(db: Database) -> Self {
        Self {
            tmutil: TmutilWrapper::default(),
            db,
            create_lock: Mutex::new(()),
        }
    }

    /// Create a new engine with a custom TmutilWrapper.
    pub fn with_tmutil(db: Database, tmutil: TmutilWrapper) -> Self {
        Self {
            tmutil,
            db,
            create_lock: Mutex::new(()),
        }
    }

    /// Check if APFS snapshots are available on this system.
    pub fn is_available(&self) -> bool {
        self.tmutil.is_available()
    }

    /// Create a snapshot triggered by an event batch.
    ///
    /// Serializes concurrent requests: if another thread is already creating
    /// a snapshot, this call blocks until the previous one completes.
    pub fn create_from_batch(
        &self,
        batch: &EventBatch,
        trigger: SnapshotTrigger,
    ) -> RewResult<Snapshot> {
        let files_added = batch.count_by_kind(&FileEventKind::Created) as u32;
        let files_modified = batch.count_by_kind(&FileEventKind::Modified) as u32;
        let files_deleted = batch.count_by_kind(&FileEventKind::Deleted) as u32;

        self.create_internal(trigger, files_added, files_modified, files_deleted, None)
    }

    /// Create a manual snapshot (user-initiated, no event batch).
    pub fn create_manual(&self) -> RewResult<Snapshot> {
        self.create_internal(SnapshotTrigger::Manual, 0, 0, 0, None)
    }

    /// Create a snapshot with full control over all fields.
    pub fn create(
        &self,
        trigger: SnapshotTrigger,
        files_added: u32,
        files_modified: u32,
        files_deleted: u32,
    ) -> RewResult<Snapshot> {
        self.create_internal(trigger, files_added, files_modified, files_deleted, None)
    }

    /// Internal snapshot creation with serialization lock.
    fn create_internal(
        &self,
        trigger: SnapshotTrigger,
        files_added: u32,
        files_modified: u32,
        files_deleted: u32,
        metadata_json: Option<String>,
    ) -> RewResult<Snapshot> {
        // Serialize concurrent snapshot requests
        let _lock = self.create_lock.lock().map_err(|e| {
            RewError::Snapshot(format!("Snapshot creation lock poisoned: {}", e))
        })?;

        info!("Creating APFS snapshot (trigger: {})", trigger);

        // Create the OS-level snapshot
        let snapshot_date = self.tmutil.create_snapshot()?;
        let os_snapshot_ref =
            format!("com.apple.TimeMachine.{}.local", snapshot_date);

        // Build the metadata record
        let snapshot = Snapshot {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            trigger,
            os_snapshot_ref,
            files_added,
            files_modified,
            files_deleted,
            pinned: false,
            metadata_json,
        };

        // Persist to SQLite
        self.db.save_snapshot(&snapshot)?;

        info!(
            "Snapshot created: id={}, os_ref={}, files: +{} ~{} -{}",
            snapshot.id, snapshot.os_snapshot_ref, files_added, files_modified, files_deleted
        );

        Ok(snapshot)
    }

    /// List all snapshots from the database (metadata store).
    pub fn list_snapshots(&self) -> RewResult<Vec<Snapshot>> {
        self.db.list_snapshots()
    }

    /// Get a snapshot by ID.
    pub fn get_snapshot(&self, id: &Uuid) -> RewResult<Option<Snapshot>> {
        self.db.get_snapshot(id)
    }

    /// Delete a snapshot: remove from both APFS and the database.
    pub fn delete_snapshot(&self, id: &Uuid) -> RewResult<()> {
        // Look up the snapshot to get the OS ref
        let snapshot = self.db.get_snapshot(id)?;
        let snapshot = snapshot.ok_or_else(|| {
            RewError::Snapshot(format!("Snapshot {} not found", id))
        })?;

        // Extract the date portion for tmutil
        if let Some(date) =
            TmutilWrapper::extract_date_from_name(&snapshot.os_snapshot_ref)
        {
            match self.tmutil.delete_snapshot(&date) {
                Ok(()) => {
                    debug!("Deleted OS snapshot: {}", snapshot.os_snapshot_ref)
                }
                Err(e) => {
                    warn!(
                        "Failed to delete OS snapshot {} (may already be gone): {}",
                        snapshot.os_snapshot_ref, e
                    );
                    // Continue to delete DB record even if OS snapshot is gone
                }
            }
        }

        let backup_root = crate::rew_home_dir().join("backups");
        delete_backup_dir(id, &backup_root)?;

        // Remove from database
        self.db.delete_snapshot(id)?;
        info!("Deleted snapshot: {}", id);

        Ok(())
    }

    /// Pin or unpin a snapshot.
    pub fn set_pinned(&self, id: &Uuid, pinned: bool) -> RewResult<()> {
        self.db.set_pinned(id, pinned)?;
        info!("Snapshot {} pinned={}", id, pinned);
        Ok(())
    }

    /// Sync database with OS: check which snapshots still exist in APFS and
    /// mark/remove records for snapshots that the OS has automatically deleted.
    pub fn sync_with_os(&self) -> RewResult<u32> {
        let os_snapshots = self.tmutil.list_snapshots()?;
        let db_snapshots = self.db.list_snapshots()?;

        let mut removed_count = 0u32;
        for snap in &db_snapshots {
            // Check if the OS snapshot still exists
            if !os_snapshots.contains(&snap.os_snapshot_ref) {
                warn!(
                    "OS snapshot {} no longer exists, removing DB record for {}",
                    snap.os_snapshot_ref, snap.id
                );
                let backup_root = crate::rew_home_dir().join("backups");
                let _ = delete_backup_dir(&snap.id, &backup_root);
                self.db.delete_snapshot(&snap.id)?;
                removed_count += 1;
            }
        }

        if removed_count > 0 {
            info!(
                "Synced with OS: removed {} stale snapshot records",
                removed_count
            );
        }

        Ok(removed_count)
    }

    /// Get a reference to the underlying database.
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Get a reference to the TmutilWrapper.
    pub fn tmutil(&self) -> &TmutilWrapper {
        &self.tmutil
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_engine() -> (MacOSSnapshotEngine, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        let engine = MacOSSnapshotEngine::new(db);
        (engine, dir)
    }

    #[test]
    fn test_engine_creation() {
        let (engine, _dir) = test_engine();
        let snapshots = engine.list_snapshots().unwrap();
        assert!(snapshots.is_empty());
    }

    // Note: create_snapshot tests require actual tmutil access (macOS with appropriate permissions).
    // Integration tests that call tmutil are gated behind cfg(target_os = "macos") and
    // require running with elevated privileges or Full Disk Access.
}
