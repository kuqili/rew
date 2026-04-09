# REW Project: Complete Architecture Exploration

**Date:** 2026-04-09
**Project:** rew (Intelligent File Recovery & Task Tracking System)

---

## 1. TASK DATA MODEL (`crates/rew-core/src/types.rs`)

### Core Types

#### TaskStatus (Enum)
```
- Active: Task is currently being executed by the AI
- Completed: Task completed normally
- RolledBack: Task has been fully rolled back
- PartialRolledBack: Some changes in the task were rolled back
```

#### RiskLevel (Enum)
```
- Low
- Medium
- High
```

#### ChangeType (Enum)
```
- Created: File was newly created
- Modified: File was modified
- Deleted: File was deleted
- Renamed: File was renamed
```

#### Task Structure (Lines 288-305)
```rust
pub struct Task {
    pub id: String,                              // Unique identifier (nanoid-style short ID)
    pub prompt: Option<String>,                  // User's original prompt text (from hook)
    pub tool: Option<String>,                    // AI tool name (e.g. "claude-code", "cursor")
    pub started_at: DateTime<Utc>,               // When the task started
    pub completed_at: Option<DateTime<Utc>>,     // When task completed (None if still active)
    pub status: TaskStatus,                      // Current status
    pub risk_level: Option<RiskLevel>,           // Risk level assessment
    pub summary: Option<String>,                 // AI-generated summary of changes
}
```

**V2 Design Notes:**
- Each task represents one user prompt → AI execution cycle
- Contains the user's original intent + AI tool used
- All file changes made during task are tracked separately

#### Change Structure (Lines 307-334)
```rust
pub struct Change {
    pub id: Option<i64>,                         // Auto-increment ID
    pub task_id: String,                         // Which task this belongs to
    pub file_path: PathBuf,                      // Absolute path of affected file
    pub change_type: ChangeType,                 // Type of change
    pub old_hash: Option<String>,                // SHA-256 hash of file before change
    pub new_hash: Option<String>,                // SHA-256 hash of file after change
    pub diff_text: Option<String>,               // Unified diff text (text files only)
    pub lines_added: u32,                        // Lines added
    pub lines_removed: u32,                      // Lines removed
    pub restored_at: Option<DateTime<Utc>>,      // Set when file was individually restored
}
```

**Key Design:**
- Tracks content via content-addressable backup (hashes point to `.rew/objects/`)
- One record per file per task (maintained via upsert operation)
- Supports single-file rollback tracking

---

## 2. DATABASE SCHEMA (`crates/rew-core/src/db.rs`)

### Tables Created

#### snapshots Table (Lines 58-68)
```sql
CREATE TABLE IF NOT EXISTS snapshots (
    id               TEXT PRIMARY KEY,
    timestamp        TEXT NOT NULL,
    trigger_type     TEXT NOT NULL,
    os_snapshot_ref  TEXT NOT NULL,
    files_added      INTEGER NOT NULL DEFAULT 0,
    files_modified   INTEGER NOT NULL DEFAULT 0,
    files_deleted    INTEGER NOT NULL DEFAULT 0,
    pinned           INTEGER NOT NULL DEFAULT 0,
    metadata_json    TEXT
)
```
**Indexes:**
- idx_snapshots_timestamp (DESC)
- idx_snapshots_trigger
- idx_snapshots_pinned

#### tasks Table (Lines 80-89)
```sql
CREATE TABLE IF NOT EXISTS tasks (
    id              TEXT PRIMARY KEY,
    prompt          TEXT,
    tool            TEXT,
    started_at      TEXT NOT NULL,
    completed_at    TEXT,
    status          TEXT NOT NULL DEFAULT 'active',
    risk_level      TEXT,
    summary         TEXT
)
```
**Indexes:**
- idx_tasks_started_at (DESC)
- idx_tasks_status

#### changes Table (Lines 98-108)
```sql
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
    restored_at     TEXT                          -- V3 migration: single-file restore tracking
)
```
**Indexes:**
- idx_changes_task_id
- idx_changes_file_path

### Database Methods (Key Operations)

