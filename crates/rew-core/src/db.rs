//! SQLite database for snapshot and task metadata storage.
//!
//! Stores snapshot records, task records, and file change records
//! in ~/.rew/rew.db (or ~/.rew/snapshots.db for backward compat).

use crate::error::{RewError, RewResult};
use crate::types::{
    Change, Snapshot, SnapshotTrigger, Task, TaskStatus,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Database handle wrapping a SQLite connection.
/// Parse a datetime string flexibly — supports RFC3339, ISO8601 with offset, and naive formats.
fn parse_datetime_flexible(s: &str) -> Result<DateTime<Utc>, String> {
    // Try RFC3339 first (e.g. "2026-04-08T10:06:25.764968+00:00")
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Ok(dt);
    }
    // Try with fixed offset (e.g. "2026-04-08T10:06:25+08:00")
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Try naive datetime with T (e.g. "2026-04-08T10:06:25")
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(naive.and_utc());
    }
    // Try naive datetime with space (e.g. "2026-04-08 10:06:25")
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive.and_utc());
    }
    Err(format!("Cannot parse datetime: '{}'", s))
}

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the database at the given path.
    pub fn open(path: &Path) -> RewResult<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Ok(Database { conn })
    }

    /// Returns a reference to the underlying SQLite connection.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Create the database tables if they don't exist.
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
                ON snapshots(pinned);

            -- V2: Task tracking (one user prompt = one task)
            CREATE TABLE IF NOT EXISTS tasks (
                id              TEXT PRIMARY KEY,
                prompt          TEXT,
                tool            TEXT,
                started_at      TEXT NOT NULL,
                completed_at    TEXT,
                status          TEXT NOT NULL DEFAULT 'active',
                risk_level      TEXT,
                summary         TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_started_at
                ON tasks(started_at DESC);

            CREATE INDEX IF NOT EXISTS idx_tasks_status
                ON tasks(status);

            -- V2: File changes within a task
            CREATE TABLE IF NOT EXISTS changes (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id         TEXT NOT NULL REFERENCES tasks(id),
                file_path       TEXT NOT NULL,
                change_type     TEXT NOT NULL,
                old_hash        TEXT,
                new_hash        TEXT,
                diff_text       TEXT,
                lines_added     INTEGER NOT NULL DEFAULT 0,
                lines_removed   INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_changes_task_id
                ON changes(task_id);

            CREATE INDEX IF NOT EXISTS idx_changes_file_path
                ON changes(file_path);",
        )?;

        // V3 migration: add restored_at column if this is an existing DB.
        // ALTER TABLE ADD COLUMN is a no-op error when the column already exists — safe to ignore.
        let _ = self.conn.execute(
            "ALTER TABLE changes ADD COLUMN restored_at TEXT",
            [],
        );

        // V4 migration: add cwd column to tasks for project directory tracking.
        let _ = self.conn.execute(
            "ALTER TABLE tasks ADD COLUMN cwd TEXT",
            [],
        );

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

    // ================================================================
    // Task CRUD (V2)
    // ================================================================

    /// Create a new task record.
    pub fn create_task(&self, task: &Task) -> RewResult<()> {
        self.conn.execute(
            "INSERT INTO tasks (id, prompt, tool, started_at, completed_at, status, risk_level, summary, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                task.id,
                task.prompt,
                task.tool,
                task.started_at.to_rfc3339(),
                task.completed_at.map(|t| t.to_rfc3339()),
                task.status.to_string(),
                task.risk_level.as_ref().map(|r| r.to_string()),
                task.summary,
                task.cwd,
            ],
        )?;
        Ok(())
    }

    /// Get a task by ID.
    pub fn get_task(&self, id: &str) -> RewResult<Option<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, prompt, tool, started_at, completed_at, status, risk_level, summary, cwd
             FROM tasks WHERE id = ?1",
        )?;

        let result = stmt.query_row(params![id], |row| Ok(Self::row_to_task(row)));

        match result {
            Ok(task) => Ok(Some(task.map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))))?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(RewError::Database(e)),
        }
    }

    /// List all tasks ordered by started_at descending.
    /// In-progress monitoring windows (tool='文件监听' with NULL completed_at) are excluded
    /// so users only see fully-sealed archive entries.
    pub fn list_tasks(&self) -> RewResult<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, prompt, tool, started_at, completed_at, status, risk_level, summary, cwd
             FROM tasks
             WHERE (tool != '文件监听' OR completed_at IS NOT NULL)
             ORDER BY started_at DESC",
        )?;

        let mut tasks = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let task = Self::row_to_task(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))))?;
            tasks.push(task);
        }

        Ok(tasks)
    }

    /// Update task status and optionally set completed_at.
    pub fn update_task_status(
        &self,
        id: &str,
        status: &TaskStatus,
        completed_at: Option<DateTime<Utc>>,
    ) -> RewResult<()> {
        if let Some(ts) = completed_at {
            self.conn.execute(
                "UPDATE tasks SET status = ?1, completed_at = ?2 WHERE id = ?3",
                params![status.to_string(), ts.to_rfc3339(), id],
            )?;
        } else {
            // Preserve existing completed_at (e.g. when marking as rolled-back we
            // don't want to overwrite the original archive timestamp).
            self.conn.execute(
                "UPDATE tasks SET status = ?1 WHERE id = ?2",
                params![status.to_string(), id],
            )?;
        }
        Ok(())
    }

    /// Return the single most-recently-started monitoring window (fs_* task).
    ///
    /// Used on startup to decide whether to resume or abandon the previous window.
    pub fn get_latest_monitoring_window(&self) -> RewResult<Option<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, prompt, tool, started_at, completed_at, status, risk_level, summary, cwd
             FROM tasks WHERE id LIKE 'fs_%' ORDER BY started_at DESC LIMIT 1",
        )?;
        let result = stmt.query_row([], |row| Ok(Self::row_to_task(row)));
        match result {
            Ok(task) => Ok(Some(task.map_err(|e| {
                RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(
                    std::io::Error::new(std::io::ErrorKind::Other, e),
                )))
            })?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(RewError::Database(e)),
        }
    }

    /// Seal any monitoring windows that still have `completed_at IS NULL`.
    ///
    /// Only needed for the rare crash-before-first-update edge case.
    pub fn seal_null_monitoring_windows(&self, completed_at: DateTime<Utc>) -> RewResult<()> {
        self.conn.execute(
            "UPDATE tasks SET completed_at = ?1
             WHERE id LIKE 'fs_%' AND completed_at IS NULL",
            params![completed_at.to_rfc3339()],
        )?;
        Ok(())
    }

    /// On app startup: mark any lingering AI tasks (non-fs_* tasks) that still
    /// have `status = 'active'` as completed.
    ///
    /// Lingering tasks arise when the user interrupts an AI session (Ctrl-C,
    /// force-quit) before `rew hook stop` fires.  After a restart there can be
    /// no live AI session, so it is safe to close them all.
    ///
    /// Returns the number of rows updated.
    pub fn recover_stale_ai_tasks(&self, now: DateTime<Utc>) -> RewResult<usize> {
        let n = self.conn.execute(
            "UPDATE tasks SET status = 'completed', completed_at = ?1
             WHERE status = 'active' AND id NOT LIKE 'fs_%'",
            params![now.to_rfc3339()],
        )?;
        Ok(n)
    }

    /// Seal a file-monitoring time window by setting its `completed_at` timestamp.
    ///
    /// Called when a new window opens to close the previous one.
    pub fn update_task_completed_at(&self, id: &str, completed_at: DateTime<Utc>) -> RewResult<()> {
        self.conn.execute(
            "UPDATE tasks SET completed_at = ?1 WHERE id = ?2",
            params![completed_at.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Delete a task row (used to clean up ghost monitoring windows with 0 changes).
    pub fn delete_task(&self, id: &str) -> RewResult<()> {
        self.conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Update task summary (set by AI after completion).
    pub fn update_task_summary(&self, id: &str, summary: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE tasks SET summary = ?1 WHERE id = ?2",
            params![summary, id],
        )?;
        Ok(())
    }

    fn row_to_task(row: &rusqlite::Row) -> Result<Task, String> {
        let started_at_str: String = row.get(3).map_err(|e| e.to_string())?;
        let completed_at_str: Option<String> = row.get(4).map_err(|e| e.to_string())?;
        let status_str: String = row.get(5).map_err(|e| e.to_string())?;
        let risk_str: Option<String> = row.get(6).map_err(|e| e.to_string())?;

        Ok(Task {
            id: row.get(0).map_err(|e| e.to_string())?,
            prompt: row.get(1).map_err(|e| e.to_string())?,
            tool: row.get(2).map_err(|e| e.to_string())?,
            started_at: parse_datetime_flexible(&started_at_str)
                .map_err(|e| format!("started_at parse '{}': {}", started_at_str, e))?,
            completed_at: completed_at_str
                .map(|s| parse_datetime_flexible(&s))
                .transpose()
                .map_err(|e| e.to_string())?,
            status: status_str.parse()?,
            risk_level: risk_str.map(|s| s.parse()).transpose()?,
            summary: row.get(7).map_err(|e| e.to_string())?,
            cwd: row.get(8).map_err(|e| e.to_string())?,
        })
    }

    // ================================================================
    // Change CRUD (V2)
    // ================================================================

    /// Record a file change within a task.
    pub fn insert_change(&self, change: &Change) -> RewResult<i64> {
        self.conn.execute(
            "INSERT INTO changes (task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                change.task_id,
                change.file_path.to_string_lossy().to_string(),
                change.change_type.to_string(),
                change.old_hash,
                change.new_hash,
                change.diff_text,
                change.lines_added,
                change.lines_removed,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert or update a change record for (task_id, file_path).
    ///
    /// Guarantees one record per file per task:
    /// - If no existing record → INSERT (same as insert_change)
    /// - If record exists → UPDATE, keeping the original `old_hash` (state before
    ///   the task started) while updating `change_type`, `new_hash`, and stats
    ///   to reflect the latest operation on this file.
    pub fn upsert_change(&self, change: &Change) -> RewResult<i64> {
        let path_str = change.file_path.to_string_lossy().to_string();

        let existing = self.conn.query_row(
            "SELECT id, old_hash FROM changes WHERE task_id = ?1 AND file_path = ?2",
            params![change.task_id, path_str],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        ).optional()?;

        if let Some((existing_id, existing_old_hash)) = existing {
            // Keep the earliest old_hash (what the file looked like before this task)
            let preserved_old_hash = existing_old_hash.or(change.old_hash.clone());
            self.conn.execute(
                "UPDATE changes SET change_type=?1, old_hash=?2, new_hash=?3,
                 diff_text=?4, lines_added=?5, lines_removed=?6
                 WHERE id=?7",
                params![
                    change.change_type.to_string(),
                    preserved_old_hash,
                    change.new_hash,
                    change.diff_text,
                    change.lines_added,
                    change.lines_removed,
                    existing_id,
                ],
            )?;
            Ok(existing_id)
        } else {
            self.insert_change(change)
        }
    }

    /// Get all changes for a task.
    pub fn get_changes_for_task(&self, task_id: &str) -> RewResult<Vec<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at
             FROM changes WHERE task_id = ?1 ORDER BY id ASC",
        )?;

        let mut changes = Vec::new();
        let mut rows = stmt.query(params![task_id])?;
        while let Some(row) = rows.next()? {
            let change = Self::row_to_change(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))))?;
            changes.push(change);
        }

        Ok(changes)
    }

    /// List tasks that have at least one change under `dir_prefix`.
    pub fn list_tasks_by_dir(&self, dir_prefix: &str) -> RewResult<Vec<Task>> {
        let like_pattern = format!("{}/%", dir_prefix);
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, t.prompt, t.tool, t.started_at, t.completed_at,
                    t.status, t.risk_level, t.summary, t.cwd
             FROM tasks t
             JOIN changes c ON c.task_id = t.id
             WHERE c.file_path LIKE ?1
               AND (t.tool != '文件监听' OR t.completed_at IS NOT NULL)
             ORDER BY t.started_at DESC",
        )?;

        let mut tasks = Vec::new();
        let mut rows = stmt.query(params![like_pattern])?;
        while let Some(row) = rows.next()? {
            let task = Self::row_to_task(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)),
                )))?;
            tasks.push(task);
        }

        Ok(tasks)
    }

    /// Filter tasks to those containing a specific file change (exact path match).
    pub fn list_tasks_by_file(&self, file_path: &str) -> RewResult<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, t.prompt, t.tool, t.started_at, t.completed_at,
                    t.status, t.risk_level, t.summary, t.cwd
             FROM tasks t
             JOIN changes c ON c.task_id = t.id
             WHERE c.file_path = ?1
               AND (t.tool != '文件监听' OR t.completed_at IS NOT NULL)
             ORDER BY t.started_at DESC",
        )?;

        let mut tasks = Vec::new();
        let mut rows = stmt.query(params![file_path])?;
        while let Some(row) = rows.next()? {
            let task = Self::row_to_task(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)),
                )))?;
            tasks.push(task);
        }

        Ok(tasks)
    }

    /// Count changes for a task that fall under `dir_prefix`.
    pub fn count_changes_in_dir(&self, task_id: &str, dir_prefix: &str) -> RewResult<usize> {
        let like_pattern = format!("{}/%", dir_prefix);
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE task_id = ?1 AND file_path LIKE ?2",
            params![task_id, like_pattern],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Count changes for a task matching a specific file path exactly.
    pub fn count_changes_for_file(&self, task_id: &str, file_path: &str) -> RewResult<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE task_id = ?1 AND file_path = ?2",
            params![task_id, file_path],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get changes for a task filtered to a specific directory prefix.
    pub fn get_changes_for_task_in_dir(&self, task_id: &str, dir_prefix: &str) -> RewResult<Vec<Change>> {
        let like_pattern = format!("{}/%", dir_prefix);
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at
             FROM changes WHERE task_id = ?1 AND file_path LIKE ?2 ORDER BY id ASC",
        )?;

        let mut changes = Vec::new();
        let mut rows = stmt.query(params![task_id, like_pattern])?;
        while let Some(row) = rows.next()? {
            let change = Self::row_to_change(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)),
                )))?;
            changes.push(change);
        }

        Ok(changes)
    }

    /// Get changes for a task matching a specific file path exactly.
    pub fn get_changes_for_task_by_file(&self, task_id: &str, file_path: &str) -> RewResult<Vec<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at
             FROM changes WHERE task_id = ?1 AND file_path = ?2 ORDER BY id ASC",
        )?;

        let mut changes = Vec::new();
        let mut rows = stmt.query(params![task_id, file_path])?;
        while let Some(row) = rows.next()? {
            let change = Self::row_to_change(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)),
                )))?;
            changes.push(change);
        }

        Ok(changes)
    }

    /// Get the most recent change for a specific file path (for shadow lookup).
    /// Returns hashes of stored versions for `file_path` that are older than the
    /// `keep_count` most-recent ones (ordered by task creation time, newest first).
    ///
    /// These hashes are candidates for ObjectStore deletion — callers must still
    /// verify via `is_hash_referenced` before removing, since content-addressable
    /// storage means the same hash can appear in multiple change records.
    pub fn get_old_version_hashes(&self, file_path: &Path, keep_count: usize) -> RewResult<Vec<String>> {
        let path_str = file_path.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT c.new_hash
             FROM changes c
             JOIN tasks t ON c.task_id = t.id
             WHERE c.file_path = ?1 AND c.new_hash IS NOT NULL
             ORDER BY t.created_at DESC",
        )?;
        let all_hashes: Vec<String> = stmt
            .query_map(params![path_str], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        // Everything beyond the keep_count most recent is a candidate for deletion
        Ok(all_hashes.into_iter().skip(keep_count).collect())
    }

    /// Returns the current `new_hash` for a `(task_id, file_path)` change record, if one exists.
    /// Used to detect which object hash becomes orphaned after an upsert.
    pub fn get_change_new_hash(&self, task_id: &str, file_path: &Path) -> RewResult<Option<String>> {
        let path_str = file_path.to_string_lossy().to_string();
        let result = self.conn.query_row(
            "SELECT new_hash FROM changes WHERE task_id = ?1 AND file_path = ?2",
            params![task_id, path_str],
            |row| row.get::<_, Option<String>>(0),
        ).optional()?;
        Ok(result.flatten())
    }

    /// Returns true if the given hash is referenced by any change record
    /// (either as `old_hash` or `new_hash`).  Used before deleting an object
    /// to confirm it is truly unreferenced.
    pub fn is_hash_referenced(&self, hash: &str) -> RewResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE old_hash = ?1 OR new_hash = ?1",
            params![hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn get_latest_change_for_file(&self, file_path: &Path) -> RewResult<Option<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at
             FROM changes WHERE file_path = ?1 ORDER BY id DESC LIMIT 1",
        )?;

        let path_str = file_path.to_string_lossy().to_string();
        let result = stmt.query_row(params![path_str], |row| Ok(Self::row_to_change(row)));

        match result {
            Ok(change) => Ok(Some(change.map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))))?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(RewError::Database(e)),
        }
    }

    fn row_to_change(row: &rusqlite::Row) -> Result<Change, String> {
        let change_type_str: String = row.get(3).map_err(|e| e.to_string())?;
        let file_path_str: String = row.get(2).map_err(|e| e.to_string())?;

        let restored_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;
        let restored_at = restored_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        Ok(Change {
            id: row.get(0).map_err(|e| e.to_string())?,
            task_id: row.get(1).map_err(|e| e.to_string())?,
            file_path: PathBuf::from(file_path_str),
            change_type: change_type_str.parse()?,
            old_hash: row.get(4).map_err(|e| e.to_string())?,
            new_hash: row.get(5).map_err(|e| e.to_string())?,
            diff_text: row.get(6).map_err(|e| e.to_string())?,
            lines_added: row.get(7).map_err(|e| e.to_string())?,
            lines_removed: row.get(8).map_err(|e| e.to_string())?,
            restored_at,
        })
    }

    /// Mark a single file change as individually restored.
    /// Idempotent: calling again just updates the timestamp.
    pub fn mark_change_restored(
        &self,
        task_id: &str,
        file_path: &std::path::Path,
        restored_at: chrono::DateTime<chrono::Utc>,
    ) -> RewResult<()> {
        let path_str = file_path.to_string_lossy().to_string();
        self.conn.execute(
            "UPDATE changes SET restored_at = ?1 WHERE task_id = ?2 AND file_path = ?3",
            params![restored_at.to_rfc3339(), task_id, path_str],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SnapshotTrigger, TaskStatus, ChangeType};
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

    #[test]
    fn test_task_crud() {
        let db = test_db();
        let task = Task {
            id: "task_001".to_string(),
            prompt: Some("帮我重构 auth 模块".to_string()),
            tool: Some("claude-code".to_string()),
            started_at: Utc::now(),
            completed_at: None,
            status: TaskStatus::Active,
            risk_level: None,
            summary: None,
            cwd: None,
        };

        // Create
        db.create_task(&task).unwrap();

        // Read
        let loaded = db.get_task("task_001").unwrap().unwrap();
        assert_eq!(loaded.id, "task_001");
        assert_eq!(loaded.prompt.unwrap(), "帮我重构 auth 模块");
        assert_eq!(loaded.tool.unwrap(), "claude-code");
        assert_eq!(loaded.status, TaskStatus::Active);

        // Update status
        db.update_task_status("task_001", &TaskStatus::Completed, Some(Utc::now()))
            .unwrap();
        let updated = db.get_task("task_001").unwrap().unwrap();
        assert_eq!(updated.status, TaskStatus::Completed);
        assert!(updated.completed_at.is_some());

        // Update summary
        db.update_task_summary("task_001", "重构了认证中间件").unwrap();
        let summarized = db.get_task("task_001").unwrap().unwrap();
        assert_eq!(summarized.summary.unwrap(), "重构了认证中间件");

        // List
        let all = db.list_tasks().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_change_crud() {
        let db = test_db();

        // Create parent task first
        let task = Task {
            id: "task_002".to_string(),
            prompt: None,
            tool: None,
            started_at: Utc::now(),
            completed_at: None,
            status: TaskStatus::Active,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        // Insert changes
        let change1 = Change {
            id: None,
            task_id: "task_002".to_string(),
            file_path: PathBuf::from("/Users/test/project/src/auth.rs"),
            change_type: ChangeType::Created,
            old_hash: None,
            new_hash: Some("abc123".to_string()),
            diff_text: Some("+pub fn authenticate() {}".to_string()),
            lines_added: 1,
            lines_removed: 0,
            restored_at: None,
        };
        let id1 = db.insert_change(&change1).unwrap();
        assert!(id1 > 0);

        let change2 = Change {
            id: None,
            task_id: "task_002".to_string(),
            file_path: PathBuf::from("/Users/test/project/src/app.rs"),
            change_type: ChangeType::Modified,
            old_hash: Some("old456".to_string()),
            new_hash: Some("new789".to_string()),
            diff_text: Some("-use old;\n+use auth;".to_string()),
            lines_added: 1,
            lines_removed: 1,
            restored_at: None,
        };
        db.insert_change(&change2).unwrap();

        // Get changes for task
        let changes = db.get_changes_for_task("task_002").unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].change_type, ChangeType::Created);
        assert_eq!(changes[1].change_type, ChangeType::Modified);

        // Get latest change for file
        let latest = db
            .get_latest_change_for_file(Path::new("/Users/test/project/src/app.rs"))
            .unwrap()
            .unwrap();
        assert_eq!(latest.old_hash.unwrap(), "old456");
        assert_eq!(latest.new_hash.unwrap(), "new789");

        // Non-existent file
        let none = db
            .get_latest_change_for_file(Path::new("/nonexistent"))
            .unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn test_tables_coexist() {
        let db = test_db();

        // Verify all 3 tables exist
        let tables: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"snapshots".to_string()));
        assert!(tables.contains(&"tasks".to_string()));
        assert!(tables.contains(&"changes".to_string()));
    }
}
