//! SQLite database for snapshot metadata storage.
//!
//! Stores snapshot records in ~/.rew/snapshots.db.

use crate::error::{RewError, RewResult};
use crate::types::{Snapshot, SnapshotTrigger};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use uuid::Uuid;

/// Database handle wrapping a SQLite connection.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the database at the given path.
    pub fn open(path: &Path) -> RewResult<Self> {
        let conn = Connection::open(path)?;
        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Ok(Database { conn })
    }

    /// Returns a reference to the underlying SQLite connection.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Create the snapshots table if it doesn't exist.
    pub fn initialize(&self) -> RewResult<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS snapshots (
                id               TEXT PRIMARY KEY,
                timestamp        TEXT NOT NULL,
                trigger_type     TEXT NOT NULL,
                os_snapshot_ref  TEXT NOT NULL,
                files_added      INTEGER NOT NULL DEFAULT 0,
                files_modified   INTEGER NOT NULL DEFAULT 0,
                files_deleted    INTEGER NOT NULL DEFAULT 0,
                pinned           INTEGER NOT NULL DEFAULT 0,
                metadata_json    TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp
                ON snapshots(timestamp DESC);

            CREATE INDEX IF NOT EXISTS idx_snapshots_trigger
                ON snapshots(trigger_type);

            CREATE INDEX IF NOT EXISTS idx_snapshots_pinned
                ON snapshots(pinned);",
        )?;
        Ok(())
    }

    /// Insert a new snapshot record.
    pub fn save_snapshot(&self, snapshot: &Snapshot) -> RewResult<()> {
        self.conn.execute(
            "INSERT INTO snapshots (id, timestamp, trigger_type, os_snapshot_ref,
                files_added, files_modified, files_deleted, pinned, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                snapshot.id.to_string(),
                snapshot.timestamp.to_rfc3339(),
                snapshot.trigger.to_string(),
                snapshot.os_snapshot_ref,
                snapshot.files_added,
                snapshot.files_modified,
                snapshot.files_deleted,
                snapshot.pinned as i32,
                snapshot.metadata_json,
            ],
        )?;
        Ok(())
    }

    /// Get a snapshot by ID.
    pub fn get_snapshot(&self, id: &Uuid) -> RewResult<Option<Snapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, trigger_type, os_snapshot_ref,
                    files_added, files_modified, files_deleted, pinned, metadata_json
             FROM snapshots WHERE id = ?1",
        )?;

        let result = stmt.query_row(params![id.to_string()], |row| {
            Ok(Self::row_to_snapshot(row))
        });

        match result {
            Ok(snapshot) => Ok(Some(
                snapshot.map_err(|e| RewError::Snapshot(e))?,
            )),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(RewError::Database(e)),
        }
    }

    /// List all snapshots ordered by timestamp descending.
    pub fn list_snapshots(&self) -> RewResult<Vec<Snapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, trigger_type, os_snapshot_ref,
                    files_added, files_modified, files_deleted, pinned, metadata_json
             FROM snapshots ORDER BY timestamp DESC",
        )?;

        let mut snapshots = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let snapshot = Self::row_to_snapshot(row)
                .map_err(|e| RewError::Snapshot(e))?;
            snapshots.push(snapshot);
        }

        Ok(snapshots)
    }

    /// Delete a snapshot by ID.
    pub fn delete_snapshot(&self, id: &Uuid) -> RewResult<()> {
        self.conn.execute(
            "DELETE FROM snapshots WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// Pin or unpin a snapshot.
    pub fn set_pinned(&self, id: &Uuid, pinned: bool) -> RewResult<()> {
        self.conn.execute(
            "UPDATE snapshots SET pinned = ?1 WHERE id = ?2",
            params![pinned as i32, id.to_string()],
        )?;
        Ok(())
    }

    /// Get snapshots older than max_age_secs that are not pinned.
    pub fn get_cleanup_candidates(&self, max_age_secs: i64) -> RewResult<Vec<Snapshot>> {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs);

        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, trigger_type, os_snapshot_ref,
                    files_added, files_modified, files_deleted, pinned, metadata_json
             FROM snapshots
             WHERE pinned = 0 AND timestamp < ?1
             ORDER BY timestamp ASC",
        )?;

        let mut snapshots = Vec::new();
        let mut rows = stmt.query(params![cutoff.to_rfc3339()])?;
        while let Some(row) = rows.next()? {
            let snapshot = Self::row_to_snapshot(row)
                .map_err(|e| RewError::Snapshot(e))?;
            snapshots.push(snapshot);
        }

        Ok(snapshots)
    }

    fn row_to_snapshot(row: &rusqlite::Row) -> Result<Snapshot, String> {
        let id_str: String = row.get(0).map_err(|e| e.to_string())?;
        let timestamp_str: String = row.get(1).map_err(|e| e.to_string())?;
        let trigger_str: String = row.get(2).map_err(|e| e.to_string())?;

        let id = Uuid::parse_str(&id_str).map_err(|e| e.to_string())?;
        let timestamp: DateTime<Utc> = timestamp_str.parse().map_err(|e: chrono::ParseError| e.to_string())?;
        let trigger: SnapshotTrigger = trigger_str.parse()?;

        Ok(Snapshot {
            id,
            timestamp,
            trigger,
            os_snapshot_ref: row.get(3).map_err(|e| e.to_string())?,
            files_added: row.get(4).map_err(|e| e.to_string())?,
            files_modified: row.get(5).map_err(|e| e.to_string())?,
            files_deleted: row.get(6).map_err(|e| e.to_string())?,
            pinned: row.get::<_, i32>(7).map_err(|e| e.to_string())? != 0,
            metadata_json: row.get(8).map_err(|e| e.to_string())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SnapshotTrigger;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        db
    }

    #[test]
    fn test_initialize_creates_table() {
        let db = test_db();
        // Verify table exists by querying it
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_snapshot_crud() {
        let db = test_db();
        let snapshot = Snapshot {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            trigger: SnapshotTrigger::Auto,
            os_snapshot_ref: "com.apple.TimeMachine.2026-04-07-123456".to_string(),
            files_added: 5,
            files_modified: 3,
            files_deleted: 1,
            pinned: false,
            metadata_json: None,
        };

        // Create
        db.save_snapshot(&snapshot).unwrap();

        // Read
        let loaded = db.get_snapshot(&snapshot.id).unwrap().unwrap();
        assert_eq!(loaded.id, snapshot.id);
        assert_eq!(loaded.trigger, SnapshotTrigger::Auto);
        assert_eq!(loaded.files_added, 5);

        // List
        let all = db.list_snapshots().unwrap();
        assert_eq!(all.len(), 1);

        // Pin
        db.set_pinned(&snapshot.id, true).unwrap();
        let pinned = db.get_snapshot(&snapshot.id).unwrap().unwrap();
        assert!(pinned.pinned);

        // Delete
        db.delete_snapshot(&snapshot.id).unwrap();
        let gone = db.get_snapshot(&snapshot.id).unwrap();
        assert!(gone.is_none());
    }
}
