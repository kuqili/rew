# rew Hook Architecture

This document explains how rew's hook system works and how to extend it to support additional AI tools beyond Claude Code and Cursor.

## Overview

The rew hook system is a **four-phase workflow** that captures AI tool operations:

```
┌─────────────────────┐
│   Claude Code       │
│   (AI Tool)         │
└──────────┬──────────┘
           │
           ├─→ UserPromptSubmit Hook
           │   └─→ rew hook prompt
           │       └─→ Create Task in DB
           │
           ├─→ PreToolUse Hook (before Write/Edit)
           │   └─→ rew hook pre-tool
           │       ├─→ Scope check (.rewscope rules)
           │       ├─→ Backup existing file to objects/
           │       └─→ (exit 0=allow, 2=deny)
           │
           ├─→ PostToolUse Hook (after Write/Edit)
           │   └─→ rew hook post-tool
           │       ├─→ Record Change (created/modified/deleted)
           │       ├─→ Store file in objects/
           │       └─→ Update database
           │
           └─→ Stop Hook (when AI finishes)
               └─→ rew hook stop
                   ├─→ Mark task as completed
                   ├─→ Compute summary stats
                   └─→ Clean up temporary markers
```

## Core Components

### 1. Hook Injectors (Installation)

Located in `crates/rew-cli/src/commands/install.rs`

Each AI tool has:
- **Detector**: Checks if tool is installed (e.g., `~/.claude-internal/settings.json` exists)
- **Injector**: Adds hook definitions to tool's config file
- **Remover**: Removes hooks during uninstall

Current implementations:
- **Claude Code**: Uses `~/.claude/settings.json` or `~/.claude-internal/settings.json`
  - Hook format: `{ type: "command", command: "..." }`
  - Events: `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`
  
- **Cursor**: Uses `~/.cursor/hooks.json`
  - Hook format: `{ command: "...", description: "..." }`
  - Events: `beforeShellExecution`, `afterFileEdit`, `beforeSubmitPrompt`, `stop`

### 2. Hook Handlers (Execution)

Located in `crates/rew-cli/src/commands/hook.rs`

Each hook reads from stdin and performs an action:

| Hook | Input | Output | Purpose |
|------|-------|--------|---------|
| `rew hook prompt` | Plain text (prompt) | (none) | Create new Task record |
| `rew hook pre-tool` | JSON (tool_name, file_path, command) | Exit code (0/2) | Scope check + backup |
| `rew hook post-tool` | JSON (tool_name, file_path, success) | (none) | Record Change |
| `rew hook stop` | (none) | (none) | Close Task, compute summary |

### 3. Scope Engine

Located in `crates/rew-core/src/scope.rs`

Reads `.rewscope` rules and enforces access control:

```yaml
allow:
  - "./**"              # Allow all files under current dir
  - "~/projects/**"     # Allow all under ~/projects

deny:
  - "~/.ssh/**"         # Block SSH keys
  - "/**/.env"          # Block .env files
  - "/**/.env.*"        # Block .env.* files

alert:
  - pattern: "rm -rf"   # Warn on dangerous commands
    action: block
```

Exit codes from `rew hook pre-tool`:
- `0` = Operation allowed
- `2` = Operation denied (AI tool must stop)

### 4. Backup System

Located in `crates/rew-core/src/objects.rs` and `crates/rew-core/src/backup/`

Uses **content-addressed storage** with SHA-256:
- File content backed up to `~/.rew/objects/<sha256-hash>`
- Original file path and change metadata stored in database
- Uses `clonefile` on macOS for CoW (copy-on-write) - shared disk blocks

## Database Integration

### Task Record

Created by `rew hook prompt`:
```rust
struct Task {
    id: String,              // "t04091400_1234"
    prompt: Option<String>,  // User's prompt
    tool: Option<String>,    // "Claude Code", "Cursor", etc.
    started_at: DateTime,
    completed_at: Option<DateTime>,
    status: TaskStatus,      // Active → Completed
    risk_level: Option<RiskLevel>,
    summary: Option<String>, // "3 files changed (+1 created, ~2 modified)"
}
```

### Change Record

Created by `rew hook post-tool`:
```rust
struct Change {
    id: i64,
    task_id: String,
    file_path: PathBuf,
    change_type: ChangeType,  // Created, Modified, Deleted, Renamed
    old_hash: Option<String>,  // SHA-256 before change
    new_hash: Option<String>,  // SHA-256 after change
    diff_text: Option<String>, // Computed on-demand
    lines_added: u32,
    lines_removed: u32,
    restored_at: Option<DateTime>, // When this file was restored
}
```

## Desktop App Integration

The rew desktop app (Tauri + React) queries the database and displays tasks:

### Frontend Flow

```typescript
// gui/src/hooks/useTasks.ts
const { tasks, loading, error } = useTasks(dirFilter);

// Calls backend: listTasks(dirFilter)
// Backend returns: TaskInfo[] (with precomputed changes_count)

// Timeline displays tasks:
// - Filters by date (today, yesterday, 24h, 7d, custom)
// - Filters by view mode ("scheduled" = monitoring windows, "ai" = AI tasks)
// - Shows tool badge, prompt, file count, timestamp
// - Allows rollback/restore operations
```

