//! SQLite database for snapshot and task metadata storage.
//!
//! Stores snapshot records, task records, and file change records
//! in ~/.rew/snapshots.db.

use crate::error::{RewError, RewResult};
use crate::types::{
    Change, Snapshot, SnapshotTrigger, Task, TaskStats, TaskStatus, TaskWithStats,
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

        // V5 migration: attribution column on changes (hook / fsevent_active / etc.)
        let _ = self.conn.execute(
            "ALTER TABLE changes ADD COLUMN attribution TEXT DEFAULT 'unknown'",
            [],
        );

        // V6: AI task statistics table
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_stats (
                task_id          TEXT PRIMARY KEY REFERENCES tasks(id),
                model            TEXT,
                duration_secs    REAL,
                tool_calls       INTEGER NOT NULL DEFAULT 0,
                files_changed    INTEGER NOT NULL DEFAULT 0,
                input_tokens     INTEGER,
                output_tokens    INTEGER,
                total_cost_usd   REAL,
                session_id       TEXT,
                conversation_id  TEXT,
                extra_json       TEXT
            );"
        )?;

        // V7: Session-based state management (replaces all temp files)
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS active_sessions (
                session_id      TEXT PRIMARY KEY,
                task_id         TEXT NOT NULL REFERENCES tasks(id),
                tool_source     TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                is_active       INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_active_sessions_task
                ON active_sessions(task_id);
            CREATE INDEX IF NOT EXISTS idx_active_sessions_active
                ON active_sessions(is_active);

            CREATE TABLE IF NOT EXISTS session_stop_guard (
                session_id      TEXT PRIMARY KEY,
                generation_id   TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pre_tool_hashes (
                session_id      TEXT NOT NULL,
                file_path       TEXT NOT NULL,
                content_hash    TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                PRIMARY KEY (session_id, file_path)
            );

            CREATE TABLE IF NOT EXISTS file_index (
                file_path       TEXT PRIMARY KEY,
                mtime_secs      INTEGER NOT NULL,
                fast_hash       TEXT NOT NULL,
                content_hash    TEXT
            );"
        )?;

        // V8: Move os_snapshot_ref from snapshots table to tasks table.
        let _ = self.conn.execute(
            "ALTER TABLE tasks ADD COLUMN os_snapshot_ref TEXT",
            [],
        );

        // V9: Performance index for get_latest_change_for_file + UNIQUE constraint.
        // Clean up any duplicate (task_id, file_path) rows first (keep highest id).
        let _ = self.conn.execute(
            "DELETE FROM changes WHERE id NOT IN (
                SELECT MAX(id) FROM changes GROUP BY task_id, file_path
            )",
            [],
        );
        let _ = self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_changes_file_path_id
                ON changes(file_path, id DESC)",
            [],
        );
        let _ = self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_changes_task_file_unique
                ON changes(task_id, file_path)",
            [],
        );

        // V10: old_file_path column for rename tracking
        let _ = self.conn.execute(
            "ALTER TABLE changes ADD COLUMN old_file_path TEXT",
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
    pub fn list_tasks(&self) -> RewResult<Vec<TaskWithStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.prompt, t.tool, t.started_at, t.completed_at,
                    t.status, t.risk_level, t.summary, t.cwd,
                    COUNT(c.id) AS changes_count,
                    COALESCE(SUM(c.lines_added), 0) AS total_lines_added,
                    COALESCE(SUM(c.lines_removed), 0) AS total_lines_removed
             FROM tasks t
             LEFT JOIN changes c ON c.task_id = t.id
             WHERE (t.tool != '文件监听' OR t.completed_at IS NOT NULL)
             GROUP BY t.id
             ORDER BY t.started_at DESC",
        )?;

        let mut tasks = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let ts = Self::row_to_task_with_stats(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))))?;
            tasks.push(ts);
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

    /// Set the APFS snapshot reference on a task.
    pub fn set_task_snapshot_ref(&self, task_id: &str, os_snapshot_ref: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE tasks SET os_snapshot_ref = ?1 WHERE id = ?2",
            params![os_snapshot_ref, task_id],
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

    /// Parse a row from a JOIN-aggregate query (columns 0..8 = task, 9..11 = stats).
    fn row_to_task_with_stats(row: &rusqlite::Row) -> Result<TaskWithStats, String> {
        let task = Self::row_to_task(row)?;
        Ok(TaskWithStats {
            task,
            changes_count: row.get::<_, i64>(9).map_err(|e| e.to_string())? as u32,
            total_lines_added: row.get::<_, i64>(10).map_err(|e| e.to_string())? as u32,
            total_lines_removed: row.get::<_, i64>(11).map_err(|e| e.to_string())? as u32,
        })
    }

    // ================================================================
    // Change CRUD (V2)
    // ================================================================

    /// Record a file change within a task.
    pub fn insert_change(&self, change: &Change) -> RewResult<i64> {
        self.conn.execute(
            "INSERT INTO changes (task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                change.task_id,
                change.file_path.to_string_lossy().to_string(),
                change.change_type.to_string(),
                change.old_hash,
                change.new_hash,
                change.diff_text,
                change.lines_added,
                change.lines_removed,
                change.attribution,
                change.old_file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Return the recorded `old_hash` for `(task_id, file_path)` if it already
    /// exists.  Used by hook.rs to compute cumulative line stats across multiple
    /// edits to the same file within the same task.
    pub fn get_task_file_old_hash(&self, task_id: &str, file_path: &Path) -> Option<String> {
        let path_str = file_path.to_string_lossy().to_string();
        self.conn.query_row(
            "SELECT old_hash FROM changes WHERE task_id = ?1 AND file_path = ?2",
            params![task_id, path_str],
            |row| row.get::<_, Option<String>>(0),
        ).optional().ok().flatten().flatten()
    }

    /// Return (change_type, old_hash) for a file within the current task.
    /// Used by resolve_baseline to distinguish Created (existed=false) from
    /// Modified/Renamed (existed=true) when a record already exists.
    pub fn get_task_file_baseline_info(
        &self,
        task_id: &str,
        file_path: &Path,
    ) -> RewResult<Option<(String, Option<String>)>> {
        let path_str = file_path.to_string_lossy().to_string();
        let result = self.conn.query_row(
            "SELECT change_type, old_hash FROM changes WHERE task_id = ?1 AND file_path = ?2",
            params![task_id, path_str],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
            )),
        ).optional()?;
        Ok(result)
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
            "SELECT id, old_hash, attribution FROM changes WHERE task_id = ?1 AND file_path = ?2",
            params![change.task_id, path_str],
            |row| Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            )),
        ).optional()?;

        if let Some((existing_id, existing_old_hash, existing_attribution)) = existing {
            let incoming_attr = change.attribution.as_deref().unwrap_or("unknown");
            let existing_attr = existing_attribution.as_deref().unwrap_or("unknown");

            // Priority: hook > fsevent_active > fsevent_grace > monitoring > unknown.
            // If existing record was written by hook, daemon writes must not
            // overwrite the diff/stats/attribution (hook data is more precise).
            if existing_attr == "hook" && incoming_attr != "hook" {
                return Ok(existing_id);
            }

            let preserved_old_hash = if incoming_attr == "hook" {
                // hook always provides the correct old_hash via get_task_file_old_hash
                change.old_hash.clone().or(existing_old_hash)
            } else {
                existing_old_hash.or(change.old_hash.clone())
            };

            self.conn.execute(
                "UPDATE changes SET change_type=?1, old_hash=?2, new_hash=?3,
                 diff_text=?4, lines_added=?5, lines_removed=?6,
                 attribution=COALESCE(?7, attribution),
                 old_file_path=COALESCE(?9, old_file_path)
                 WHERE id=?8",
                params![
                    change.change_type.to_string(),
                    preserved_old_hash,
                    change.new_hash,
                    change.diff_text,
                    change.lines_added,
                    change.lines_removed,
                    change.attribution,
                    existing_id,
                    change.old_file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
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
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at, attribution, old_file_path
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
    pub fn list_tasks_by_dir(&self, dir_prefix: &str) -> RewResult<Vec<TaskWithStats>> {
        let like_pattern = format!("{}/%", dir_prefix);
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.prompt, t.tool, t.started_at, t.completed_at,
                    t.status, t.risk_level, t.summary, t.cwd,
                    COUNT(c.id) AS changes_count,
                    COALESCE(SUM(c.lines_added), 0) AS total_lines_added,
                    COALESCE(SUM(c.lines_removed), 0) AS total_lines_removed
             FROM tasks t
             JOIN changes c ON c.task_id = t.id
             WHERE c.file_path LIKE ?1
               AND (t.tool != '文件监听' OR t.completed_at IS NOT NULL)
             GROUP BY t.id
             ORDER BY t.started_at DESC",
        )?;

        let mut tasks = Vec::new();
        let mut rows = stmt.query(params![like_pattern])?;
        while let Some(row) = rows.next()? {
            let ts = Self::row_to_task_with_stats(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)),
                )))?;
            tasks.push(ts);
        }

        Ok(tasks)
    }

    /// Filter tasks to those containing a specific file change (exact path match).
    pub fn list_tasks_by_file(&self, file_path: &str) -> RewResult<Vec<TaskWithStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.prompt, t.tool, t.started_at, t.completed_at,
                    t.status, t.risk_level, t.summary, t.cwd,
                    COUNT(c.id) AS changes_count,
                    COALESCE(SUM(c.lines_added), 0) AS total_lines_added,
                    COALESCE(SUM(c.lines_removed), 0) AS total_lines_removed
             FROM tasks t
             JOIN changes c ON c.task_id = t.id
             WHERE c.file_path = ?1
               AND (t.tool != '文件监听' OR t.completed_at IS NOT NULL)
             GROUP BY t.id
             ORDER BY t.started_at DESC",
        )?;

        let mut tasks = Vec::new();
        let mut rows = stmt.query(params![file_path])?;
        while let Some(row) = rows.next()? {
            let ts = Self::row_to_task_with_stats(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)),
                )))?;
            tasks.push(ts);
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
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at, attribution, old_file_path
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
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at, attribution, old_file_path
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
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at, attribution, old_file_path
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

    /// Like get_latest_change_for_file but excludes records from a specific task.
    /// Used by resolve_baseline to avoid circular reference (reading own task's
    /// records as "historical baseline").
    pub fn get_latest_change_for_file_excluding_task(
        &self,
        file_path: &Path,
        exclude_task_id: &str,
    ) -> RewResult<Option<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, restored_at, attribution, old_file_path
             FROM changes WHERE file_path = ?1 AND task_id != ?2 ORDER BY id DESC LIMIT 1",
        )?;

        let path_str = file_path.to_string_lossy().to_string();
        let result = stmt.query_row(params![path_str, exclude_task_id], |row| Ok(Self::row_to_change(row)));

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

        let attribution: Option<String> = row.get(10).unwrap_or(None);
        let old_file_path: Option<String> = row.get(11).unwrap_or(None);

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
            attribution,
            old_file_path: old_file_path.map(PathBuf::from),
        })
    }

    // ================================================================
    // TaskStats CRUD (V6)
    // ================================================================

    /// Create an initial stats row for a task (called at prompt time).
    pub fn create_task_stats(&self, stats: &TaskStats) -> RewResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO task_stats
             (task_id, model, duration_secs, tool_calls, files_changed,
              input_tokens, output_tokens, total_cost_usd,
              session_id, conversation_id, extra_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                stats.task_id,
                stats.model,
                stats.duration_secs,
                stats.tool_calls,
                stats.files_changed,
                stats.input_tokens,
                stats.output_tokens,
                stats.total_cost_usd,
                stats.session_id,
                stats.conversation_id,
                stats.extra_json,
            ],
        )?;
        Ok(())
    }

    /// Increment tool_calls counter by 1 for a task.
    pub fn increment_tool_calls(&self, task_id: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE task_stats SET tool_calls = tool_calls + 1 WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }

    /// Finalize stats at task stop: set duration_secs and files_changed.
    pub fn finalize_task_stats(
        &self,
        task_id: &str,
        duration_secs: f64,
        files_changed: i32,
    ) -> RewResult<()> {
        self.conn.execute(
            "UPDATE task_stats SET duration_secs = ?1, files_changed = ?2 WHERE task_id = ?3",
            params![duration_secs, files_changed, task_id],
        )?;
        Ok(())
    }

    /// Retrieve stats for a task.
    pub fn get_task_stats(&self, task_id: &str) -> RewResult<Option<TaskStats>> {
        let result = self.conn.query_row(
            "SELECT task_id, model, duration_secs, tool_calls, files_changed,
                    input_tokens, output_tokens, total_cost_usd,
                    session_id, conversation_id, extra_json
             FROM task_stats WHERE task_id = ?1",
            params![task_id],
            |row| {
                Ok(TaskStats {
                    task_id: row.get(0)?,
                    model: row.get(1)?,
                    duration_secs: row.get(2)?,
                    tool_calls: row.get(3)?,
                    files_changed: row.get(4)?,
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    total_cost_usd: row.get(7)?,
                    session_id: row.get(8)?,
                    conversation_id: row.get(9)?,
                    extra_json: row.get(10)?,
                })
            },
        ).optional()?;
        Ok(result)
    }

    /// Delete a single change record by its auto-increment ID.
    /// Used by reconcile to remove net-zero changes.
    pub fn delete_change_by_id(&self, id: i64) -> RewResult<()> {
        self.conn.execute(
            "DELETE FROM changes WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Update a change record after reconcile (change_type, new_hash, line stats).
    /// Does NOT touch old_hash or attribution — those are set at record time.
    pub fn update_change_reconciled(
        &self,
        id: i64,
        change_type: &crate::types::ChangeType,
        new_hash: Option<&str>,
        lines_added: u32,
        lines_removed: u32,
    ) -> RewResult<()> {
        self.conn.execute(
            "UPDATE changes SET change_type = ?1, new_hash = ?2,
             lines_added = ?3, lines_removed = ?4
             WHERE id = ?5",
            params![
                change_type.to_string(),
                new_hash,
                lines_added,
                lines_removed,
                id,
            ],
        )?;
        Ok(())
    }

    /// Update a change record to reflect a rename pairing (reconcile).
    /// Sets change_type=Renamed, old_file_path, old_hash, and recalculated line stats.
    pub fn update_change_rename_paired(
        &self,
        id: i64,
        old_file_path: &str,
        old_hash: Option<&str>,
        lines_added: u32,
        lines_removed: u32,
    ) -> RewResult<()> {
        self.conn.execute(
            "UPDATE changes SET change_type = 'renamed', old_file_path = ?1,
             old_hash = ?2, lines_added = ?3, lines_removed = ?4
             WHERE id = ?5",
            params![old_file_path, old_hash, lines_added, lines_removed, id],
        )?;
        Ok(())
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

    // ================================================================
    // Active Sessions (V7 — replaces session files)
    // ================================================================

    /// Register a new active session mapping session_id -> task_id.
    pub fn insert_active_session(
        &self,
        session_id: &str,
        task_id: &str,
        tool_source: &str,
        started_at: DateTime<Utc>,
    ) -> RewResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO active_sessions (session_id, task_id, tool_source, started_at, is_active)
             VALUES (?1, ?2, ?3, ?4, 1)",
            params![session_id, task_id, tool_source, started_at.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Get the task_id for an active session.
    pub fn get_active_task_for_session(&self, session_id: &str) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT task_id FROM active_sessions WHERE session_id = ?1 AND is_active = 1",
            params![session_id],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(result)
    }

    /// Deactivate all sessions for a given task_id.
    pub fn deactivate_sessions_for_task(&self, task_id: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE active_sessions SET is_active = 0 WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }

    /// Deactivate a specific session by session_id.
    pub fn deactivate_session(&self, session_id: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE active_sessions SET is_active = 0 WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Deactivate all sessions (used on startup recovery).
    pub fn deactivate_all_sessions(&self) -> RewResult<()> {
        self.conn.execute("UPDATE active_sessions SET is_active = 0", [])?;
        Ok(())
    }

    /// Get the most recently started active AI task_id (for daemon routing).
    pub fn get_most_recent_active_task_id(&self) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT task_id FROM active_sessions WHERE is_active = 1
             ORDER BY started_at DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(result)
    }

    /// Get an AI task that completed within `within_secs` seconds ago.
    /// Used as a grace-period fallback so delayed FSEvents still land in
    /// the just-completed AI task rather than a new monitoring window.
    pub fn get_recently_completed_task_id(&self, within_secs: i64) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT id FROM tasks
             WHERE tool IS NOT NULL AND tool != '文件监听'
               AND completed_at IS NOT NULL
               AND unixepoch(completed_at) IS NOT NULL
               AND unixepoch(completed_at) > unixepoch('now', ?1)
             ORDER BY completed_at DESC LIMIT 1",
            params![format!("-{} seconds", within_secs)],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(result)
    }

    /// Check if a file change with matching new_hash already exists in any
    /// active or recently completed task. Used by daemon to skip files already
    /// handled by hook or a recently finished task.
    pub fn is_change_already_recorded(
        &self,
        file_path: &str,
        new_hash: &str,
    ) -> RewResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes c
             JOIN tasks t ON c.task_id = t.id
             WHERE c.file_path = ?1
               AND c.new_hash = ?2
               AND (
                 t.id IN (SELECT task_id FROM active_sessions WHERE is_active = 1)
                 OR (t.completed_at IS NOT NULL
                     AND unixepoch(t.completed_at) IS NOT NULL
                     AND unixepoch(t.completed_at) > unixepoch('now', '-90 seconds'))
               )",
            params![file_path, new_hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    // ================================================================
    // Session Stop Guard (V7 — replaces .genid files)
    // ================================================================

    /// Store the generation_id for a session (Cursor stop-hook dedup).
    pub fn set_stop_guard(&self, session_id: &str, generation_id: &str) -> RewResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO session_stop_guard (session_id, generation_id)
             VALUES (?1, ?2)",
            params![session_id, generation_id],
        )?;
        Ok(())
    }

    /// Get the stored generation_id for a session.
    pub fn get_stop_guard(&self, session_id: &str) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT generation_id FROM session_stop_guard WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(result)
    }

    /// Remove the stop guard for a session (after stop completes).
    pub fn delete_stop_guard(&self, session_id: &str) -> RewResult<()> {
        self.conn.execute(
            "DELETE FROM session_stop_guard WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Remove all stop guards (used on startup recovery).
    pub fn delete_all_stop_guards(&self) -> RewResult<()> {
        self.conn.execute("DELETE FROM session_stop_guard", [])?;
        Ok(())
    }

    // ================================================================
    // Pre-tool Hashes (V7 — replaces .pre_tool/ files)
    // ================================================================

    /// Store the pre-tool content hash for a file in a session.
    pub fn set_pre_tool_hash(
        &self,
        session_id: &str,
        file_path: &str,
        content_hash: &str,
    ) -> RewResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO pre_tool_hashes (session_id, file_path, content_hash, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, file_path, content_hash, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Get the pre-tool content hash for a file in a session.
    pub fn get_pre_tool_hash(&self, session_id: &str, file_path: &str) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT content_hash FROM pre_tool_hashes WHERE session_id = ?1 AND file_path = ?2",
            params![session_id, file_path],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(result)
    }

    /// Consume (read and delete) the pre-tool hash for a file.
    pub fn consume_pre_tool_hash(&self, session_id: &str, file_path: &str) -> RewResult<Option<String>> {
        let hash = self.get_pre_tool_hash(session_id, file_path)?;
        if hash.is_some() {
            self.conn.execute(
                "DELETE FROM pre_tool_hashes WHERE session_id = ?1 AND file_path = ?2",
                params![session_id, file_path],
            )?;
        }
        Ok(hash)
    }

    /// Delete all pre-tool hashes for a session (cleanup on stop).
    pub fn delete_pre_tool_hashes_for_session(&self, session_id: &str) -> RewResult<()> {
        self.conn.execute(
            "DELETE FROM pre_tool_hashes WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Delete all pre-tool hashes (used on startup recovery).
    pub fn delete_all_pre_tool_hashes(&self) -> RewResult<()> {
        self.conn.execute("DELETE FROM pre_tool_hashes", [])?;
        Ok(())
    }

    // ================================================================
    // File Index (V7 — replaces scan_manifest.json)
    // ================================================================

    /// Upsert a file_index entry (path -> mtime + fast_hash + optional content_hash).
    pub fn upsert_file_index(
        &self,
        file_path: &str,
        mtime_secs: u64,
        fast_hash: &str,
        content_hash: Option<&str>,
    ) -> RewResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO file_index (file_path, mtime_secs, fast_hash, content_hash)
             VALUES (?1, ?2, ?3, ?4)",
            params![file_path, mtime_secs as i64, fast_hash, content_hash],
        )?;
        Ok(())
    }

    /// Get file_index entry for a path.
    pub fn get_file_index(&self, file_path: &str) -> RewResult<Option<(u64, String, Option<String>)>> {
        let result = self.conn.query_row(
            "SELECT mtime_secs, fast_hash, content_hash FROM file_index WHERE file_path = ?1",
            params![file_path],
            |row| Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            )),
        ).optional()?;
        Ok(result)
    }

    /// Get the fast_hash for a file from file_index (used by hook as old_hash fallback).
    /// Lazily persist a computed SHA-256 content_hash for a file_index entry.
    /// Called by resolve_baseline when upgrading from fast_hash to content_hash.
    pub fn update_file_index_content_hash(&self, file_path: &str, content_hash: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE file_index SET content_hash = ?1 WHERE file_path = ?2",
            params![content_hash, file_path],
        )?;
        Ok(())
    }

    pub fn get_file_index_hash(&self, file_path: &str) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT content_hash FROM file_index WHERE file_path = ?1",
            params![file_path],
            |row| row.get::<_, Option<String>>(0),
        ).optional()?;
        Ok(result.flatten())
    }

    /// Batch begin/commit for scanner performance (wraps in a transaction).
    pub fn begin_transaction(&self) -> RewResult<()> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    pub fn commit_transaction(&self) -> RewResult<()> {
        self.conn.execute_batch("COMMIT")?;
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
            attribution: None,
            old_file_path: None,
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
            attribution: None,
            old_file_path: None,
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
