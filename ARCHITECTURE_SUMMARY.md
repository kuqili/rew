# REW Project Architecture Summary

**Last Updated:** April 9, 2026

---

## 1. Task Data Model (`crates/rew-core/src/types.rs`)

### Task Structure
```rust
pub struct Task {
    pub id: String,                           // Unique identifier (nanoid-style short ID)
    pub prompt: Option<String>,               // User's original prompt text
    pub tool: Option<String>,                 // AI tool name (e.g. "claude-code", "cursor")
    pub started_at: DateTime<Utc>,            // When task started
    pub completed_at: Option<DateTime<Utc>>,  // When task completed (None if active)
    pub status: TaskStatus,                   // Current status (see below)
    pub risk_level: Option<RiskLevel>,        // Risk assessment
    pub summary: Option<String>,              // AI-generated summary of changes
}
```

### TaskStatus Enum
```rust
pub enum TaskStatus {
    Active,              // Task currently being executed by AI
    Completed,           // Task completed normally
    RolledBack,          // Task fully rolled back
    PartialRolledBack,   // Some changes rolled back
}
```

### RiskLevel Enum
```rust
pub enum RiskLevel {
    Low,
    Medium,
    High,
}
```

### Change Structure
```rust
pub struct Change {
    pub id: Option<i64>,                      // Auto-increment ID
    pub task_id: String,                      // Foreign key to tasks table
    pub file_path: PathBuf,                   // Absolute path of affected file
    pub change_type: ChangeType,              // Type of change (see below)
    pub old_hash: Option<String>,             // SHA-256 hash before change
    pub new_hash: Option<String>,             // SHA-256 hash after change
    pub diff_text: Option<String>,            // Unified diff text (text files only)
    pub lines_added: u32,                     // Lines added
    pub lines_removed: u32,                   // Lines removed
    pub restored_at: Option<DateTime<Utc>>,   // Timestamp of individual restoration
}
```

### ChangeType Enum
```rust
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
    Renamed,
}
```

**Key Design Points:**
- One user prompt = one task
- Each task contains all file changes made during execution
- Content hashes point to `.rew/objects/` for content-addressable backup storage
- Supports individual file restoration tracking via `restored_at` timestamp
- File-monitoring windows use `tool = "文件监听"` (Chinese for "file monitoring")

---

## 2. Database Schema (`crates/rew-core/src/db.rs`)

### Database Tables

#### `snapshots` Table
```sql
CREATE TABLE snapshots (
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

-- Indexes
CREATE INDEX idx_snapshots_timestamp ON snapshots(timestamp DESC);
CREATE INDEX idx_snapshots_trigger ON snapshots(trigger_type);
CREATE INDEX idx_snapshots_pinned ON snapshots(pinned);
```

#### `tasks` Table (V2 Core)
```sql
CREATE TABLE tasks (
    id              TEXT PRIMARY KEY,
    prompt          TEXT,
    tool            TEXT,
    started_at      TEXT NOT NULL,
    completed_at    TEXT,
    status          TEXT NOT NULL DEFAULT 'active',
    risk_level      TEXT,
    summary         TEXT
);

-- Indexes
CREATE INDEX idx_tasks_started_at ON tasks(started_at DESC);
CREATE INDEX idx_tasks_status ON tasks(status);
```

#### `changes` Table (V2 Core)
```sql
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
    restored_at     TEXT  -- V3 migration: single-file restoration tracking
);

-- Indexes
CREATE INDEX idx_changes_task_id ON changes(task_id);
CREATE INDEX idx_changes_file_path ON changes(file_path);
```

### Key Database Operations

**Task CRUD:**
- `create_task()` - Create new task
- `get_task(id)` - Get task by ID
- `list_tasks()` - List all tasks (excluding 文件监听 without completed_at)
- `update_task_status(id, status, completed_at)` - Update status
- `update_task_summary(id, summary)` - Update summary
- `get_latest_monitoring_window()` - Get most recent fs_* task

**Change CRUD:**
- `insert_change()` - Record file change
- `upsert_change()` - Insert or update (preserves original old_hash)
- `get_changes_for_task(task_id)` - Get all changes in task
- `get_changes_for_task_in_dir(task_id, dir_prefix)` - Filter by directory
- `get_changes_for_task_by_file(task_id, file_path)` - Filter by file
- `mark_change_restored(task_id, file_path, restored_at)` - Mark individual file as restored

**Filtering:**
- `list_tasks_by_dir(dir_prefix)` - Tasks with changes in directory
- `list_tasks_by_file(file_path)` - Tasks with changes to specific file
- `count_changes_in_dir(task_id, dir_prefix)` - Count changes in directory
- `count_changes_for_file(task_id, file_path)` - Count changes for file