**Task CRUD:**
- `create_task(task: &Task)` - Insert new task
- `get_task(id: &str)` - Fetch task by ID
- `list_tasks()` - Get all tasks (excludes active monitoring windows with tool='文件监听')
- `update_task_status(id, status, completed_at)` - Update task completion
- `update_task_summary(id, summary)` - Set AI summary after completion

**Change Operations:**
- `insert_change(change: &Change)` - Record file change
- `upsert_change(change: &Change)` - Insert or update (preserves original old_hash)
- `get_changes_for_task(task_id)` - Get all changes in task
- `get_changes_for_task_in_dir(task_id, dir_prefix)` - Filtered by directory
- `get_changes_for_task_by_file(task_id, file_path)` - Get changes for specific file
- `get_latest_change_for_file(file_path)` - Shadow lookup
- `mark_change_restored(task_id, file_path, restored_at)` - Track individual restore

**Filtering:**
- `list_tasks_by_dir(dir_prefix)` - Tasks affecting a directory
- `list_tasks_by_file(file_path)` - Tasks affecting a file
- `count_changes_in_dir(task_id, dir_prefix)` - Count changes in directory
- `count_changes_for_file(task_id, file_path)` - Count changes for file

**Monitoring Window:**
- `get_latest_monitoring_window()` - Get most recent fs_* task
- `seal_null_monitoring_windows(completed_at)` - Clean up crashed windows
- `update_task_completed_at(id, completed_at)` - Seal monitoring window

### Database Features
- WAL mode enabled for concurrent read performance
- Flexible datetime parsing (supports multiple formats)
- Transaction support through Connection API
- All datetimes stored as RFC3339 strings

---

## 3. FRONTEND DISPLAY COMPONENTS

### TaskTimeline Component (`gui/src/components/TaskTimeline.tsx`)

**Purpose:** Main timeline view showing tasks with date filtering and view modes

**Props:**
```typescript
interface Props {
  selectedId: string | null;           // Currently selected task ID
  onSelect: (id: string) => void;      // Callback when user selects task
  dirFilter?: string | null;            // Optional directory filter
}
```

**State:**
- `viewMode`: "scheduled" | "ai" - Toggle between monitoring windows and AI tasks
- `dateFilter`: DateMode ("today" | "yesterday" | "24h" | "7d" | "custom")

**Key Features:**
1. **Two View Modes:**
   - "定时存档" (Scheduled archives): File monitoring windows only (tool === "文件监听")
   - "AI 任务" (AI tasks): All non-monitoring-window tasks

2. **Date Filtering:**
   - Calendar date picker (Monday-based grid)
   - Quick shortcuts: Today, Yesterday, Last 24h, Last 7 days
   - Custom date selection

3. **Task Row Display:**
   ```
   [Timeline Graph] | [Description] | [File Count] | [Timestamp]
   ```
   - Graph column shows connecting lines + status dots:
     - Red circle (border): rolled-back status
     - Yellow circle: active status
     - Gray circle (border): monitoring window
     - Blue circle: completed task
   - Description: prompt text (truncated) or monitoring window time
   - Tool badges: "Claude Code", "Cursor"
   - Status badges: "已读档" (rolled-back), "进行中" (active)
   - File count: "N 文件"
   - Time: Absolute local timestamp (YYYY-MM-DD HH:mm)

4. **Monitoring Window Filtering:**
   - Excludes currently-active monitoring window (not yet sealed)
   - Only sealed windows (with completed_at set) appear in timeline

**Data Expected:**
```typescript
interface TaskInfo {
  id: string;
  prompt: string | null;
  tool: string | null;
  started_at: string;                 // ISO-8601
  completed_at: string | null;        // ISO-8601
  status: "active" | "completed" | "rolled-back" | "partial-rolled-back";
  risk_level: string | null;
  summary: string | null;
  changes_count: number;              // Total changes in this task
}
```

### TaskDetail Component (`gui/src/components/TaskDetail.tsx`)

**Purpose:** Detailed view of a single task with file list and diff viewer

**Props:**
```typescript
interface Props {
  taskId: string;
  dirFilter?: string | null;
  onTaskUpdated: () => void;
  onBack: () => void;
}
```

