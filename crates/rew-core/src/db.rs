//! SQLite database for snapshot and task metadata storage.
//!
//! Stores snapshot records, task records, and file change records
//! in ~/.rew/snapshots.db.

use crate::error::{RewError, RewResult};
use crate::types::{
    Change, RestoreOperation, RestoreOperationStatus, RestoreScopeType, RestoreTriggeredBy,
    Snapshot, SnapshotTrigger, Task, TaskStats, TaskStatus, TaskWithStats,
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

fn subpath_like_pattern(path_prefix: &str) -> String {
    if path_prefix == "/" {
        "/%".to_string()
    } else {
        format!("{}/%", path_prefix.trim_end_matches('/'))
    }
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = match conn.prepare(&pragma) {
        Ok(stmt) => stmt,
        Err(_) => return false,
    };
    let rows = match stmt.query_map([], |row| row.get::<_, String>(1)) {
        Ok(rows) => rows,
        Err(_) => return false,
    };
    let has_column = rows.flatten().any(|name| name == column);
    has_column
}

pub struct Database {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIndexEntry {
    pub file_path: PathBuf,
    pub mtime_secs: u64,
    pub fast_hash: String,
    pub content_hash: Option<String>,
    pub exists_now: bool,
    pub last_seen_at: Option<String>,
    pub last_event_kind: Option<String>,
    pub last_confirmed_by: Option<String>,
    pub deleted_at: Option<String>,
    pub last_scan_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDeletedDirSummary {
    pub dir_path: PathBuf,
    pub file_count: usize,
}

#[derive(Debug, Clone)]
pub struct InsightTaskRow {
    pub task_id: String,
    pub prompt: Option<String>,
    pub summary: Option<String>,
    pub tool: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_secs: Option<f64>,
    pub task_changes_count: u32,
    pub stat_files_changed: Option<u32>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFinalizationJob {
    pub task_id: String,
    pub stop_event_id: Option<String>,
    pub status: String,
    pub attempts: u32,
    pub enqueued_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RestoreFailureSample {
    pub file_path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreFileIndexSyncEntry {
    pub file_path: PathBuf,
    pub mtime_secs: u64,
    pub fast_hash: String,
    pub content_hash: Option<String>,
    pub deleted: bool,
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
        let has_task_changes_count = table_has_column(&self.conn, "tasks", "changes_count");
        let has_task_lines_added = table_has_column(&self.conn, "tasks", "lines_added");
        let has_task_lines_removed = table_has_column(&self.conn, "tasks", "lines_removed");

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
                lines_removed   INTEGER NOT NULL DEFAULT 0,
                attribution     TEXT DEFAULT 'unknown',
                old_file_path   TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_changes_task_id
                ON changes(task_id);

            CREATE INDEX IF NOT EXISTS idx_changes_file_path
                ON changes(file_path);",
        )?;

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
                content_hash    TEXT,
                exists_now      INTEGER NOT NULL DEFAULT 1,
                last_seen_at    TEXT,
                last_event_kind TEXT,
                last_confirmed_by TEXT,
                deleted_at      TEXT,
                last_scan_epoch INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_file_index_exists_now_path
                ON file_index(exists_now, file_path);
            CREATE INDEX IF NOT EXISTS idx_file_index_scan_epoch_path
                ON file_index(last_scan_epoch, file_path);"
        )?;

        // V8: Move os_snapshot_ref from snapshots table to tasks table.
        let _ = self.conn.execute(
            "ALTER TABLE tasks ADD COLUMN os_snapshot_ref TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE tasks ADD COLUMN changes_count INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE tasks ADD COLUMN lines_added INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE tasks ADD COLUMN lines_removed INTEGER NOT NULL DEFAULT 0",
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
            "CREATE INDEX IF NOT EXISTS idx_changes_task_file_path
                ON changes(task_id, file_path)",
            [],
        );
        let _ = self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_changes_task_old_file_path
                ON changes(task_id, old_file_path)",
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
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_deleted_dirs (
                task_id      TEXT NOT NULL REFERENCES tasks(id),
                dir_path     TEXT NOT NULL,
                file_count   INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (task_id, dir_path)
            );
            CREATE INDEX IF NOT EXISTS idx_task_deleted_dirs_task
                ON task_deleted_dirs(task_id);

            CREATE TABLE IF NOT EXISTS restore_operations (
                id                  TEXT PRIMARY KEY,
                source_task_id      TEXT NOT NULL REFERENCES tasks(id),
                scope_type          TEXT NOT NULL,
                scope_path          TEXT,
                triggered_by        TEXT NOT NULL,
                started_at          TEXT NOT NULL,
                completed_at        TEXT,
                status              TEXT NOT NULL,
                requested_count     INTEGER NOT NULL DEFAULT 0,
                restored_count      INTEGER NOT NULL DEFAULT 0,
                deleted_count       INTEGER NOT NULL DEFAULT 0,
                failed_count        INTEGER NOT NULL DEFAULT 0,
                failure_sample_json TEXT,
                metadata_json       TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_restore_operations_task_started
                ON restore_operations(source_task_id, started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_restore_operations_status_started
                ON restore_operations(status, started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_restore_operations_scope
                ON restore_operations(scope_type, scope_path);

            CREATE TABLE IF NOT EXISTS task_finalization_queue (
                task_id        TEXT PRIMARY KEY REFERENCES tasks(id),
                stop_event_id  TEXT,
                status         TEXT NOT NULL DEFAULT 'pending',
                enqueued_at    TEXT NOT NULL,
                started_at     TEXT,
                completed_at   TEXT,
                attempts       INTEGER NOT NULL DEFAULT 0,
                last_error     TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_task_finalization_queue_status_enqueued
                ON task_finalization_queue(status, enqueued_at);

            CREATE TABLE IF NOT EXISTS hook_event_receipts (
                idempotency_key TEXT PRIMARY KEY,
                event_type      TEXT NOT NULL,
                processed_at    TEXT NOT NULL
            );",
        )?;

        if table_has_column(&self.conn, "changes", "restored_at") {
            self.rebuild_changes_table_without_restored_at()?;
        }

        let _ = self.conn.execute(
            "ALTER TABLE file_index ADD COLUMN exists_now INTEGER NOT NULL DEFAULT 1",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE file_index ADD COLUMN last_seen_at TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE file_index ADD COLUMN last_event_kind TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE file_index ADD COLUMN last_confirmed_by TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE file_index ADD COLUMN deleted_at TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE file_index ADD COLUMN last_scan_epoch INTEGER",
            [],
        );
        let _ = self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_file_index_exists_now_path
                ON file_index(exists_now, file_path)",
            [],
        );
        let _ = self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_file_index_scan_epoch_path
                ON file_index(last_scan_epoch, file_path)",
            [],
        );

        if !has_task_changes_count || !has_task_lines_added || !has_task_lines_removed {
            self.backfill_task_rollups_from_changes()?;
        }

        Ok(())
    }

    fn rebuild_changes_table_without_restored_at(&self) -> RewResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_changes_task_id;
             DROP INDEX IF EXISTS idx_changes_file_path;
             DROP INDEX IF EXISTS idx_changes_file_path_id;
             DROP INDEX IF EXISTS idx_changes_task_file_path;
             DROP INDEX IF EXISTS idx_changes_task_old_file_path;
             DROP INDEX IF EXISTS idx_changes_task_file_unique;
             ALTER TABLE changes RENAME TO changes_old;
             CREATE TABLE changes (
                 id              INTEGER PRIMARY KEY AUTOINCREMENT,
                 task_id         TEXT NOT NULL REFERENCES tasks(id),
                 file_path       TEXT NOT NULL,
                 change_type     TEXT NOT NULL,
                 old_hash        TEXT,
                 new_hash        TEXT,
                 diff_text       TEXT,
                 lines_added     INTEGER NOT NULL DEFAULT 0,
                 lines_removed   INTEGER NOT NULL DEFAULT 0,
                 attribution     TEXT DEFAULT 'unknown',
                 old_file_path   TEXT
             );
             INSERT INTO changes (
                 id, task_id, file_path, change_type, old_hash, new_hash,
                 diff_text, lines_added, lines_removed, attribution, old_file_path
             )
             SELECT
                 id, task_id, file_path, change_type, old_hash, new_hash,
                 diff_text, lines_added, lines_removed, attribution, old_file_path
             FROM changes_old;
             DROP TABLE changes_old;
             CREATE INDEX idx_changes_task_id
                 ON changes(task_id);
             CREATE INDEX idx_changes_file_path
                 ON changes(file_path);
             CREATE INDEX idx_changes_file_path_id
                 ON changes(file_path, id DESC);
             CREATE INDEX idx_changes_task_file_path
                 ON changes(task_id, file_path);
             CREATE INDEX idx_changes_task_old_file_path
                 ON changes(task_id, old_file_path);
             CREATE UNIQUE INDEX idx_changes_task_file_unique
                 ON changes(task_id, file_path);",
        )?;
        tx.commit()?;
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
                    COALESCE(t.changes_count, 0) AS changes_count,
                    COALESCE(t.lines_added, 0) AS total_lines_added,
                    COALESCE(t.lines_removed, 0) AS total_lines_removed
             FROM tasks t
             WHERE (t.tool != '文件监听' OR t.completed_at IS NOT NULL)
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

    pub fn list_insight_tasks(
        &self,
        started_at_min: &str,
        started_at_max: &str,
    ) -> RewResult<Vec<InsightTaskRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.prompt, t.summary, t.tool, t.started_at, t.completed_at,
                    ts.duration_secs,
                    COALESCE(t.changes_count, 0) AS task_changes_count,
                    ts.files_changed,
                    ts.input_tokens,
                    ts.output_tokens,
                    ts.total_cost_usd,
                    ts.model
             FROM tasks t
             LEFT JOIN task_stats ts ON ts.task_id = t.id
             WHERE t.started_at >= ?1
               AND t.started_at <= ?2
               AND t.tool IS NOT NULL
               AND t.tool NOT IN ('文件监听', '手动存档')
               AND t.status != 'active'
             ORDER BY t.started_at DESC",
        )?;

        let mut rows = stmt.query(params![started_at_min, started_at_max])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let started_at_str: String = row.get(4)?;
            let completed_at_str: Option<String> = row.get(5)?;
            out.push(InsightTaskRow {
                task_id: row.get(0)?,
                prompt: row.get(1)?,
                summary: row.get(2)?,
                tool: row.get(3)?,
                started_at: parse_datetime_flexible(&started_at_str)
                    .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(
                        std::io::Error::new(std::io::ErrorKind::Other, e),
                    ))))?,
                completed_at: completed_at_str
                    .map(|s| parse_datetime_flexible(&s))
                    .transpose()
                    .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(
                        std::io::Error::new(std::io::ErrorKind::Other, e),
                    ))))?,
                duration_secs: row.get(6)?,
                task_changes_count: row.get::<_, i64>(7)? as u32,
                stat_files_changed: row.get::<_, Option<i64>>(8)?.map(|v| v as u32),
                input_tokens: row.get(9)?,
                output_tokens: row.get(10)?,
                total_cost_usd: row.get(11)?,
                model: row.get(12)?,
            });
        }
        Ok(out)
    }

    pub fn get_task_rollup(&self, task_id: &str) -> RewResult<(u32, u32, u32)> {
        let result = self.conn.query_row(
            "SELECT COALESCE(changes_count, 0), COALESCE(lines_added, 0), COALESCE(lines_removed, 0)
             FROM tasks WHERE id = ?1",
            params![task_id],
            |row| Ok((
                row.get::<_, i64>(0)? as u32,
                row.get::<_, i64>(1)? as u32,
                row.get::<_, i64>(2)? as u32,
            )),
        ).optional()?;
        Ok(result.unwrap_or((0, 0, 0)))
    }

    pub fn update_task_rollup(
        &self,
        task_id: &str,
        changes_count: u32,
        lines_added: u32,
        lines_removed: u32,
    ) -> RewResult<()> {
        self.conn.execute(
            "UPDATE tasks
             SET changes_count = ?1, lines_added = ?2, lines_removed = ?3
             WHERE id = ?4",
            params![changes_count as i64, lines_added as i64, lines_removed as i64, task_id],
        )?;
        Ok(())
    }

    pub fn refresh_task_rollup_from_changes(&self, task_id: &str) -> RewResult<(u32, u32, u32)> {
        let rollup = self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(lines_added), 0),
                    COALESCE(SUM(lines_removed), 0)
             FROM changes
             WHERE task_id = ?1",
            params![task_id],
            |row| Ok((
                row.get::<_, i64>(0)? as u32,
                row.get::<_, i64>(1)? as u32,
                row.get::<_, i64>(2)? as u32,
            )),
        )?;
        self.update_task_rollup(task_id, rollup.0, rollup.1, rollup.2)?;
        Ok(rollup)
    }

    fn backfill_task_rollups_from_changes(&self) -> RewResult<()> {
        self.conn.execute_batch(
            "UPDATE tasks
             SET changes_count = COALESCE((
                    SELECT COUNT(*) FROM changes c WHERE c.task_id = tasks.id
                 ), 0),
                 lines_added = COALESCE((
                    SELECT SUM(c.lines_added) FROM changes c WHERE c.task_id = tasks.id
                 ), 0),
                 lines_removed = COALESCE((
                    SELECT SUM(c.lines_removed) FROM changes c WHERE c.task_id = tasks.id
                 ), 0);",
        )?;
        Ok(())
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

    pub fn clear_task_summary(&self, id: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE tasks SET summary = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn enqueue_task_finalization(
        &self,
        task_id: &str,
        stop_event_id: Option<&str>,
    ) -> RewResult<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO task_finalization_queue (
                task_id, stop_event_id, status, enqueued_at, started_at, completed_at, attempts, last_error
             ) VALUES (?1, ?2, 'pending', ?3, NULL, NULL, 0, NULL)
             ON CONFLICT(task_id) DO UPDATE SET
                stop_event_id = excluded.stop_event_id,
                status = 'pending',
                enqueued_at = excluded.enqueued_at,
                started_at = NULL,
                completed_at = NULL,
                last_error = NULL",
            params![task_id, stop_event_id, now],
        )?;
        Ok(())
    }

    pub fn get_task_finalization_status(&self, task_id: &str) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT status FROM task_finalization_queue WHERE task_id = ?1",
            params![task_id],
            |row| row.get::<_, String>(0),
        ).optional()?;
        Ok(result)
    }

    pub fn recover_stale_task_finalizations(&self) -> RewResult<usize> {
        let now = Utc::now().to_rfc3339();
        let updated = self.conn.execute(
            "UPDATE task_finalization_queue
             SET status = 'pending',
                 enqueued_at = COALESCE(started_at, enqueued_at),
                 started_at = NULL,
                 completed_at = NULL,
                 last_error = COALESCE(last_error, ?1)
             WHERE status = 'running'",
            params![format!("worker interrupted before {}", now)],
        )?;
        Ok(updated)
    }

    pub fn claim_next_task_finalization_job(&self) -> RewResult<Option<TaskFinalizationJob>> {
        let tx = self.conn.unchecked_transaction()?;

        let next_job = {
            let mut stmt = tx.prepare(
                "SELECT task_id, stop_event_id, status, attempts, enqueued_at
                 FROM task_finalization_queue
                 WHERE status IN ('pending', 'failed')
                 ORDER BY enqueued_at ASC
                 LIMIT 1",
            )?;
            stmt.query_row([], |row| {
                Ok(TaskFinalizationJob {
                    task_id: row.get(0)?,
                    stop_event_id: row.get(1)?,
                    status: row.get(2)?,
                    attempts: row.get::<_, i64>(3)? as u32,
                    enqueued_at: row.get(4)?,
                })
            }).optional()?
        };

        let Some(job) = next_job else {
            tx.commit()?;
            return Ok(None);
        };

        let now = Utc::now().to_rfc3339();
        let updated = tx.execute(
            "UPDATE task_finalization_queue
             SET status = 'running',
                 started_at = ?1,
                 completed_at = NULL,
                 attempts = attempts + 1,
                 last_error = NULL
             WHERE task_id = ?2 AND status IN ('pending', 'failed')",
            params![now, job.task_id.as_str()],
        )?;

        if updated == 0 {
            tx.commit()?;
            return Ok(None);
        }

        tx.commit()?;

        Ok(Some(TaskFinalizationJob {
            attempts: job.attempts + 1,
            status: "running".to_string(),
            ..job
        }))
    }

    pub fn mark_task_finalization_done(&self, task_id: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE task_finalization_queue
             SET status = 'done',
                 completed_at = ?1,
                 last_error = NULL
             WHERE task_id = ?2",
            params![Utc::now().to_rfc3339(), task_id],
        )?;
        Ok(())
    }

    pub fn mark_task_finalization_failed(&self, task_id: &str, error: &str) -> RewResult<()> {
        self.conn.execute(
            "UPDATE task_finalization_queue
             SET status = 'failed',
                 completed_at = NULL,
                 last_error = ?1
             WHERE task_id = ?2",
            params![error, task_id],
        )?;
        Ok(())
    }

    pub fn claim_hook_event_receipt(
        &self,
        idempotency_key: &str,
        event_type: &str,
    ) -> RewResult<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO hook_event_receipts (idempotency_key, event_type, processed_at)
             VALUES (?1, ?2, ?3)",
            params![idempotency_key, event_type, Utc::now().to_rfc3339()],
        )?;
        Ok(inserted > 0)
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
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
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

    /// Get at most `limit` changes for a task.
    pub fn get_changes_for_task_limited(&self, task_id: &str, limit: usize) -> RewResult<Vec<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
             FROM changes WHERE task_id = ?1 ORDER BY id ASC LIMIT ?2",
        )?;

        let mut changes = Vec::new();
        let mut rows = stmt.query(params![task_id, limit as i64])?;
        while let Some(row) = rows.next()? {
            let change = Self::row_to_change(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))))?;
            changes.push(change);
        }

        Ok(changes)
    }

    pub fn count_changes_for_task(&self, task_id: &str) -> RewResult<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE task_id = ?1",
            params![task_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// List tasks that have at least one change under `dir_prefix`.
    pub fn list_tasks_by_dir(&self, dir_prefix: &str) -> RewResult<Vec<TaskWithStats>> {
        let like_pattern = subpath_like_pattern(dir_prefix);
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
        let like_pattern = subpath_like_pattern(dir_prefix);
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE task_id = ?1 AND file_path LIKE ?2",
            params![task_id, like_pattern],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn count_changes_matching_dir_scope(&self, task_id: &str, dir_prefix: &str) -> RewResult<usize> {
        let like_pattern = subpath_like_pattern(dir_prefix);
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes
             WHERE task_id = ?1
               AND (file_path LIKE ?2 OR old_file_path LIKE ?2)",
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
        let like_pattern = subpath_like_pattern(dir_prefix);
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
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

    pub fn get_changes_for_task_in_dir_for_restore(
        &self,
        task_id: &str,
        dir_prefix: &str,
    ) -> RewResult<Vec<Change>> {
        let like_pattern = subpath_like_pattern(dir_prefix);
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
             FROM changes
             WHERE task_id = ?1
               AND (file_path LIKE ?2 OR old_file_path LIKE ?2)
             ORDER BY id ASC",
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

    pub fn get_changes_for_task_in_dir_limited(
        &self,
        task_id: &str,
        dir_prefix: &str,
        limit: usize,
    ) -> RewResult<Vec<Change>> {
        let like_pattern = subpath_like_pattern(dir_prefix);
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
             FROM changes WHERE task_id = ?1 AND file_path LIKE ?2 ORDER BY id ASC LIMIT ?3",
        )?;

        let mut changes = Vec::new();
        let mut rows = stmt.query(params![task_id, like_pattern, limit as i64])?;
        while let Some(row) = rows.next()? {
            let change = Self::row_to_change(row)
                .map_err(|e| RewError::Database(rusqlite::Error::ToSqlConversionFailure(
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)),
                )))?;
            changes.push(change);
        }

        Ok(changes)
    }

    /// Returns true if any change exists for the exact `file_path`.
    pub fn has_exact_change_path(&self, file_path: &str) -> RewResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE file_path = ?1",
            params![file_path],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Returns true if any change exists below the `dir_prefix`.
    pub fn has_changes_under_dir_prefix(&self, dir_prefix: &str) -> RewResult<bool> {
        let like_pattern = subpath_like_pattern(dir_prefix);
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE file_path LIKE ?1",
            params![like_pattern],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// List files currently believed to exist under a directory prefix.
    pub fn list_live_files_under_dir(&self, dir_prefix: &Path) -> RewResult<Vec<PathBuf>> {
        let like_pattern = subpath_like_pattern(&dir_prefix.to_string_lossy());
        let mut stmt = self.conn.prepare(
            "SELECT file_path
             FROM file_index
             WHERE exists_now = 1 AND file_path LIKE ?1
             ORDER BY file_path ASC",
        )?;

        let paths = stmt
            .query_map(params![like_pattern], |row| row.get::<_, String>(0))?
            .filter_map(|row| row.ok().map(PathBuf::from))
            .collect();
        Ok(paths)
    }

    /// Get changes for a task matching a specific file path exactly.
    pub fn get_changes_for_task_by_file(&self, task_id: &str, file_path: &str) -> RewResult<Vec<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
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

    pub fn get_changes_for_task_by_file_limited(
        &self,
        task_id: &str,
        file_path: &str,
        limit: usize,
    ) -> RewResult<Vec<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
             FROM changes WHERE task_id = ?1 AND file_path = ?2 ORDER BY id ASC LIMIT ?3",
        )?;

        let mut changes = Vec::new();
        let mut rows = stmt.query(params![task_id, file_path, limit as i64])?;
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
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
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
            "SELECT id, task_id, file_path, change_type, old_hash, new_hash, diff_text, lines_added, lines_removed, attribution, old_file_path
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
        let attribution: Option<String> = row.get(9).unwrap_or(None);
        let old_file_path: Option<String> = row.get(10).unwrap_or(None);

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

    pub fn insert_restore_operation_started(
        &self,
        id: &str,
        source_task_id: &str,
        scope_type: &RestoreScopeType,
        scope_path: Option<&Path>,
        triggered_by: &RestoreTriggeredBy,
        started_at: DateTime<Utc>,
        requested_count: usize,
        metadata_json: Option<&str>,
    ) -> RewResult<()> {
        self.conn.execute(
            "INSERT INTO restore_operations (
                 id, source_task_id, scope_type, scope_path, triggered_by,
                 started_at, completed_at, status, requested_count,
                 restored_count, deleted_count, failed_count,
                 failure_sample_json, metadata_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'running', ?7, 0, 0, 0, NULL, ?8)",
            params![
                id,
                source_task_id,
                scope_type.to_string(),
                scope_path.map(|path| path.to_string_lossy().to_string()),
                triggered_by.to_string(),
                started_at.to_rfc3339(),
                requested_count as i64,
                metadata_json,
            ],
        )?;
        Ok(())
    }

    pub fn complete_restore_operation(
        &self,
        id: &str,
        status: &RestoreOperationStatus,
        completed_at: DateTime<Utc>,
        restored_count: usize,
        deleted_count: usize,
        failed_count: usize,
        failure_samples: &[RestoreFailureSample],
    ) -> RewResult<()> {
        let failure_sample_json = if failure_samples.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(failure_samples)
                    .map_err(|e| RewError::Config(format!("Failed to encode restore failures: {e}")))?,
            )
        };
        self.conn.execute(
            "UPDATE restore_operations
             SET completed_at = ?2,
                 status = ?3,
                 restored_count = ?4,
                 deleted_count = ?5,
                 failed_count = ?6,
                 failure_sample_json = ?7
             WHERE id = ?1",
            params![
                id,
                completed_at.to_rfc3339(),
                status.to_string(),
                restored_count as i64,
                deleted_count as i64,
                failed_count as i64,
                failure_sample_json,
            ],
        )?;
        Ok(())
    }

    pub fn list_restore_operations_for_task(
        &self,
        task_id: &str,
        limit: usize,
    ) -> RewResult<Vec<RestoreOperation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_task_id, scope_type, scope_path, triggered_by,
                    started_at, completed_at, status, requested_count,
                    restored_count, deleted_count, failed_count,
                    failure_sample_json, metadata_json
             FROM restore_operations
             WHERE source_task_id = ?1
             ORDER BY started_at DESC
             LIMIT ?2",
        )?;
        let mut rows = stmt.query(params![task_id, limit as i64])?;
        let mut operations = Vec::new();
        while let Some(row) = rows.next()? {
            let started_at = parse_datetime_flexible(&row.get::<_, String>(5)?)
                .map_err(|e| RewError::Config(format!("Invalid restore started_at: {e}")))?;
            let completed_at = row
                .get::<_, Option<String>>(6)?
                .map(|value| parse_datetime_flexible(&value))
                .transpose()
                .map_err(RewError::Config)?;
            let scope_path = row.get::<_, Option<String>>(3)?.map(PathBuf::from);
            operations.push(RestoreOperation {
                id: row.get(0)?,
                source_task_id: row.get(1)?,
                scope_type: row
                    .get::<_, String>(2)?
                    .parse()
                    .map_err(RewError::Config)?,
                scope_path,
                triggered_by: row
                    .get::<_, String>(4)?
                    .parse()
                    .map_err(RewError::Config)?,
                started_at,
                completed_at,
                status: row
                    .get::<_, String>(7)?
                    .parse()
                    .map_err(RewError::Config)?,
                requested_count: row.get::<_, i64>(8)? as u32,
                restored_count: row.get::<_, i64>(9)? as u32,
                deleted_count: row.get::<_, i64>(10)? as u32,
                failed_count: row.get::<_, i64>(11)? as u32,
                failure_sample_json: row.get(12)?,
                metadata_json: row.get(13)?,
            });
        }
        Ok(operations)
    }

    pub fn sync_file_index_after_restore_batch(
        &mut self,
        entries: &[RestoreFileIndexSyncEntry],
        seen_at: &str,
    ) -> RewResult<()> {
        let tx = self.conn.transaction()?;
        {
            let mut upsert_stmt = tx.prepare(
                "INSERT INTO file_index (
                     file_path, mtime_secs, fast_hash, content_hash,
                     exists_now, last_seen_at, last_event_kind, last_confirmed_by,
                     deleted_at, last_scan_epoch
                 )
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, 'restored_live', 'restore', NULL, NULL)
                 ON CONFLICT(file_path) DO UPDATE SET
                     mtime_secs = excluded.mtime_secs,
                     fast_hash = excluded.fast_hash,
                     content_hash = COALESCE(excluded.content_hash, file_index.content_hash),
                     exists_now = 1,
                     last_seen_at = excluded.last_seen_at,
                     last_event_kind = excluded.last_event_kind,
                     last_confirmed_by = excluded.last_confirmed_by,
                     deleted_at = NULL",
            )?;
            let mut delete_update_stmt = tx.prepare(
                "UPDATE file_index
                 SET exists_now = 0,
                     deleted_at = ?2,
                     last_seen_at = ?2,
                     last_event_kind = 'restored_delete',
                     last_confirmed_by = 'restore',
                     content_hash = COALESCE(content_hash, ?3)
                 WHERE file_path = ?1",
            )?;
            let mut delete_insert_stmt = tx.prepare(
                "INSERT INTO file_index (
                     file_path, mtime_secs, fast_hash, content_hash,
                     exists_now, last_seen_at, last_event_kind, last_confirmed_by,
                     deleted_at, last_scan_epoch
                 )
                 VALUES (?1, 0, ?2, ?3, 0, ?4, 'restored_delete', 'restore', ?4, NULL)",
            )?;

            for entry in entries {
                let path_str = entry.file_path.to_string_lossy().to_string();
                if entry.deleted {
                    let restore_hash = entry.content_hash.as_deref().or(Some(entry.fast_hash.as_str()));
                    let updated = delete_update_stmt.execute(params![
                        path_str,
                        seen_at,
                        restore_hash,
                    ])?;
                    if updated == 0 {
                        if let Some(hash) = restore_hash {
                            delete_insert_stmt.execute(params![path_str, hash, hash, seen_at])?;
                        }
                    }
                } else {
                    upsert_stmt.execute(params![
                        path_str,
                        entry.mtime_secs as i64,
                        entry.fast_hash,
                        entry.content_hash,
                        seen_at,
                    ])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_task_deleted_dir(
        &self,
        task_id: &str,
        dir_path: &Path,
        file_count: usize,
    ) -> RewResult<()> {
        self.conn.execute(
            "INSERT INTO task_deleted_dirs (task_id, dir_path, file_count)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(task_id, dir_path)
             DO UPDATE SET file_count = MAX(task_deleted_dirs.file_count, excluded.file_count)",
            params![task_id, dir_path.to_string_lossy().to_string(), file_count as i64],
        )?;
        Ok(())
    }

    pub fn list_task_deleted_dirs(&self, task_id: &str) -> RewResult<Vec<TaskDeletedDirSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT dir_path, file_count
             FROM task_deleted_dirs
             WHERE task_id = ?1
             ORDER BY file_count DESC, dir_path ASC",
        )?;

        let rows = stmt.query_map(params![task_id], |row| {
            Ok(TaskDeletedDirSummary {
                dir_path: PathBuf::from(row.get::<_, String>(0)?),
                file_count: row.get::<_, i64>(1)? as usize,
            })
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
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

    /// Upsert a live file_index entry and mark it as currently existing.
    pub fn upsert_live_file_index_entry(
        &self,
        file_path: &str,
        mtime_secs: u64,
        fast_hash: &str,
        content_hash: Option<&str>,
        source: &str,
        event_kind: &str,
        seen_at: &str,
        scan_epoch: Option<i64>,
    ) -> RewResult<()> {
        self.conn.execute(
            "INSERT INTO file_index (
                 file_path, mtime_secs, fast_hash, content_hash,
                 exists_now, last_seen_at, last_event_kind, last_confirmed_by,
                 deleted_at, last_scan_epoch
             )
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, NULL, ?8)
             ON CONFLICT(file_path) DO UPDATE SET
                 mtime_secs = excluded.mtime_secs,
                 fast_hash = excluded.fast_hash,
                 content_hash = COALESCE(excluded.content_hash, file_index.content_hash),
                 exists_now = 1,
                 last_seen_at = excluded.last_seen_at,
                 last_event_kind = excluded.last_event_kind,
                 last_confirmed_by = excluded.last_confirmed_by,
                 deleted_at = NULL,
                 last_scan_epoch = COALESCE(excluded.last_scan_epoch, file_index.last_scan_epoch)",
            params![
                file_path,
                mtime_secs as i64,
                fast_hash,
                content_hash,
                seen_at,
                event_kind,
                source,
                scan_epoch,
            ],
        )?;
        Ok(())
    }

    /// Tombstone a path in file_index while preserving the last recoverable hash.
    pub fn mark_file_index_deleted(
        &self,
        file_path: &str,
        restore_hash: Option<&str>,
        source: &str,
        event_kind: &str,
        deleted_at: &str,
    ) -> RewResult<()> {
        let updated = self.conn.execute(
            "UPDATE file_index
             SET exists_now = 0,
                 deleted_at = ?2,
                 last_seen_at = ?2,
                 last_event_kind = ?3,
                 last_confirmed_by = ?4,
                 content_hash = COALESCE(content_hash, ?5)
             WHERE file_path = ?1",
            params![file_path, deleted_at, event_kind, source, restore_hash],
        )?;

        if updated == 0 {
            if let Some(hash) = restore_hash {
                self.conn.execute(
                    "INSERT INTO file_index (
                         file_path, mtime_secs, fast_hash, content_hash,
                         exists_now, last_seen_at, last_event_kind, last_confirmed_by,
                         deleted_at, last_scan_epoch
                     )
                     VALUES (?1, 0, ?2, ?3, 0, ?4, ?5, ?6, ?4, NULL)",
                    params![file_path, hash, Some(hash), deleted_at, event_kind, source],
                )?;
            }
        }

        Ok(())
    }

    pub fn mark_file_index_renamed(
        &self,
        old_path: &str,
        new_path: &str,
        mtime_secs: u64,
        fast_hash: &str,
        content_hash: Option<&str>,
        source: &str,
        seen_at: &str,
    ) -> RewResult<()> {
        self.mark_file_index_deleted(old_path, content_hash.or(Some(fast_hash)), source, "rename_old", seen_at)?;
        self.upsert_live_file_index_entry(
            new_path,
            mtime_secs,
            fast_hash,
            content_hash,
            source,
            "rename_new",
            seen_at,
            None,
        )?;
        Ok(())
    }

    /// Tombstone paths under a directory that were not seen in the current scan epoch.
    pub fn tombstone_missing_paths_under_dir(
        &self,
        dir_prefix: &Path,
        scan_epoch: i64,
        tombstoned_at: &str,
    ) -> RewResult<usize> {
        let like_pattern = subpath_like_pattern(&dir_prefix.to_string_lossy());
        let updated = self.conn.execute(
            "UPDATE file_index
             SET exists_now = 0,
                 deleted_at = ?2,
                 last_seen_at = ?2,
                 last_event_kind = 'scan_missing',
                 last_confirmed_by = 'scanner'
             WHERE file_path LIKE ?1
               AND exists_now = 1
               AND COALESCE(last_scan_epoch, -1) != ?3",
            params![like_pattern, tombstoned_at, scan_epoch],
        )?;
        Ok(updated)
    }

    /// Get file_index entry for an active path.
    pub fn get_file_index(&self, file_path: &str) -> RewResult<Option<(u64, String, Option<String>)>> {
        let result = self.conn.query_row(
            "SELECT mtime_secs, fast_hash, content_hash
             FROM file_index
             WHERE file_path = ?1 AND exists_now = 1",
            params![file_path],
            |row| Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            )),
        ).optional()?;
        Ok(result)
    }

    pub fn get_file_index_entry(&self, file_path: &str) -> RewResult<Option<FileIndexEntry>> {
        let result = self.conn.query_row(
            "SELECT file_path, mtime_secs, fast_hash, content_hash, exists_now,
                    last_seen_at, last_event_kind, last_confirmed_by, deleted_at, last_scan_epoch
             FROM file_index
             WHERE file_path = ?1",
            params![file_path],
            |row| {
                Ok(FileIndexEntry {
                    file_path: PathBuf::from(row.get::<_, String>(0)?),
                    mtime_secs: row.get::<_, i64>(1)? as u64,
                    fast_hash: row.get(2)?,
                    content_hash: row.get(3)?,
                    exists_now: row.get::<_, i64>(4)? != 0,
                    last_seen_at: row.get(5)?,
                    last_event_kind: row.get(6)?,
                    last_confirmed_by: row.get(7)?,
                    deleted_at: row.get(8)?,
                    last_scan_epoch: row.get(9)?,
                })
            },
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
            "SELECT content_hash
             FROM file_index
             WHERE file_path = ?1 AND exists_now = 1",
            params![file_path],
            |row| row.get::<_, Option<String>>(0),
        ).optional()?;
        Ok(result.flatten())
    }

    pub fn get_file_index_restore_hash(&self, file_path: &str) -> RewResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT content_hash, fast_hash
             FROM file_index
             WHERE file_path = ?1",
            params![file_path],
            |row| Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, String>(1)?,
            )),
        ).optional()?;

        Ok(result.and_then(|(content_hash, fast_hash)| content_hash.or(Some(fast_hash))))
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
    use crate::types::{
        ChangeType, RestoreOperationStatus, RestoreScopeType, RestoreTriggeredBy, SnapshotTrigger,
        TaskStatus,
    };
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
    fn test_task_finalization_queue_claims_and_completes_jobs() {
        let db = test_db();
        let task = Task {
            id: "task_finalize".to_string(),
            prompt: Some("finalize".to_string()),
            tool: Some("cursor".to_string()),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        db.enqueue_task_finalization(&task.id, Some("stop-1")).unwrap();
        assert_eq!(
            db.get_task_finalization_status(&task.id).unwrap().as_deref(),
            Some("pending")
        );

        let claimed = db.claim_next_task_finalization_job().unwrap().unwrap();
        assert_eq!(claimed.task_id, task.id);
        assert_eq!(claimed.stop_event_id.as_deref(), Some("stop-1"));
        assert_eq!(claimed.status, "running");
        assert_eq!(claimed.attempts, 1);
        assert!(db.claim_next_task_finalization_job().unwrap().is_none());

        db.mark_task_finalization_done(&task.id).unwrap();
        assert_eq!(
            db.get_task_finalization_status(&task.id).unwrap().as_deref(),
            Some("done")
        );
    }

    #[test]
    fn test_recover_stale_task_finalizations_requeues_running_jobs() {
        let db = test_db();
        let task = Task {
            id: "task_finalize_recover".to_string(),
            prompt: Some("recover".to_string()),
            tool: Some("codebuddy".to_string()),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();
        db.enqueue_task_finalization(&task.id, Some("stop-2")).unwrap();
        let _ = db.claim_next_task_finalization_job().unwrap().unwrap();

        let recovered = db.recover_stale_task_finalizations().unwrap();
        assert_eq!(recovered, 1);
        assert_eq!(
            db.get_task_finalization_status(&task.id).unwrap().as_deref(),
            Some("pending")
        );
    }

    #[test]
    fn test_hook_event_receipt_claim_is_idempotent() {
        let db = test_db();
        assert!(db
            .claim_hook_event_receipt("prompt:cursor:abc", "prompt-started")
            .unwrap());
        assert!(!db
            .claim_hook_event_receipt("prompt:cursor:abc", "prompt-started")
            .unwrap());
    }

    #[test]
    fn test_refresh_task_rollup_from_changes_persists_summary_fields_on_tasks() {
        let db = test_db();
        let task = Task {
            id: "task_rollup".to_string(),
            prompt: Some("test".to_string()),
            tool: Some("cursor".to_string()),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "task_rollup".to_string(),
            file_path: PathBuf::from("/tmp/a.rs"),
            change_type: ChangeType::Modified,
            old_hash: Some("old".to_string()),
            new_hash: Some("new".to_string()),
            diff_text: None,
            lines_added: 3,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();
        db.insert_change(&Change {
            id: None,
            task_id: "task_rollup".to_string(),
            file_path: PathBuf::from("/tmp/b.rs"),
            change_type: ChangeType::Created,
            old_hash: None,
            new_hash: Some("new2".to_string()),
            diff_text: None,
            lines_added: 5,
            lines_removed: 0,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();

        let rollup = db.refresh_task_rollup_from_changes("task_rollup").unwrap();
        assert_eq!(rollup, (2, 8, 1));
        assert_eq!(db.get_task_rollup("task_rollup").unwrap(), (2, 8, 1));

        let listed = db.list_tasks().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].changes_count, 2);
        assert_eq!(listed[0].total_lines_added, 8);
        assert_eq!(listed[0].total_lines_removed, 1);
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

    #[test]
    fn test_has_changes_under_dir_prefix_and_exact_path() {
        let db = test_db();
        let task = Task {
            id: "task_dir_filter".to_string(),
            prompt: None,
            tool: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: task.id.clone(),
            file_path: PathBuf::from("/tmp/project/go/main.rs"),
            change_type: ChangeType::Deleted,
            old_hash: Some("old".into()),
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        }).unwrap();

        assert!(db.has_changes_under_dir_prefix("/tmp/project/go").unwrap());
        assert!(!db.has_changes_under_dir_prefix("/tmp/project/missing").unwrap());
        assert!(db.has_exact_change_path("/tmp/project/go/main.rs").unwrap());
        assert!(!db.has_exact_change_path("/tmp/project/go").unwrap());
    }

    #[test]
    fn test_list_live_files_under_dir_only_returns_active_entries() {
        let db = test_db();
        let task = Task {
            id: "task_known_dir".to_string(),
            prompt: None,
            tool: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        let now = Utc::now().to_rfc3339();
        db.upsert_live_file_index_entry(
            "/tmp/project/go/a.rs",
            1,
            "fast-a",
            Some("sha-a"),
            "scanner",
            "scan_seen",
            &now,
            Some(7),
        ).unwrap();
        db.upsert_live_file_index_entry(
            "/tmp/project/go/sub/b.rs",
            1,
            "fast-b",
            Some("sha-b"),
            "scanner",
            "scan_seen",
            &now,
            Some(7),
        ).unwrap();

        db.upsert_live_file_index_entry(
            "/tmp/project/go/runtime/c.rs",
            2,
            "sha-c",
            Some("sha-c"),
            "hook",
            "modified",
            &now,
            None,
        ).unwrap();
        db.mark_file_index_deleted(
            "/tmp/project/go/deleted/d.rs",
            Some("sha-d-old"),
            "daemon",
            "deleted",
            &now,
        ).unwrap();

        let files = db.list_live_files_under_dir(Path::new("/tmp/project/go")).unwrap();
        let rendered: Vec<String> = files
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        assert_eq!(
            rendered,
            vec![
                "/tmp/project/go/a.rs".to_string(),
                "/tmp/project/go/runtime/c.rs".to_string(),
                "/tmp/project/go/sub/b.rs".to_string(),
            ]
        );
    }

    #[test]
    fn test_mark_file_index_deleted_preserves_restore_hash() {
        let db = test_db();
        let now = Utc::now().to_rfc3339();

        db.mark_file_index_deleted(
            "/tmp/project/go/deleted.rs",
            Some("sha-deleted"),
            "daemon",
            "deleted",
            &now,
        ).unwrap();

        let entry = db.get_file_index_entry("/tmp/project/go/deleted.rs").unwrap().unwrap();
        assert!(!entry.exists_now);
        assert_eq!(entry.deleted_at.as_deref(), Some(now.as_str()));
        assert_eq!(entry.content_hash.as_deref(), Some("sha-deleted"));
        assert_eq!(
            db.get_file_index_restore_hash("/tmp/project/go/deleted.rs").unwrap().as_deref(),
            Some("sha-deleted")
        );
    }

    #[test]
    fn test_tombstone_missing_paths_under_dir_marks_unseen_rows_deleted() {
        let db = test_db();
        let now = Utc::now().to_rfc3339();

        db.upsert_live_file_index_entry(
            "/tmp/project/go/a.rs",
            1,
            "fast-a",
            Some("sha-a"),
            "scanner",
            "scan_seen",
            &now,
            Some(10),
        ).unwrap();
        db.upsert_live_file_index_entry(
            "/tmp/project/go/b.rs",
            1,
            "fast-b",
            Some("sha-b"),
            "scanner",
            "scan_seen",
            &now,
            Some(9),
        ).unwrap();

        let updated = db
            .tombstone_missing_paths_under_dir(Path::new("/tmp/project/go"), 10, &now)
            .unwrap();
        assert_eq!(updated, 1);

        let a = db.get_file_index_entry("/tmp/project/go/a.rs").unwrap().unwrap();
        let b = db.get_file_index_entry("/tmp/project/go/b.rs").unwrap().unwrap();
        assert!(a.exists_now);
        assert!(!b.exists_now);
        assert_eq!(b.last_event_kind.as_deref(), Some("scan_missing"));
    }

    #[test]
    fn test_restore_operations_keep_multiple_history_entries() {
        let db = test_db();
        let task = Task {
            id: "task_restore_history".to_string(),
            prompt: None,
            tool: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        let started_at = Utc::now();
        db.insert_restore_operation_started(
            "restore-op-1",
            &task.id,
            &RestoreScopeType::File,
            Some(Path::new("/tmp/project/a.rs")),
            &RestoreTriggeredBy::Ui,
            started_at,
            1,
            None,
        )
        .unwrap();
        db.complete_restore_operation(
            "restore-op-1",
            &RestoreOperationStatus::Completed,
            started_at,
            1,
            0,
            0,
            &[],
        )
        .unwrap();

        let later = started_at + chrono::Duration::seconds(5);
        db.insert_restore_operation_started(
            "restore-op-2",
            &task.id,
            &RestoreScopeType::Task,
            None,
            &RestoreTriggeredBy::Cli,
            later,
            3,
            None,
        )
        .unwrap();
        db.complete_restore_operation(
            "restore-op-2",
            &RestoreOperationStatus::Partial,
            later,
            2,
            0,
            1,
            &[RestoreFailureSample {
                file_path: PathBuf::from("/tmp/project/b.rs"),
                error: "permission denied".to_string(),
            }],
        )
        .unwrap();

        let operations = db.list_restore_operations_for_task(&task.id, 10).unwrap();
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0].id, "restore-op-2");
        assert_eq!(operations[0].triggered_by, RestoreTriggeredBy::Cli);
        assert_eq!(operations[1].id, "restore-op-1");
        assert_eq!(operations[1].triggered_by, RestoreTriggeredBy::Ui);
    }

    #[test]
    fn test_directory_restore_operation_is_single_audit_row() {
        let db = test_db();
        let task = Task {
            id: "task_directory_restore".to_string(),
            prompt: None,
            tool: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::Completed,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        db.insert_restore_operation_started(
            "restore-dir-1",
            &task.id,
            &RestoreScopeType::Directory,
            Some(Path::new("/tmp/project/go")),
            &RestoreTriggeredBy::Ui,
            Utc::now(),
            1200,
            None,
        )
        .unwrap();
        db.complete_restore_operation(
            "restore-dir-1",
            &RestoreOperationStatus::Completed,
            Utc::now(),
            1190,
            10,
            0,
            &[],
        )
        .unwrap();

        let operations = db.list_restore_operations_for_task(&task.id, 10).unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].scope_type, RestoreScopeType::Directory);
        assert_eq!(
            operations[0].scope_path.as_deref(),
            Some(Path::new("/tmp/project/go"))
        );
        assert_eq!(operations[0].requested_count, 1200);
    }

    #[test]
    fn test_mark_file_index_renamed_tombstones_old_and_upsserts_new() {
        let db = test_db();
        let seen_at = Utc::now().to_rfc3339();

        db.upsert_live_file_index_entry(
            "/tmp/project/old.txt",
            1,
            "sha-old",
            Some("sha-old"),
            "test",
            "seed",
            &seen_at,
            None,
        )
        .unwrap();

        db.mark_file_index_renamed(
            "/tmp/project/old.txt",
            "/tmp/project/new.txt",
            2,
            "sha-new",
            Some("sha-new"),
            "test",
            &seen_at,
        )
        .unwrap();

        let old_entry = db
            .get_file_index_entry("/tmp/project/old.txt")
            .unwrap()
            .unwrap();
        let new_entry = db
            .get_file_index_entry("/tmp/project/new.txt")
            .unwrap()
            .unwrap();
        assert!(!old_entry.exists_now);
        assert_eq!(old_entry.last_event_kind.as_deref(), Some("rename_old"));
        assert!(new_entry.exists_now);
        assert_eq!(new_entry.last_event_kind.as_deref(), Some("rename_new"));
        assert_eq!(new_entry.content_hash.as_deref(), Some("sha-new"));
    }
}