**Datetime Handling:**
- Flexible parser: `parse_datetime_flexible()` handles RFC3339, ISO8601, and naive formats
- All timestamps stored as RFC3339 strings in SQLite
- WAL mode enabled for better concurrent reads

---

## 3. Frontend Display Components (`gui/src/components/`)

### TaskTimeline.tsx
The main timeline view showing tasks.

**Props:**
```typescript
interface Props {
  selectedId: string | null;        // Currently selected task ID
  onSelect: (id: string) => void;   // Callback when task selected
  dirFilter?: string | null;        // Optional directory filter
}
```

**Features:**
- **Two view modes**: "scheduled" (monitoring windows) vs "ai" (AI tasks)
- **Date filtering**: Today, Yesterday, Last 24h, Last 7 days, or Custom date
- **Calendar picker** with month navigation and quick shortcuts
- **Timeline visualization**: Vertical timeline with dots and connecting lines
  - Rolled-back tasks: red outline dots
  - Active tasks: yellow filled dots
  - Monitoring windows: gray outline dots
  - Completed AI tasks: blue filled dots
- **Task row displays**:
  - Vertical timeline dot (left column)
  - Description: truncated prompt or summary (middle)
  - File count (right column)
  - Absolute timestamp in local time (far right)

**Filtering Logic:**
- Monitoring windows identified by: `task.tool === "文件监听"`
- Effective timestamp for windows: `completed_at ?? started_at`
- Effective timestamp for AI tasks: `started_at`

**Status Badges:**
- "已读档" (Rolled back): Red background
- "进行中" (Active): Yellow background
- Tool label (Claude Code/Cursor): Blue background

### TaskDetail.tsx
Detailed view showing changes within a task.

**Props:**
```typescript
interface Props {
  taskId: string;                   // Task to display
  dirFilter?: string | null;        // Optional filter
  onTaskUpdated: () => void;        // Callback after restore
  onBack: () => void;               // Close detail view
}
```

**Layout: SourceTree-style two-pane**
- **Left pane (resizable, ~280px default)**:
  - File list with change icons (A/M/D/R for Added/Modified/Deleted/Renamed)
  - Lines added (+) and removed (-) stats
  - Individual file restore buttons with two-step confirmation
  - Shows restoration timestamp if previously restored
- **Right pane**: Unified diff viewer for selected file
- **Divider**: Drag-to-resize between panes

**Header Info:**
- Tool badge (Claude Code/Cursor)
- Status badges (rolled-back, file-monitoring)
- File count and aggregate stats (total lines added/removed)
- Time ago (relative timestamp)
- Rollback button

**Change Icons:**
```
A (Added):    green background
M (Modified): yellow background
D (Deleted):  red background
R (Renamed):  gray background
```

**Rollback Features:**
- Preview rollback with list of files to restore/delete
- Full task rollback or individual file restoration
- Two-step confirmation for destructive operations
- Shows "上次读档: X时间前" (Last restored: X time ago) when applicable

---

## 4. Tauri IPC Commands (`src-tauri/src/commands.rs`)

### Data Transfer Objects (DTOs)

#### TaskInfo (sent to frontend)
```typescript
interface TaskInfo {
  id: string;
  prompt: string | null;
  tool: string | null;
  started_at: string;                    // RFC3339 ISO timestamp
  completed_at: string | null;
  status: string;                        // "active"|"completed"|"rolled-back"|"partial-rolled-back"
  risk_level: string | null;
  summary: string | null;
  changes_count: usize;                  // Precomputed count
}
```

#### ChangeInfo (sent to frontend)
```typescript
interface ChangeInfo {
  id: number | null;
  task_id: string;
  file_path: string;                     // String path (not PathBuf)
  change_type: string;                   // "created"|"modified"|"deleted"|"renamed"
  old_hash: string | null;
  new_hash: string | null;
  diff_text: string | null;
  lines_added: u32;
  lines_removed: u32;
  restored_at: string | null;            // ISO8601 timestamp or null
}
```

#### UndoPreviewInfo
```typescript
interface UndoPreviewInfo {
  task_id: string;
  total_changes: usize;
  files_to_restore: string[];            // Paths to restore from backup
  files_to_delete: string[];             // Paths that were added and should be deleted
}
```

#### UndoResultInfo
```typescript
interface UndoResultInfo {
  files_restored: usize;
  files_deleted: usize;
  failures: [string, string][];          // [(path, error_message)]
}
```

#### ChangeDiffResult
```typescript
interface ChangeDiffResult {
  diff_text: string | null;              // Unified diff or null for binary
  lines_added: u32;                      // Re-computed
  lines_removed: u32;                    // Re-computed
}
```

### Task-Related Commands