**Layout:**
```
┌─────────────────────────────────────────────────────────┐
│ [Badges] | [Title/Prompt] | [Stats] | [Rollback Button]│ ← Header
├─────────────┬───────────────────────────────────────────┤
│ File List   │ Diff Viewer (Unified Diff Format)         │ ← Body
│ (280px)     │ (Resizable)                               │
├─────────────┼───────────────────────────────────────────┤
│ [~4px drag] │ (resizable divider)                       │
└─────────────┴───────────────────────────────────────────┘
```

**Header Features:**
- Tool badge: "Claude Code", "Cursor", or "文件监听"
- Status badges: "已读档", "进行中"
- Task title: prompt or window label (start – end time)
- Stats: "N 个文件", "+X lines", "-Y lines", "time ago"
- Rollback button (states: "↩ 读档", "↩ 再次读档")

**File List Column:**
- Header: "变更文件 (N)" - Change count
- Per-file row showing:
  - Change type icon: "A"=Added (green), "M"=Modified (yellow), "D"=Deleted (red), "R"=Renamed (gray)
  - Filename (monospace, truncated)
  - Directory path (dimmed, hidden on small screens)
  - Line counts: "+X", "-Y"
  - Restore button with two-step confirmation (only for modified/deleted with old_hash)
  - Restored hint: "上次读档: X ago"

**Diff Pane:**
- Header shows: Change icon | Filename | Directory | Line counts
- Content: Unified diff or "binary file" message
- Auto-loads diff when file selected

**Data Expected:**
```typescript
interface ChangeInfo {
  id: number;
  task_id: string;
  file_path: string;
  change_type: "created" | "modified" | "deleted" | "renamed";
  old_hash: string | null;
  new_hash: string | null;
  diff_text: string | null;
  lines_added: number;
  lines_removed: number;
  restored_at: string | null;          // ISO-8601 when individually restored
}
```

**Interactive Features:**
- Horizontal drag-to-resize (160px min, 75% max)
- File selection drives diff display
- Two-step restore confirmation prevents accidents
- "已读档" state persists (restored_at field)

---

## 4. TAURI IPC COMMANDS

**Backend:** `src-tauri/src/commands.rs`
**Frontend Types:** `gui/src/lib/tauri.ts`

### Task-Related Commands (Lines 321-715)

#### Queries

**`list_tasks(dir_filter?: string) -> Vec<TaskInfo>`**
- Returns tasks excluding currently-accumulating monitoring window
- Filters by dir or file if dir_filter provided
- Respects directory/file filter logic (is_file_filter check)
- Calculates changes_count based on filter
- Logs debug info to trace backend filtering

**`get_task(task_id: string) -> TaskInfo`**
- Fetch single task by ID with full change count

**`get_task_changes(task_id: string, dir_filter?: string) -> Vec<ChangeInfo>`**
- Get all changes for task (optionally filtered by dir/file)
- Returns full ChangeInfo with restored_at timestamps

**`get_change_diff(task_id: string, file_path: string) -> ChangeDiffResult`**
- On-demand diff computation:
  - Reads old_hash and new_hash from DB
  - Retrieves content from ObjectStore (`.rew/objects/`)
  - Computes unified diff via `rew_core::diff::compute_diff`
  - Returns diff_text or None for binary files
  - Re-computes lines_added/lines_removed (may update stale DB values)

**Returns:**
```rust
pub struct ChangeDiffResult {
    pub diff_text: Option<String>,      // Unified diff text
    pub lines_added: u32,
    pub lines_removed: u32,
}
```

#### Rollback Commands

**`preview_rollback(task_id: string) -> UndoPreviewInfo`**
- Preview what rollback will affect
- Uses `TaskRestoreEngine::preview_undo()`
- Returns files to restore, files to delete

**`rollback_task_cmd(task_id: string) -> UndoResultInfo`**
- Full task rollback
- Sets rolling_back flag → waits 3s → clears
- Adds affected paths to suppressed_paths (60s TTL)
- Prevents async FSEvent spurious timeline entries after restore

