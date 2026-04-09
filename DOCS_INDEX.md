# rew Documentation Index

This project contains comprehensive documentation on the rew file safety system and its Claude Code integration.

## Quick Reference

**Start here if you want to:**

- **Use rew:** See `README.md` for quick start
- **Install Claude Code hooks:** See `INSTALL_CLAUDE_CODE_HOOKS.md` (5-minute guide)
- **Understand hook system:** See `CLAUDE_CODE_HOOK_INTEGRATION.md` (detailed technical reference)
- **Understand rew architecture:** See `REW_ARCHITECTURE.md` (full system deep-dive)

## Document Breakdown

### User Guides

| Document | Purpose | Read Time |
|----------|---------|-----------|
| [`README.md`](README.md) | Project overview and features | 5 min |
| [`INSTALL_CLAUDE_CODE_HOOKS.md`](INSTALL_CLAUDE_CODE_HOOKS.md) | Step-by-step hook installation for Claude Code | 10 min |
| [`QUICK_REFERENCE.md`](QUICK_REFERENCE.md) | Quick lookup guide for common tasks | 15 min |

### Technical References

| Document | Purpose | Read Time |
|----------|---------|-----------|
| [`CLAUDE_CODE_HOOK_INTEGRATION.md`](CLAUDE_CODE_HOOK_INTEGRATION.md) | Complete hook system architecture and event reference | 30 min |
| [`REW_ARCHITECTURE.md`](REW_ARCHITECTURE.md) | Full system architecture: data model, database, frontend | 40 min |

## What is rew?

**rew** = AI 时代的文件安全网 ("File safety net for the AI era")

rew automatically:
1. **Monitors** your files in real-time (FSEvents on macOS)
2. **Backs up** file contents before AI tools modify them
3. **Records** what changed (via hooks in Claude Code, Cursor, etc.)
4. **Lets you restore** any file to any point in time with one click

### Key Features

- 🔄 **Real-time file monitoring** — Captures all changes via FSEvents
- 📦 **Content-addressable storage** — SHA-256 hashes + APFS clonefile CoW
- 🎣 **AI tool hooks** — Claude Code, Cursor integration with task tracking
- 📊 **Desktop GUI** — SourceTree-style timeline + diff viewer (Tauri 2 + React)
- 📁 **Scope rules** — `.rewscope` file to control what AI tools can modify
- ⚡ **High performance** — <20ms hook overhead, instant file access

## Architecture Overview

```
┌─────────────────────────────────────────┐
│         rew Project Structure            │
├─────────────────────────────────────────┤
│                                          │
│  crates/                                 │
│  ├── rew-core/          Core library     │
│  │   ├── types.rs       Task/Change      │
│  │   ├── db.rs          SQLite schema    │
│  │   ├── objects.rs     File storage     │
│  │   ├── scope.rs       .rewscope rules  │
│  │   └── [more modules]                 │
│  │                                       │
│  └── rew-cli/           CLI tool         │
│      ├── hook.rs        Hook handlers    │
│      ├── install.rs     Hook injection   │
│      └── [commands]                      │
│                                          │
│  src-tauri/             Tauri backend    │
│  ├── commands.rs        IPC commands     │
│  ├── daemon.rs          FSEvents loop    │
│  └── state.rs           Shared state     │
│                                          │
│  gui/                   React frontend   │
│  ├── components/        UI components    │
│  ├── hooks/             React hooks      │
│  └── lib/               Utilities        │
│                                          │
└─────────────────────────────────────────┘
```

## Data Storage

All data lives in `~/.rew/`:

```
~/.rew/
├── snapshots.db        SQLite database (tasks + changes)
├── objects/            File backup storage (content-addressable)
├── config.toml         Configuration
├── .current_task       Marker file (current task ID)
└── .scan_manifest.json Incremental scan state
```

## Key Concepts

### Task
A "Task" represents one Claude Code or Cursor session. Fields:
- `id` — Unique identifier
- `prompt` — What the user asked
- `tool` — Which tool ("claude-code", "cursor", etc.)
- `started_at` / `completed_at` — Timestamps
- `status` — "Active", "Completed", "RolledBack"
- `summary` — Human-readable change summary

