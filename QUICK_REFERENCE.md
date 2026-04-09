# REW Architecture - Quick Reference Guide

## Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                         FRONTEND (React)                            │
├─────────────────────────────────────────────────────────────────────┤
│  TaskTimeline.tsx              TaskDetail.tsx                       │
│  ├─ List view                  ├─ File list (left pane)            │
│  ├─ Date filtering             ├─ Diff viewer (right pane)         │
│  └─ Click to select            └─ Restore buttons                  │
└──────────────────────┬──────────────────────────────────────────────┘
                       │ Tauri IPC invoke()
                       ▼
┌─────────────────────────────────────────────────────────────────────┐
│                   TAURI BACKEND (Rust)                             │
├─────────────────────────────────────────────────────────────────────┤
│  src-tauri/src/commands.rs                                          │
│  ├─ list_tasks()          → TaskInfo[]                             │
│  ├─ get_task(id)          → TaskInfo                               │
│  ├─ get_task_changes()    → ChangeInfo[]                           │
│  ├─ get_change_diff()     → ChangeDiffResult                       │
│  ├─ rollback_task_cmd()   → UndoResultInfo                         │
│  └─ restore_file_cmd()    → UndoResultInfo                         │
└──────────────────────┬──────────────────────────────────────────────┘
                       │ SQL queries
                       ▼
┌─────────────────────────────────────────────────────────────────────┐
│              DATABASE (SQLite)                                      │
├─────────────────────────────────────────────────────────────────────┤
│  tasks table                                                        │
│  ├─ id (PK)                                                         │
│  ├─ prompt, tool, summary                                          │
│  ├─ started_at, completed_at                                       │
│  └─ status, risk_level                                             │
│                                                                     │
│  changes table                                                      │
│  ├─ id (PK), task_id (FK)                                          │
│  ├─ file_path, change_type                                         │
│  ├─ old_hash, new_hash, diff_text                                  │
│  ├─ lines_added, lines_removed                                     │
│  └─ restored_at (individual restore timestamp)                     │
└─────────────────────────────────────────────────────────────────────┘
```

## Object Relationships

```
Task (one user prompt or monitoring window)
  │
  ├─ id: "task_123" or "fs_001"
  ├─ prompt: "帮我重构 auth 模块" (user's original instruction)
  ├─ tool: "claude-code" | "cursor" | "文件监听" (file monitoring)
  ├─ started_at: 2026-04-09T10:00:00Z
  ├─ completed_at: 2026-04-09T10:15:00Z (null if still active)
  ├─ status: "active" | "completed" | "rolled-back" | "partial-rolled-back"
  ├─ risk_level: "low" | "medium" | "high"
  ├─ summary: "重构了认证中间件" (AI-generated)
  │
  └─ Changes (one per file affected)
       │
       ├─ id: 42 (auto-increment)
       ├─ file_path: "/path/to/auth.rs"
       ├─ change_type: "created" | "modified" | "deleted" | "renamed"
       ├─ old_hash: "abc123..." (SHA-256, null if created)
       ├─ new_hash: "def456..." (SHA-256, null if deleted)
       ├─ diff_text: "@@ -1,5 +1,10 @@\n-old\n+new\n..."
       ├─ lines_added: 10
       ├─ lines_removed: 3
       └─ restored_at: 2026-04-09T11:30:00Z (null if never restored)
```

## Frontend Components Communication

```
MainLayout
├─ TaskTimeline (left sidebar)
│  └─ Emits: onSelect(taskId)
│
├─ TaskDetail (main view)
│  ├─ Left pane: FileListRow (each file)
│  │  ├─ Shows change icon (A/M/D/R)
│  │  ├─ Shows lines +/- stats
│  │  └─ Shows restore button
│  │
│  └─ Right pane: DiffViewer
│     └─ Shows unified diff
│
└─ RollbackPanel (modal, shown on demand)
   └─ Preview + confirm buttons
```

## Key Tauri Commands (Frontend Perspective)

```typescript
// Timeline view
const tasks = await listTasks(dirFilter?);
tasks.forEach(task => {
  // Each task has precomputed changes_count
  console.log(task.changes_count); // e.g., 5 files changed
});

// Detail view
const task = await getTask(taskId);
const changes = await getTaskChanges(taskId, dirFilter?);

// Diff on-demand (when user clicks file)
const diff = await getChangeDiff(taskId, filePath);
// { diff_text: "...", lines_added: 5, lines_removed: 2 }

// Restore single file
const result = await restoreFile(taskId, filePath);
// { files_restored: 1, files_deleted: 0, failures: [] }

// Rollback entire task
const preview = await previewRollback(taskId);
// { task_id, total_changes, files_to_restore, files_to_delete }
const result = await rollbackTask(taskId);
```

## Database Schema Summary

```sql
-- Tasks: one per user prompt or monitoring window
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,            -- "task_123" or "fs_001"
    started_at TEXT NOT NULL,       -- When task began (RFC3339)
    completed_at TEXT,              -- When task finished (null if active)
    status TEXT DEFAULT 'active',   -- active|completed|rolled-back|partial-rolled-back
    prompt TEXT,                    -- Original user request
    tool TEXT,                      -- "claude-code"|"cursor"|"文件监听"
    summary TEXT,                   -- AI-generated summary
    risk_level TEXT                 -- low|medium|high
);

