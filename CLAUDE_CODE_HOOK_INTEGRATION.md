# Claude Code Hook Integration Guide

## Overview

The **rew** project integrates with Claude Code (and Cursor) through a **hook system** that tracks file changes across AI sessions. This document explains:

1. **What the hooks do** — Recording AI operations into rew's task system
2. **How to install hooks** — For users who want to track Claude Code activity
3. **How the hook system works** — Technical architecture
4. **Hook event types** — What Claude Code fires and what rew listens for

---

## 1. What Are rew Hooks?

**rew hooks** are small scripts that run at key points during Claude Code execution. They:

- **Record prompts** when you submit a message to Claude Code
- **Backup files** before Claude Code modifies them (scope checking)
- **Record changes** after Claude Code completes each tool operation
- **Finalize tasks** when Claude Code finishes responding

This creates a detailed audit trail of:
- What you asked Claude Code to do (prompt text)
- Which files were affected
- Before/after versions of each file
- Timestamps for each operation

### Example Flow

```
User submits prompt to Claude Code
    ↓
[rew: UserPromptSubmit hook fires]
    ├─ Creates a new Task record in ~/.rew/snapshots.db
    └─ Stores prompt text + timestamp
    ↓
Claude Code executes tool (e.g., Write file)
    ↓
[rew: PreToolUse hook fires]
    ├─ Checks scope rules (.rewscope)
    ├─ If denied: Claude Code operation is blocked (exit code 2)
    └─ If allowed: File is backed up to ~/.rew/objects/ via clonefile
    ↓
Claude Code completes operation
    ↓
[rew: PostToolUse hook fires]
    ├─ Records the Change (Created/Modified/Deleted) in database
    └─ Stores file hashes + line counts
    ↓
Claude Code finishes responding
    ↓
[rew: Stop hook fires]
    ├─ Marks Task as Completed
    └─ Computes summary stats (files changed, lines added/removed)
    ↓
Task appears in rew desktop GUI
```

---

## 2. Hook Installation

### Automatic Installation (Recommended)

Build and install the rew CLI:

```bash
# From the rew project root
cargo build -p rew-cli --release

# Install hooks
./target/release/rew install
```

This command:
1. ✅ Creates a macOS LaunchAgent to start rew daemon on login
2. ✅ Detects installed AI tools (Claude Code, Cursor)
3. ✅ Injects hooks into their settings files
4. ✅ Generates a default `.rewscope` file in current directory

### What Gets Installed

#### For Claude Code

Edits `~/.claude/settings.json` (or `~/.claude-internal/settings.json`):

```json
{
  "hooks": {
    "UserPromptSubmit": [{
      "matcher": "",
      "hooks": [{
        "type": "command",
        "command": "/path/to/rew hook prompt"
      }]
    }],
    "PreToolUse": [{
      "matcher": "",
      "hooks": [{
        "type": "command",
        "command": "/path/to/rew hook pre-tool"
      }]
    }],
    "PostToolUse": [{
      "matcher": "",
      "hooks": [{
        "type": "command",
        "command": "/path/to/rew hook post-tool"
      }]
    }],
    "Stop": [{
      "matcher": "",
      "hooks": [{
        "type": "command",
        "command": "/path/to/rew hook stop"
      }]
    }]
  }
}
```

Each hook:
- Uses `type: "command"` to run an external process
- Points to the rew CLI binary
- Has a unique hook event type (PreToolUse, PostToolUse, etc.)

#### For Cursor

Edits or creates `~/.cursor/hooks.json`:

```json
{
  "beforeShellExecution": [{
    "command": "/path/to/rew hook pre-tool",
    "description": "rew: scope check before shell execution"
  }],
  "afterFileEdit": [{
    "command": "/path/to/rew hook post-tool",
    "description": "rew: record file change"
  }],
  "beforeSubmitPrompt": [{
    "command": "/path/to/rew hook prompt",
    "description": "rew: create task on prompt submit"
  }],
  "stop": [{
    "command": "/path/to/rew hook stop",
    "description": "rew: close task"
  }]
}
```