### Change
A "Change" records one file modification within a task. Fields:
- `file_path` — Which file changed
- `change_type` — "Created", "Modified", "Deleted", "Renamed"
- `old_hash` / `new_hash` — SHA-256 content hashes
- `lines_added` / `lines_removed` — Diff statistics
- `restored_at` — When it was restored (if applicable)

### Hook Events

Four events fire during Claude Code operation:

| Event | Trigger | rew Does |
|-------|---------|----------|
| UserPromptSubmit | User submits prompt | Create new Task |
| PreToolUse | Before file operation | Check scope, backup file |
| PostToolUse | After file operation | Record Change in DB |
| Stop | Claude Code finishes | Mark Task as Completed |

## Getting Started

### 1. Build rew

```bash
cd /Users/kuqili/Desktop/project/rew
cargo build -p rew-cli --release      # CLI tool
cargo build -p rew-tauri --release    # Desktop app
```

### 2. Install hooks for Claude Code

```bash
./target/release/rew install
```

This:
- Injects hooks into `~/.claude/settings.json`
- Creates LaunchAgent for daemon auto-start
- Generates `.rewscope` scope rules

### 3. Start daemon

```bash
./target/release/rew daemon
```

Or it will auto-start on next login.

### 4. Open desktop app

```bash
open src-tauri/target/release/rew.app
```

### 5. Use Claude Code

Start a Claude Code session and make file changes. Tasks will appear in rew's "AI 任务" tab.

## File Guide for Developers

### Core Library (`crates/rew-core/src/`)

- `types.rs` — Task, Change, Snapshot data structures (120 lines)
- `db.rs` — SQLite schema + queries (846 lines)
- `objects.rs` — Content-addressable file storage (180 lines)
- `scope.rs` — `.rewscope` rule engine (200 lines)
- `backup/` — APFS clonefile API (150 lines)
- `detector/` — Anomaly detection rules (400 lines)
- `pipeline.rs` — FSEvents → database pipeline (300 lines)
- `restore.rs` — Rollback/restore engine (250 lines)

### CLI Tool (`crates/rew-cli/src/`)

- `commands/hook.rs` — Hook handlers (rew hook prompt/pre-tool/post-tool/stop) (385 lines)
- `commands/install.rs` — Hook injection for Claude Code & Cursor (337 lines)
- `commands/config.rs` — .rewscope rule management
- `commands/status.rs` — rew daemon status
- `commands/list.rs` — List tasks
- `commands/show.rs` — Show task details
- `commands/restore.rs` — Restore files

### Tauri Backend (`src-tauri/src/`)

- `commands.rs` — IPC commands exposed to frontend (1436 lines)
  - `list_tasks` — Get task timeline
  - `get_task` — Get single task
  - `get_task_changes` — Get file changes for task
  - `get_change_diff` — Get diff for one file
  - `rollback_task` — Restore entire task
  - `restore_file` — Restore single file
  - `preview_rollback` — Show what would be restored
- `daemon.rs` — FSEvents loop + anomaly detection
- `state.rs` — AppState shared across threads

### Frontend (`gui/src/`)

- `components/TaskTimeline.tsx` — Timeline view with two-tab interface
- `components/TaskDetail.tsx` — Detailed task view with split-pane diff
- `components/DiffViewer.tsx` — Unified diff display
- `hooks/useTasks.ts` — React hook for task fetching
- `lib/tauri.ts` — Frontend IPC wrapper

## Questions?

See the relevant documentation above or check:
- `crates/rew-core/src/` for data model questions
- `crates/rew-cli/src/commands/hook.rs` for hook implementation
- `src-tauri/src/commands.rs` for IPC contract
- `gui/src/components/` for UI implementation

## Next Steps

1. 📖 Read `INSTALL_CLAUDE_CODE_HOOKS.md` to install hooks
2. 🧪 Test with Claude Code
3. 🔍 Explore data in rew desktop app
4. 📚 Read `REW_ARCHITECTURE.md` for deep dive
5. 🛠️ Contribute improvements!

---

**Last Updated:** 2026-04-09  
**Branch:** feat/dmg-distribution  
**Status:** Functional hook system, ready for user testing