#### `list_tasks(dir_filter?: string) -> Vec<TaskInfo>`
- Lists all tasks ordered by started_at DESC
- Excludes currently-accumulating monitoring window (compared against `AppState.fs_window_task`)
- Optional `dir_filter`: filter by directory or file path
- If filter is file path: calls `list_tasks_by_file()`
- If filter is directory: calls `list_tasks_by_dir()`
- Includes precomputed `changes_count` for each task

**Key Behavior:**
- Never returns in-progress monitoring window to avoid showing it multiple times
- Excludes 文件监听 tasks with `completed_at IS NULL` from main list

#### `get_task(task_id: String) -> TaskInfo`
- Retrieve single task by ID
- Includes precomputed changes_count

#### `get_task_changes(task_id: String, dir_filter?: String) -> Vec<ChangeInfo>`
- Get all changes for a task
- Optional directory/file filter
- Returns changes with `restored_at` timestamps

#### `get_change_diff(task_id: String, file_path: String) -> ChangeDiffResult`
- On-demand diff computation from hashes
- Reads old/new content from `.rew/objects/`
- Computes unified diff using `rew_core::diff::compute_diff()`
- Returns null diff_text for binary files
- Re-computes line counts (may differ from stale DB values)

### Rollback Commands

#### `preview_rollback(task_id: String) -> UndoPreviewInfo`
- Shows what would happen if task is rolled back
- Lists files to restore (had old content) and to delete (were created)

#### `rollback_task_cmd(task_id: String) -> UndoResultInfo`
- Restore all changes in task to pre-task state
- Adds all affected paths to `suppressed_paths` (60s TTL) to avoid spurious FSEvents
- Sets global `rolling_back` flag for 3 seconds
- Returns count of restored/deleted files and failures

#### `restore_file_cmd(task_id: String, file_path: String) -> UndoResultInfo`
- Restore single file to pre-task state
- Persists `restored_at` timestamp to database
- Suppresses FSEvents for that path for 60 seconds
- Clears global `rolling_back` flag after 3 seconds

**Legacy aliases (backward compatible):**
- `preview_undo()` → `preview_rollback()`
- `undo_task_cmd()` → `rollback_task_cmd()`
- `undo_file_cmd()` → `restore_file_cmd()`

### Monitoring Window Commands

#### `get_monitoring_window() -> u64`
- Returns monitoring window duration in seconds
- Read from config

#### `set_monitoring_window(secs: u64) -> ()`
- Set monitoring window (clamped 60–7200 seconds)
- Seals current monitoring window immediately
- Prevents situations where shorter window would create seal_time in past

### Scan & Directory Commands

#### `add_watch_dir(dir_path: String) -> ()`
- Add directory to watch list
- Detects overlaps: rejects subdirs of existing watches
- Absorbs existing watches if new path is parent
- Marks as "pending" in scan progress
- Spawns background full scan
- Hot-adds to running FSEvents pipeline

#### `remove_watch_dir(dir_path: String) -> ()`
- Remove from watch list
- Updates config file
- Removes from scan progress
- Hot-removes from FSEvents pipeline

#### `get_scan_progress() -> ScanProgressInfo`
- Returns per-directory scan status
- Includes: pending/scanning/complete status, file counts, percentages

#### `rescan_watch_dir(dir_path: String) -> ()`
- Re-scan single directory
- Respects current exclude config

### Frontend Invocation Helper (TypeScript)
```typescript
// Example usage in frontend
import { listTasks, getTask, getTaskChanges, rollbackTask } from "./lib/tauri";

const tasks = await listTasks();                    // All tasks
const filtered = await listTasks("/path/to/dir");  // Only tasks affecting /path/to/dir
const task = await getTask("task_123");            // Single task
const changes = await getTaskChanges("task_123");  // Changes in task
const preview = await previewRollback("task_123"); // What if rollback?
const result = await rollbackTask("task_123");     // Execute rollback
const result2 = await restoreFile("task_123", "/path/to/file"); // Single file
```

---

## 5. Frontend-Backend Data Flow

### Initialization
1. Frontend calls `list_tasks()` with optional `dirFilter`
2. Backend checks `AppState.fs_window_task` to exclude in-progress window
3. Backend queries database with appropriate SQL (all / by_dir / by_file)
4. Backend counts changes for each task
5. Returns `TaskInfo[]` with precomputed `changes_count`

### Viewing Task Details
1. User clicks task in timeline
2. Frontend calls `getTask(taskId)` to get full task info
3. Frontend calls `getTaskChanges(taskId)` to get file list
4. Frontend auto-selects first file
5. Frontend calls `getChangeDiff(taskId, filePath)` on demand when file selected
6. Backend reads hashes from DB, loads content from objects store, computes diff