-- File changes: many per task
CREATE TABLE changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,          -- FK: which task this belongs to
    file_path TEXT NOT NULL,        -- Absolute path: "/Users/.../file.rs"
    change_type TEXT NOT NULL,      -- created|modified|deleted|renamed
    old_hash TEXT,                  -- SHA-256 hash before change (in .rew/objects/)
    new_hash TEXT,                  -- SHA-256 hash after change (in .rew/objects/)
    diff_text TEXT,                 -- Unified diff (text files only)
    lines_added INTEGER DEFAULT 0,  -- Lines added
    lines_removed INTEGER DEFAULT 0,-- Lines removed
    restored_at TEXT                -- ISO timestamp when individually restored
);
```

## Task Status Lifecycle

```
[New] 
  ↓ task created with status="active"
[Active] ← User/AI working on files
  ↓ task completes
[Completed] ← Normal completion
  ├─ Can be rolled back → [RolledBack]
  └─ Can be partially restored → [PartialRolledBack]

[RolledBack] ← All changes undone
  └─ Can be re-restored again (no time limit)

[PartialRolledBack] ← Some files restored
  └─ Can restore other files individually
```

## Special Task Type: Monitoring Windows (文件监听)

```
Regular Task (AI tool like claude-code):
┌─ id: "task_123"
├─ prompt: "Add authentication" (user's instruction)
├─ tool: "claude-code"
├─ started_at: 10:00:00
├─ completed_at: 10:15:00 ✓
└─ status: "completed"

Monitoring Window (automatic file surveillance):
┌─ id: "fs_001"
├─ prompt: null
├─ tool: "文件监听" (special marker)
├─ started_at: 10:00:00 (window opened)
├─ completed_at: 10:05:00 ✓ (window closed/sealed)
└─ status: "completed"
  
Display: monitoring window shows as "10:00 – 10:05" (time range)
Hidden: if completed_at IS NULL (still accumulating in background)
```

## Change Types & Icons

```
Change Type | Icon | Color   | Meaning
─────────────────────────────────────────────────
Created     | A    | Green   | New file added
Modified    | M    | Yellow  | Existing file changed
Deleted     | D    | Red     | File removed
Renamed     | R    | Gray    | File moved/renamed
```

## Filtering Capabilities

```
// List all tasks
list_tasks()

// Filter by directory (includes subdirs)
list_tasks("/Users/project/src")
// Returns: All tasks with changes in /Users/project/src/**/*

// Filter by specific file
list_tasks("/Users/project/src/main.rs")
// Returns: All tasks that changed /Users/project/src/main.rs

// In TaskDetail, same filters apply to changes list
```

## Restoration Workflow

```
1. User sees "↩ 读档" (restore) button on file
   
2. Click 1: Show confirmation dialog
   "覆盖当前版本？" (Overwrite current version?)
   
3. Click 2: Execute restore
   - Load old_hash from database
   - Fetch content from .rew/objects/
   - Write to original file path
   - Set restored_at timestamp
   - Add path to suppressed_paths (60s TTL)
   
4. UI shows: "✓ 已读档" (Successfully restored)
   
5. Show hint: "上次读档: 2分钟前" (Last restored: 2 min ago)
```

## Key Design Principles

| Principle | Implementation |
|-----------|-----------------|
| **One task = one unit** | User prompt or monitoring window; all changes grouped |
| **Content-addressed** | SHA-256 hashes; enables dedup & integrity checks |
| **Original state preserved** | `old_hash` never changes; always points to pre-task state |
| **Restorable multiple times** | No limit on restores; `restored_at` tracks each |
| **Safe by default** | Two-step confirmation for destructive operations |
| **Efficient queries** | `changes_count` precomputed; indexed on task_id & file_path |
| **Flexible datetimes** | Parser handles RFC3339, ISO8601, naive formats |
| **Hot-patchable** | New watches hot-added to FSEvents pipeline |

## File Organization

```
rew/
├─ crates/rew-core/src/
│  ├─ types.rs          ← Data models (Task, Change, etc.)
│  └─ db.rs             ← Database schema & CRUD ops
│
├─ gui/src/
│  ├─ components/
│  │  ├─ TaskTimeline.tsx     ← Timeline view (list of tasks)
│  │  ├─ TaskDetail.tsx       ← Detail view (file list + diff)
│  │  └─ DiffViewer.tsx       ← Diff display
│  └─ lib/
│     └─ tauri.ts            ← Invoke helpers & type definitions
│
└─ src-tauri/src/
   ├─ commands.rs           ← IPC command handlers
   ├─ state.rs              ← AppState definition
   └─ main.rs               ← Tauri setup
```

## Common Queries

```sql
-- Get all tasks ordered by recency
SELECT * FROM tasks ORDER BY started_at DESC;

-- Get changes for task_123
SELECT * FROM changes WHERE task_id = 'task_123' ORDER BY id ASC;

-- Get tasks that affected /path/to/dir/**
SELECT DISTINCT t.* FROM tasks t
JOIN changes c ON t.id = c.task_id
WHERE c.file_path LIKE '/path/to/dir/%'
ORDER BY t.started_at DESC;

-- Get tasks that changed specific file
SELECT DISTINCT t.* FROM tasks t
JOIN changes c ON t.id = c.task_id
WHERE c.file_path = '/path/to/file.rs'
ORDER BY t.started_at DESC;

-- Count files changed in task
SELECT COUNT(*) FROM changes WHERE task_id = 'task_123';

-- Get individual file restoration history
SELECT * FROM changes 
WHERE task_id = 'task_123' AND file_path = '/path/to/file.rs'
AND restored_at IS NOT NULL;
```

## Performance Tips

1. **Precomputed counts**: Backend computes `changes_count` before sending TaskInfo
2. **Indexed lookups**: task_id and file_path are indexed
3. **Date queries**: Use `started_at DESC` index
4. **Lazy diffs**: Diff computed on-demand only when file selected
5. **Suppression TTL**: 60s cache prevents re-recording restored files