### Tauri IPC Commands

Key commands in `src-tauri/src/commands.rs`:

| Command | Input | Output | Purpose |
|---------|-------|--------|---------|
| `list_tasks` | dirFilter? | Vec<TaskInfo> | List all tasks |
| `get_task` | taskId | TaskInfo | Get single task |
| `get_task_changes` | taskId, dirFilter? | Vec<ChangeInfo> | Get changes for task |
| `rollback_task_cmd` | taskId | UndoResultInfo | Restore all files in task |
| `restore_file_cmd` | taskId, filePath | UndoResultInfo | Restore single file |

## Hook Data Flow (Detailed)

When you submit a prompt "Create test.md" in Claude Code:

### Step 1: UserPromptSubmit Hook

**Time**: Prompt submitted, before AI starts
```
Claude Code → triggers UserPromptSubmit hook
            ↓
hook command: /path/to/rew hook prompt
stdin: "Create test.md"
            ↓
rew hook prompt:
  1. Read stdin text
  2. Generate task_id = "t04091400_5678"
  3. Create Task record:
     {
       id: "t04091400_5678",
       prompt: "Create test.md",
       tool: null (set later by post-tool),
       started_at: 2026-04-09T14:00:...
       status: Active
     }
  4. Write to ~/.rew/snapshots.db
  5. Write task_id to ~/.rew/.current_task
  6. Exit 0 (always succeed, don't block AI)
```

### Step 2: PreToolUse Hook

**Time**: Right before AI writes file
```
Claude Code → triggers PreToolUse hook (about to write test.md)
            ↓
hook command: /path/to/rew hook pre-tool
stdin: {
  "tool_name": "Write",
  "file_path": "./test.md",
  "command": null
}
            ↓
rew hook pre-tool:
  1. Parse JSON input
  2. Load ScopeEngine from .rewscope
  3. Check path: ./test.md
     - Check allow rules → match "./**" → ALLOW
     - Check deny rules → no match → OK
     - Check alert rules → no match → OK
  4. File exists? No (new file)
  5. Return exit 0 → Claude Code continues
```

### Step 3: PostToolUse Hook

**Time**: After AI writes file
```
Claude Code → file written to test.md
            ↓
trigger PostToolUse hook
            ↓
hook command: /path/to/rew hook post-tool
stdin: {
  "tool_name": "Write",
  "file_path": "./test.md",
  "success": true
}
            ↓
rew hook post-tool:
  1. Parse JSON input
  2. Read current task_id from ~/.rew/.current_task → "t04091400_5678"
  3. Determine change type:
     - File exists? Yes
     - Previous change for file exists? No
     → change_type = Created
  4. Compute hashes:
     - old_hash = null (didn't exist before)
     - new_hash = sha256(test.md) = "a1b2c3d4..."
  5. Backup to objects:
     - Store test.md to ~/.rew/objects/a1b2c3d4
  6. Create Change record:
     {
       task_id: "t04091400_5678",
       file_path: "./test.md",
       change_type: Created,
       old_hash: null,
       new_hash: "a1b2c3d4...",
       lines_added: 1
     }
  7. Insert into database
  8. Exit 0 (async, doesn't block)
```

### Step 4: Stop Hook

**Time**: Claude Code finishes responding
```
Claude Code → finishes response
            ↓
trigger Stop hook
            ↓
hook command: /path/to/rew hook stop
stdin: (empty)
            ↓
rew hook stop:
  1. Read task_id from ~/.rew/.current_task → "t04091400_5678"
  2. Get changes for task:
     - 1 Created file
  3. Update task:
     {
       status: Completed,
       completed_at: 2026-04-09T14:01:...,
       summary: "1 files changed (+1 created, ~0 modified, -0 deleted)"
     }
  4. Clean up marker: rm ~/.rew/.current_task
  5. Exit 0
```

### Step 5: Desktop App Display

```
React component queries: listTasks()
                    ↓
Tauri invokes: list_tasks(dirFilter=null)
                    ↓
Rust backend:
  - Query tasks table: SELECT * FROM tasks ORDER BY started_at DESC
  - For each task: SELECT COUNT(*) FROM changes WHERE task_id = ?
  - Return TaskInfo with precomputed changes_count
                    ↓
Frontend receives: [
  {
    id: "t04091400_5678",
    prompt: "Create test.md",
    tool: null,
    started_at: "2026-04-09T14:00:00Z",
    completed_at: "2026-04-09T14:01:00Z",
    status: "completed",
    summary: "1 files changed (+1 created, ~0 modified, -0 deleted)",
    changes_count: 1
  }
]
                    ↓
Timeline renders:
  ┌─────────────────────────────────────────┐
  │ ● Create test.md         1 文件    14:00 │
  └─────────────────────────────────────────┘
```

## Adding Support for New AI Tools

To add support for a new AI tool (e.g., GitHub Copilot, Windsurf):

### Step 1: Add Detector

In `install.rs`, add a function to detect the tool:

```rust
fn inject_copilot_hooks(rew_bin: &str) -> Option<Result<(), String>> {
    let home = dirs::home_dir()?;
    let copilot_config = home.join(".config").join("github-copilot").join("settings.json");
    
    if copilot_config.exists() {
        return Some(inject_copilot_hooks_to(&copilot_config, rew_bin));
    }
    None
}
```

### Step 2: Add Injector

```rust
fn inject_copilot_hooks_to(config_path: &PathBuf, rew_bin: &str) -> Result<(), String> {
    let hooks = serde_json::json!({
        "hooks": {
            "sessionStart": [{
                "command": format!("{} hook prompt", rew_bin)
            }],
            "preToolUse": [{
                "command": format!("{} hook pre-tool", rew_bin)
            }],
            "postToolUse": [{
                "command": format!("{} hook post-tool", rew_bin)
            }],
            "agentStop": [{
                "command": format!("{} hook stop", rew_bin)
            }]
        }
    });
    
    let content = serde_json::to_string_pretty(&hooks)?;
    std::fs::write(config_path, content)?;
    Ok(())
}
```

### Step 3: Wire Into install()

```rust
pub fn install() -> RewResult<()> {
    // ... existing code ...
    
    // Copilot
    if let Some(result) = inject_copilot_hooks(&rew_bin) {
        match result {
            Ok(()) => println!("{} Copilot hook 已注入", display::success_prefix()),
            Err(e) => println!("  ⚠ Copilot hook 注入失败: {}", e),
        }
    }
    
    // ... rest of code ...
}
```

### Step 4: Add Hook Handler (if needed)

If the new tool uses a different JSON format, extend `hook.rs`:

```rust
pub struct CopilotPreToolInput {
    pub action: String,  // Different field names
    pub target_path: String,
}

pub fn handle_copilot_pre_tool() -> RewResult<i32> {
    let input_str = read_stdin_text();
    let input: CopilotPreToolInput = serde_json::from_str(&input_str)?;
    
    // ... scope check logic ...
    
    Ok(0) // or 2 for deny
}
```

## Hook Standards (Proposed)

For consistency across AI tools, we should follow these standards:

### Hook Events (Unified)

All AI tools should fire these events:
1. **UserPromptSubmit** - When user submits a prompt
2. **PreToolUse** - Before tool modifies files (can deny)
3. **PostToolUse** - After tool completes operation
4. **Stop** - When AI finishes responding

### Hook Input Format (Unified JSON)

```json
{
  "event": "PreToolUse",
  "tool_name": "Write",
  "session_id": "sess_1234",
  "file_path": "/absolute/path/to/file",
  "command": null,
  "timestamp": "2026-04-09T14:00:00Z"
}
```

### Hook Exit Codes

```
0 = Allow (operation proceeds)
1 = Error (tool may retry)
2 = Deny (operation blocked)
```

### Environment Variables

Each hook should expose:
```
REW_HOOK_NAME = "prompt" | "pre-tool" | "post-tool" | "stop"
REW_SESSION_ID = current session ID
REW_TOOL_NAME = name of AI tool
```

## Performance Characteristics

Hook performance is critical since they're called frequently:

| Operation | Time | Bottleneck |
|-----------|------|-----------|
| `rew hook prompt` | 2ms | Database write |
| `rew hook pre-tool` (new file) | 3ms | Scope check |
| `rew hook pre-tool` (existing file) | 5ms | File backup to objects |
| `rew hook post-tool` | 1ms | Async queue |
| `rew hook stop` | 2ms | Summary computation |

**Optimization strategies**:
- Pre-tool backs up file to objects async (doesn't wait for completion)
- Post-tool uses async database queue (fire and forget)
- Scope check uses regex cache for hot patterns
- Database uses WAL mode for concurrent reads

## Troubleshooting Hooks

### Diagnosis

```bash
# Check if hooks are registered
cat ~/.claude-internal/settings.json | jq '.hooks'

# Test a hook manually
echo "test prompt" | rew hook prompt
echo '{"tool_name":"Write","file_path":"./test.txt"}' | rew hook pre-tool

# View database directly
sqlite3 ~/.rew/snapshots.db "SELECT id, prompt, status FROM tasks ORDER BY started_at DESC LIMIT 5;"
```

### Common Issues

1. **Hooks not called**: Check Claude Code settings file location
2. **Exit code 2 (denied)**: Review `.rewscope` rules
3. **Tasks not in database**: Check database file exists and isn't locked
4. **Desktop app shows no tasks**: Ensure rew daemon is running

## Future Enhancements

Potential improvements to hook system:

1. **Hook chaining** - Multiple hooks per event, configurable order
2. **Hook templating** - Use variables in hook paths (e.g., `$REW_BIN`)
3. **Async hooks** - Support long-running validators (webhooks, remote servers)
4. **Hook versioning** - Support multiple hook API versions simultaneously
5. **Hook marketplace** - Share custom hooks for specific use cases
6. **Hook testing framework** - CLI tools to test hook configurations
7. **Conditional hooks** - Only run hooks for matching file patterns
8. **Hook metrics** - Track performance and errors per hook