### Restoring Files
1. User clicks "读档" (restore) button on a file
2. Two-step confirmation:
   - First click: show "覆盖当前版本？" (Overwrite current version?)
   - Second click: execute restore
3. Frontend calls `restoreFile(taskId, filePath)`
4. Backend:
   - Executes restore via `TaskRestoreEngine::undo_file()`
   - Marks `restored_at` in database
   - Adds path to `suppressed_paths` (60s TTL)
   - Sets global `rolling_back` flag
5. Frontend shows "✓ 已读档" success feedback
6. Calls `onRestored()` callback to refresh change list

### Full Task Rollback
1. User clicks "↩ 读档" button in header
2. Shows `RollbackPanel` with preview
3. Frontend calls `previewRollback(taskId)`
4. Frontend calls `rollbackTask(taskId)` on confirmation
5. Backend restores all files and deletes created files
6. Frontend dismisses panel and refreshes task list

---

## 6. Key Integration Points

### State Management
- **AppState** (Rust/Tauri):
  - `db: Mutex<Database>` - SQLite connection
  - `fs_window_task: Mutex<Option<FsWindowTask>>` - Currently accumulating monitoring window
  - `rolling_back: Mutex<bool>` - Global flag during restore operations
  - `suppressed_paths: Mutex<Map<PathBuf, Instant>>` - Paths to ignore in FSEvents
  - `scan_progress: Mutex<ScanProgress>` - Per-directory scan state
  - `config: Mutex<RewConfig>` - Configuration

### Frontend State
- **useTasks hook**: Fetches and caches task list
- **useTaskChanges hook**: Fetches changes for specific task
- **Component state**: Selected task ID, selected file path, diff content

### Timeline Rendering
- Tasks grouped by date (today/yesterday/custom)
- Split into two tabs: "定时存档" (scheduled) vs "AI 任务" (AI tasks)
- Vertical timeline visualization with dots and connecting lines
- Click to select and view details

### Diff Display
- DiffViewer component renders unified diff text
- Color-coded: red for removed lines, green for added lines
- Line numbers with context

---

## 7. Special Features

### Monitoring Windows (文件监听)
- Special tasks with `tool = "文件监听"`
- Represent periodic file surveillance intervals
- Display time range as "HH:mm – HH:mm" or just "HH:mm"
- Hidden from main list if `completed_at IS NULL` (still accumulating)
- Format: `formatWindowTime()` uses `completed_at ?? started_at`

### Individual File Restoration
- `restored_at` field tracks when file was restored
- Multiple restores allowed ("读档可重复")
- Shows "上次读档: X时间前" hint
- Two-step confirmation UI prevents accidents

### Risk Level Assessment
- Tasks can have Low/Medium/High risk
- Set during task completion
- Displayed in UI (future enhancement)

### Change Type Icons
- A (Added): green
- M (Modified): yellow
- D (Deleted): red
- R (Renamed): gray

### Content-Addressable Storage
- All file contents hashed with SHA-256
- Stored in `.rew/objects/` directory
- Enables deduplication and integrity verification
- Frontend can compute diffs by fetching from object store

---

## 8. Code Paths for Common Operations

### Adding a task
1. Daemon detects file change via FSEvents
2. Creates Change record with task_id
3. Updates task status/summary
4. Calls `db.upsert_change()` to preserve original `old_hash`

### Viewing timeline
1. Frontend: `useTasks()` hook
2. Calls `listTasks(dirFilter?)`
3. Backend queries `tasks` table + joins to count changes
4. Returns sorted list with precomputed counts

### Rolling back one file
1. Frontend: User clicks restore button in file list
2. Backend: `restore_file_cmd(task_id, file_path)`
3. Calls `TaskRestoreEngine::undo_file()`
4. Reads `old_hash` from DB
5. Loads content from `.rew/objects/`
6. Writes to original location
7. Updates `restored_at` timestamp
8. Adds to `suppressed_paths` to prevent FSEvent re-recording

---

## 9. Important Design Notes

1. **Task Granularity**: One task = one user prompt (or one monitoring window)
2. **Change Tracking**: Each file can have one record per task (via upsert logic)
3. **Original State Preserved**: `old_hash` always points to state before task (never overwritten)
4. **Monitoring Window Exclusion**: In-progress windows (文件监听) are hidden from timeline until sealed
5. **Datetime Flexibility**: Parser handles multiple formats for robustness
6. **Content Hashing**: SHA-256 enables content-addressed storage and integrity
7. **Two-Step Confirmation**: Rollback operations require confirmation to prevent accidents
8. **Suppression TTL**: Recently restored paths ignored for 60s to avoid spurious timeline entries
9. **Resizable Panes**: TaskDetail uses drag-to-resize between file list and diff viewer
10. **Precomputed Counts**: `changes_count` computed in backend to avoid N+1 queries