**`restore_file_cmd(task_id: string, file_path: string) -> UndoResultInfo`**
- Single-file restore (not full task)
- Marks change.restored_at in DB
- Adds path to suppressed_paths
- Clears rolling_back flag after 3s delay

**Returns:**
```rust
pub struct UndoResultInfo {
    pub files_restored: usize,
    pub files_deleted: usize,
    pub failures: Vec<(String, String)>,  // (path, error)
}
```

**Legacy Aliases:**
- `preview_undo` → `preview_rollback`
- `undo_task_cmd` → `rollback_task_cmd`
- `undo_file_cmd` → `restore_file_cmd`

#### Monitoring Window Configuration

**`get_monitoring_window() -> u64`**
- Returns monitoring_window_secs from config

**`set_monitoring_window(secs: u64) -> ()`**
- Clamps to range: 60–7200 seconds (1 min – 2 hours)
- Seals currently-open monitoring window immediately at now
- Prevents new window_secs producing seal_time in past

### Data Models for Frontend

**TaskInfo** (sent from backend):
```rust
pub struct TaskInfo {
    pub id: String,
    pub prompt: Option<String>,
    pub tool: Option<String>,
    pub started_at: String,             // RFC3339
    pub completed_at: Option<String>,   // RFC3339
    pub status: String,                 // "active", "completed", "rolled-back", etc.
    pub risk_level: Option<String>,
    pub summary: Option<String>,
    pub changes_count: usize,           // Calculated server-side
}
```

**ChangeInfo** (sent from backend):
```rust
pub struct ChangeInfo {
    pub id: Option<i64>,
    pub task_id: String,
    pub file_path: String,
    pub change_type: String,            // "created", "modified", "deleted", "renamed"
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub diff_text: Option<String>,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub restored_at: Option<String>,    // ISO-8601 timestamp or null
}
```

**UndoPreviewInfo** (sent from backend):
```rust
pub struct UndoPreviewInfo {
    pub task_id: String,
    pub total_changes: usize,
    pub files_to_restore: Vec<String>,  // Absolute paths
    pub files_to_delete: Vec<String>,   // Absolute paths
}
```

---

## 5. KEY ARCHITECTURAL PATTERNS

### V2 Task Model
- **One task = One user prompt** (+ one AI execution cycle)
- Task ID format: nanoid-style short ID, or special prefixes:
  - `fs_*` = File monitoring window ID
  - `manual_*` = Manual snapshot ID

### Task Exclusion Logic
- **Live monitoring windows hidden from timeline:**
  - Frontend: Comparison against active_window_id in AppState
  - Backend: Stores most-recent fs_* task in state, excludes from list_tasks
  - Only sealed windows (completed_at set) appear in UI

### Content-Addressable Storage
- File content backed up by SHA-256 hash in `.rew/objects/`
- old_hash = state before task started
- new_hash = state after task completed
- Supports dedup and efficient storage via APFS clonefile

### Single-File Restore Tracking
- Each Change has `restored_at: Option<DateTime<Utc>>`
- Set when user clicks "↩ 读档" on individual file
- Persisted to DB via `mark_change_restored()`
- UI shows: "上次读档: X ago"

### Suppressed Paths (FSEvents Suppression)
- After rollback, affected paths added to suppressed_paths HashMap
- 60-second TTL prevents immediate re-capture of restored file
- Prevents spurious timeline entries from async FSEvent delivery

### Rollback State Management
- Global rolling_back flag in AppState
- Set true before operation, cleared after 3-second delay
- Prevents concurrent rollback attempts during operation

---

## 6. FRONTEND-BACKEND DATA FLOW

### Task List Load
```
Frontend: useTasks(dirFilter)
    ↓
Tauri: list_tasks(dirFilter)
    ↓
Backend: 
  - Query tasks (filtered by dir/file if needed)
  - Exclude active monitoring window
  - Calculate changes_count per task
  - Convert to TaskInfo DTOs
    ↓
Frontend: 
  - Filter by date range (inDateRange)
  - Split by view mode (scheduled vs AI)
  - Render TaskTimeline rows
```