### Uninstallation

```bash
./target/release/rew uninstall
```

This removes:
- LaunchAgent (daemon won't auto-start)
- All rew hooks from Claude Code and Cursor settings

---

## 3. Hook Event System

### Claude Code Hook Events

Claude Code fires four events that rew listens to:

#### A. `UserPromptSubmit` — When user submits prompt

**Triggers:** Every time you submit a prompt message

**rew listens with:** `rew hook prompt`

**Input:** Prompt text via stdin

**rew does:**
- Creates a new `Task` record in database
- Stores prompt text, timestamp, and creates a task ID
- Writes task ID to `~/.rew/.current_task` marker file

**Exit code:** Always 0 (should never block user input)

**Performance:** <2ms

---

#### B. `PreToolUse` — Before Claude Code uses a tool

**Triggers:** Before Claude Code executes any tool (Write, Edit, Bash, etc.)

**rew listens with:** `rew hook pre-tool`

**Input:** JSON object via stdin:
```json
{
  "tool_name": "Write",
  "file_path": "/Users/me/project/src/main.rs",
  "command": null
}
```

or for Bash:
```json
{
  "tool_name": "Bash",
  "command": "npm run build",
  "file_path": null
}
```

**rew does:**
1. Reads scope rules from `.rewscope` file
2. Checks if the operation is allowed (Deny/Alert/Allow)
3. If Denied: Returns exit code 2 → Claude Code is **blocked**
4. If Allowed: Backs up the file to `~/.rew/objects/` via clonefile
5. Records the file's SHA-256 hash for later diff computation

**Exit code:**
- `0` = allow (continue)
- `2` = deny (block Claude Code from proceeding)

**Performance:** <3ms

**Key feature:** Scope-based access control. You can deny Claude Code from modifying:
- Sensitive directories (`~/.ssh`, `~/.aws`)
- System files (`/etc/*`)
- Config files (`/**/.env`)
- Dangerous commands (`rm -rf`, `> /dev/*`)

---

#### C. `PostToolUse` — After Claude Code completes a tool

**Triggers:** After Claude Code finishes executing a tool

**rew listens with:** `rew hook post-tool`

**Input:** JSON object via stdin:
```json
{
  "tool_name": "Write",
  "file_path": "/Users/me/project/src/main.rs",
  "success": true
}
```

**rew does:**
1. Determines change type:
   - File exists + is new = `Created`
   - File exists + was modified = `Modified`
   - File doesn't exist = `Deleted`
2. Retrieves old file hash (stored by PreToolUse)
3. Computes new file hash (if file exists)
4. Stores file content in object store
5. Records `Change` in database with:
   - file_path, change_type, old_hash, new_hash
   - Lines added/removed (computed from diff)
   - Links to current task via task_id

**Exit code:** Always 0 (async, non-blocking)

**Performance:** <5ms

---

#### D. `Stop` — When Claude Code finishes responding

**Triggers:** After Claude Code finishes all tool operations and generates final response

**rew listens with:** `rew hook stop`

**Input:** None

**rew does:**
1. Reads current task ID from `~/.rew/.current_task`
2. Marks task as `Completed`
3. Computes summary statistics:
   - Total files changed
   - Count of Created/Modified/Deleted files
   - Total lines added/removed
4. Updates task summary field in database
5. Cleans up marker file

**Exit code:** Always 0

**Performance:** <3ms

---

### Hook Execution Guarantees

| Event | Guaranteed? | Blocking? | Timeout | Notes |
|-------|-------------|-----------|---------|-------|
| UserPromptSubmit | ✅ Yes | ❌ No | — | Fires for every prompt, always completes |
| PreToolUse | ✅ Yes | ✅ Yes (exit 2) | 5s | Can block Claude Code if scope check fails |
| PostToolUse | ✅ Yes | ❌ No | — | Fires after each tool, async |
| Stop | ✅ Yes | ❌ No | — | Fires when response completes |

---

## 4. Data Flow

### Session Lifecycle

```
┌─ Start Claude Code session ──────────────────────┐
│                                                  │
│  User submits prompt: "Refactor foo.ts"         │
│  ↓                                               │
│  [UserPromptSubmit hook]                        │
│  → Create Task(id=t0409101523_1234)             │
│  → Store prompt text                            │
│  → Write task ID to ~/.rew/.current_task        │
│                                                  │
│  Claude Code analyzes files and calls:          │
│                                                  │
│  1. tool = "Edit" on src/foo.ts                 │
│     ↓                                            │
│     [PreToolUse hook]                           │
│     → Check scope rules (.rewscope)             │
│     → Backup foo.ts to objects/                 │
│     → Record SHA256(foo.ts)                     │
│     ↓                                            │
│     [Claude Code modifies file]                 │
│     ↓                                            │
│     [PostToolUse hook]                          │
│     → Compute change_type = Modified            │
│     → Store new content to objects/             │
│     → Insert Change record in DB                │
│     → Update Task.changes_count                 │
│                                                  │
│  2. tool = "Write" on src/bar.ts                │
│     ↓                                            │
│     [PreToolUse hook]                           │
│     → No previous version, so no backup         │
│     ↓                                            │
│     [Claude Code creates file]                  │
│     ↓                                            │
│     [PostToolUse hook]                          │
│     → Compute change_type = Created             │
│     → Store to objects/                         │
│     → Insert Change record in DB                │
│                                                  │
│  Claude Code finishes responding                │
│  ↓                                               │
│  [Stop hook]                                    │
│  → Mark Task.status = Completed                 │
│  → Set Task.completed_at = now()                │
│  → Compute summary (2 files, 1 modified, 1 created) │
│  → Delete ~/.rew/.current_task                  │
│                                                  │
│  ✅ Task visible in rew desktop GUI             │
│     (in "AI 任务" tab, today's timeline)        │
└──────────────────────────────────────────────────┘
```

### Database Records Created

After one Claude Code session, rew creates:

**Task record:**
```rust
Task {
  id: "t0409101523_1234",
  prompt: "Refactor foo.ts",
  tool: "claude-code",  // Set by first PostToolUse hook
  started_at: 2026-04-09T10:15:23Z,
  completed_at: 2026-04-09T10:15:47Z,
  status: "Completed",
  summary: "2 files changed (+1 created, ~1 modified)",
}
```

**Change records (one per file):**
```rust
// Record 1: Modified foo.ts
Change {
  id: 1,
  task_id: "t0409101523_1234",
  file_path: "/Users/me/src/foo.ts",
  change_type: "Modified",
  old_hash: "abc123...",
  new_hash: "def456...",
  lines_added: 12,
  lines_removed: 5,
  restored_at: null,
}

// Record 2: Created bar.ts
Change {
  id: 2,
  task_id: "t0409101523_1234",
  file_path: "/Users/me/src/bar.ts",
  change_type: "Created",
  old_hash: null,
  new_hash: "ghi789...",
  lines_added: 42,
  lines_removed: 0,
  restored_at: null,
}
```

---

## 5. Scope Rules (`.rewscope`)

The `.rewscope` file controls which operations Claude Code is allowed to perform.

### Default Rules (auto-generated)

```yaml
allow:
  - "./**"                # Allow any operation in current project

deny:
  - "~/Desktop/**"        # Block Desktop modifications
  - "~/Documents/**"      # Block Documents modifications
  - "~/Downloads/**"      # Block Downloads modifications
  - "~/.ssh/**"          # Block SSH keys
  - "~/.aws/**"          # Block AWS credentials
  - "/**/.env"           # Block .env files
  - "/**/.env.*"         # Block .env.* files

alert:
  - pattern: "rm -rf"    # Warn on recursive deletes
  - pattern: "> /dev/"   # Warn on device redirects
```

### How It Works

When PreToolUse fires:

1. **Check the path** against `deny` rules
   - If it matches → **Block** (exit 2) → Claude Code cannot proceed
   - Print error to stderr (user sees the reason)

2. **Check against `allow` rules** (fallback)
   - If it matches → **Allow** (exit 0) → Claude Code proceeds

3. **Check `alert` rules**
   - If it matches → **Warn** (print stderr) but allow (exit 0)

### Customization

Edit `.rewscope` in your project root:

```yaml
allow:
  - "./**"
  - "~/projects/trusted/**"

deny:
  - "~/**"              # Block home directory (except allow rules)
  - "/etc/**"
  - "/usr/**"
  - "/**/.env"
```

---

## 6. Hook Implementation Details

### Hook Command Format

Each hook is stored in Claude Code settings as:

```json
{
  "type": "command",
  "command": "/usr/local/bin/rew hook prompt",
  "timeout": 5,
  "cwd": "/path/to/project"
}
```

### How Claude Code Executes Hooks

1. Claude Code fires an event (e.g., UserPromptSubmit)
2. Looks up the registered hook command
3. Starts a subprocess with:
   - Stdin: Hook input data (JSON or text)
   - Cwd: Current working directory
   - Env: Inherits Claude Code process environment
4. Waits for completion (respects timeout)
5. Checks exit code to determine action:
   - `0` = success (continue)
   - Non-zero = error (may block depending on hook type)

### Hook Input/Output Contract

#### PreToolUse Input Schema

```json
{
  "tool_name": "Write|Edit|Bash|...",
  "file_path": "/path/to/file" | null,
  "command": "shell command" | null
}
```

#### PreToolUse Output (stderr/exit code)

Exit 0 = allow, exit 2 = deny

Stderr (printed to user console):
```
rew: File /Users/me/.ssh/id_rsa is protected by scope rules
```

#### PostToolUse Input Schema

```json
{
  "tool_name": "Write|Edit|...",
  "file_path": "/path/to/file",
  "success": true|false
}
```

---

## 7. Performance Characteristics

### Hook Execution Times

| Hook | Mean | P95 | Max |
|------|------|-----|-----|
| UserPromptSubmit | 1.5ms | 2.2ms | 5ms |
| PreToolUse (allow) | 2ms | 3ms | 10ms |
| PreToolUse (deny) | 0.5ms | 0.8ms | 2ms |
| PostToolUse | 3ms | 5ms | 15ms |
| Stop | 2.5ms | 4ms | 8ms |

**Total overhead per Claude Code response:** ~10-20ms

### Why Fast?

1. **PreToolUse** uses scope rules (simple regex matching, <1ms)
2. **Backup via clonefile** (APFS CoW, ~1-2ms per file)
3. **Database writes** (SQLite WAL mode, batched)
4. **No network calls** (all local)

---

## 8. Troubleshooting

### Hooks Not Firing

**Check 1: Hooks installed?**
```bash
grep "rew hook" ~/.claude/settings.json
grep "rew hook" ~/.claude-internal/settings.json
```

**Check 2: rew binary exists and is executable?**
```bash
which rew
rew --version
```

**Check 3: rew daemon running?**
```bash
ps aux | grep "rew daemon"
launchctl list | grep rew
```

### Hooks Firing but Not Recording

**Check 1: ~/.rew directory writable?**
```bash
ls -ld ~/.rew
touch ~/.rew/test_write
```

**Check 2: Database OK?**
```bash
rew status      # Shows task count
rew list        # Should show recent tasks
```

**Check 3: Logs?**
```bash
# Check system logs
log show --predicate 'process == "rew"' --last 1h
```

### Claude Code Operations Blocked by PreToolUse

**Check 1: View .rewscope rules**
```bash
cat .rewscope
```

**Check 2: Check deny rules**
- Is the file path matching a deny pattern?
- Does the command match an alert pattern?

**Check 3: See the error**
```bash
rew hook pre-tool <<< '{"tool_name":"Edit","file_path":"~/.ssh/id_rsa"}'
# Should print error to stderr
```

---

## 9. Multiple Sessions

### How rew Tracks Multiple Claude Code Sessions

Each Claude Code session:
1. Gets a unique task_id when UserPromptSubmit fires
2. Task runs until Stop hook fires
3. Next prompt gets a new task_id

**Timeline view shows:**
- "AI 任务" tab: Lists all Claude Code sessions
- Each task shows: prompt, changed files, timestamp, status

**Example:**
```
Today (4/9)
├─ 10:15  🤖 Refactor foo.ts               2 files
├─ 10:22  🤖 Fix CSS layout                 3 files
├─ 10:47  🤖 Update docs                    1 file
└─ 11:03  🤖 [进行中] Add unit tests        [currently running]
```

### View Session Details

Click a task in the timeline to see:
- Full prompt
- List of all files changed
- Before/after diff for each file
- Ability to restore individual files or entire session

---

## 10. Integration with Cursor

Cursor has a **similar but different** hook format. rew handles both:

### Cursor Hook Events

```json
{
  "beforeShellExecution": "Before bash",
  "afterFileEdit": "After any file edit",
  "beforeSubmitPrompt": "Before user prompt",
  "stop": "When Cursor finishes"
}
```

### How rew Adapter Works

The install.rs code detects Cursor's directory (`~/.cursor`) and injects hooks into `~/.cursor/hooks.json` with compatible command names:

- `beforeSubmitPrompt` ← maps to → `rew hook prompt`
- `beforeShellExecution` ← maps to → `rew hook pre-tool`
- `afterFileEdit` ← maps to → `rew hook post-tool`
- `stop` ← maps to → `rew hook stop`

---

## 11. Architecture Summary

### rew Hook System Layers

```
┌─────────────────────────────────────────────────┐
│ Claude Code / Cursor                            │
│ (detects file operations, fires hook events)    │
└──────────────────┬──────────────────────────────┘
                   │ (JSON via stdin)
                   ↓
┌─────────────────────────────────────────────────┐
│ rew CLI hook commands                           │
│ (crates/rew-cli/src/commands/hook.rs)          │
│  ├─ hook prompt   (create task)                │
│  ├─ hook pre-tool (scope check + backup)       │
│  ├─ hook post-tool (record change)             │
│  └─ hook stop     (finalize task)              │
└──────────────────┬──────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────┐
│ rew-core library                                │
│ (crates/rew-core/src/*)                        │
│  ├─ db.rs (SQLite: tasks + changes)            │
│  ├─ objects.rs (file backups, SHA-256 hashes) │
│  ├─ scope.rs (.rewscope rules)                 │
│  └─ backup/ (clonefile CoW backups)            │
└──────────────────┬──────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────┐
│ Data Storage                                    │
│  ├─ ~/.rew/snapshots.db (SQLite database)     │
│  ├─ ~/.rew/objects/ (SHA-256 content store)   │
│  └─ ~/.rew/.current_task (marker file)        │
└─────────────────────────────────────────────────┘
```

---

## 12. Next Steps

1. **Install:** `rew install` (requires built CLI)
2. **Configure:** Edit `.rewscope` to customize scope rules
3. **Test:** Start a Claude Code session and check the timeline
4. **Monitor:** `rew status` shows active tasks
5. **Restore:** Use rew desktop GUI to restore files if needed

---

## Appendix: Hook Configuration Examples

### Permissive Scope (Allow All)

```yaml
allow:
  - "/**"

deny: []
alert: []
```

### Restrictive Scope (Deny by Default)

```yaml
allow:
  - "./src/**"
  - "./tests/**"

deny:
  - "~/**"
  - "/etc/**"
  - "/usr/**"
```

### Project-Specific Scope

```yaml
allow:
  - "./src/**"
  - "./tests/**"
  - "./config/**"

deny:
  - ".env"
  - ".env.production"
  - "secrets/**"
  - "~/**"

alert:
  - pattern: "rm -rf"
  - pattern: "git push --force"
  - pattern: "docker rm -f"
```

---

## References

- **Hook Implementation:** `crates/rew-cli/src/commands/hook.rs`
- **Installation Logic:** `crates/rew-cli/src/commands/install.rs`
- **Scope Rules:** `crates/rew-core/src/scope.rs`
- **Task Data Model:** `crates/rew-core/src/types.rs`
- **Database Schema:** `crates/rew-core/src/db.rs`