### Task Detail Load
```
Frontend: TaskDetail(taskId)
    ↓
Parallel requests:
  - Tauri: get_task(taskId) → TaskInfo
  - Tauri: get_task_changes(taskId, dirFilter) → Vec<ChangeInfo>
    ↓
Frontend:
  - Set task header badges/stats
  - Populate file list with auto-selected first file
  - Lazy-load diff on file select
    ↓
On file select:
  - Tauri: get_change_diff(taskId, filePath) → ChangeDiffResult
    ↓
Frontend: Render DiffViewer with unified diff text
```

### Rollback Flow
```
Frontend: Click "↩ 读档" button
    ↓
Tauri: preview_rollback(taskId) → UndoPreviewInfo
    ↓
Frontend: Show RollbackPanel confirmation
    ↓
User confirms:
  - For full rollback: Tauri: rollback_task_cmd(taskId)
  - For single file: Tauri: restore_file_cmd(taskId, filePath)
    ↓
Backend:
  - Restore files from ObjectStore
  - Mark rolled-back in DB
  - Add to suppressed_paths
  - Emit task-updated event
    ↓
Frontend:
  - Call onTaskUpdated() to refresh timeline
  - Show success notification
  - Update task status badge to "已读档"
```

---

## 7. FILTERING & QUERYING

### Directory Filtering Logic
```typescript
// Backend checks: is this a directory or file?
const is_file_filter = dir_filter && !Path::new(path).is_dir();

if is_file_filter {
    // Query by exact file path
    db.list_tasks_by_file(path)
    db.get_changes_for_task_by_file(task_id, path)
} else if dir_filter {
    // Query by directory prefix (LIKE "dir/%")
    db.list_tasks_by_dir(dir_prefix)
    db.get_changes_for_task_in_dir(task_id, dir_prefix)
} else {
    // No filter: get everything
    db.list_tasks()
    db.get_changes_for_task(task_id)
}
```

### Date Filtering (Frontend Only)
```typescript
type DateMode = "today" | "yesterday" | "24h" | "7d" | "custom";

function inDateRange(task, filter):
  - "today": dayOf(task_ts) === todayStr()
  - "yesterday": dayOf(task_ts) === yesterdayStr()
  - "24h": now - task_ts < 86_400_000 ms
  - "7d": now - task_ts < 7 * 86_400_000 ms
  - "custom": dayOf(task_ts) === filter.date
```

---

## 8. SPECIAL CASES & EDGE CASES

### Monitoring Windows (文件监听)
- **Tool name:** "文件监听" (File Monitoring)
- **Excluded from timeline:** Unless completed_at is set
- **Effective timestamp:** completed_at (when sealed) or started_at (if still active)
- **Purpose:** Track file changes during scheduled intervals between AI tasks
- **Display:** Time range like "10:30 – 10:45" or single time if start=end

### Manual Snapshots (手动存档)
- **ID prefix:** "manual_" + timestamp
- **Tool name:** "手动存档" (Manual Snapshot)
- **Created by:** `create_manual_snapshot()` command
- **Seals current window:** Before creating snapshot task
- **Purpose:** User-initiated checkpoint between AI tasks

### Binary Files in Diffs
- `get_change_diff` returns diff_text = None for binary files
- Frontend DiffViewer detects None and shows "binary file" message
- Old/new hashes still available for content lookup

### Rolled-Back Tasks
- Status changes to "rolled-back" or "partial-rolled-back"
- Restored_at timestamps persisted for each file
- Rollback operation is idempotent (can rollback same task multiple times)
- Badge shows "已读档" and buttons show "↩ 再次读档"

---

## Summary

The rew project implements a sophisticated task-and-change tracking system:

1. **Core Model:** Tasks encapsulate user prompts and AI executions, with granular file change tracking
2. **Storage:** SQLite DB with content-addressable blob storage via SHA-256 hashes
3. **Frontend:** SourceTree-style dual-pane interface (timeline + detail)
4. **IPC:** Tauri commands provide query, diff, and rollback operations
5. **State Management:** AppState tracks live monitoring windows, suppressed paths, and rolling-back flag
6. **Special Handling:** Monitoring windows are hidden from timeline until sealed; manual snapshots and rollback operations are fully tracked and reversible

