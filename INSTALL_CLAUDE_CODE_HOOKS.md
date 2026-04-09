# Installing rew Hooks for Claude Code

This guide walks you through installing rew hooks so Claude Code can track all file changes in the rew desktop application.

## Prerequisites

- macOS 11.0+ with APFS filesystem
- Claude Code installed
- rew project cloned or built
- Basic terminal knowledge

---

## Quick Start (5 minutes)

### Step 1: Build the rew CLI

```bash
cd /Users/kuqili/Desktop/project/rew
cargo build -p rew-cli --release
```

**Output:** `./target/release/rew` (the CLI binary)

### Step 2: Install Hooks

```bash
./target/release/rew install
```

**What this does:**
- ✅ Injects hooks into Claude Code settings (`~/.claude/settings.json`)
- ✅ Injects hooks into Cursor settings if installed (`~/.cursor/hooks.json`)
- ✅ Creates macOS LaunchAgent for rew daemon
- ✅ Generates `.rewscope` in current directory

**Expected output:**
```
ℹ 正在安装 LaunchAgent...
✓ LaunchAgent 已安装

ℹ 正在检测 AI 工具并注入 hook...
✓ Claude Code hook 已注入
✓ Cursor hook 已注入

✓ 已生成 .rewscope 规则文件

  使用 rew daemon 立即启动守护进程
```

### Step 3: Start rew Daemon

The daemon can be started manually or will auto-start on next login:

```bash
# Manual start
./target/release/rew daemon

# Or check if it's running
ps aux | grep "rew daemon"

# Check status
./target/release/rew status
```

### Step 4: Verify Installation

Check that hooks were installed in Claude Code settings:

```bash
# Look for Claude Code settings
cat ~/.claude/settings.json | grep -i "hook"

# or if using ~/.claude-internal
cat ~/.claude-internal/settings.json | grep -i "hook"
```

Should see:
```json
{
  "hooks": {
    "PreToolUse": [...],
    "PostToolUse": [...],
    "UserPromptSubmit": [...],
    "Stop": [...]
  }
}
```

### Step 5: Test with Claude Code

1. Open Claude Code
2. Start a new conversation
3. Ask Claude to make a file change (e.g., "Create a test.txt file")
4. Open rew desktop app
5. Check the "AI 任务" tab → should see your session!

---

## Detailed Installation Steps

### Understanding What Gets Installed

#### 1. Claude Code Settings File

**Location:** `~/.claude/settings.json` or `~/.claude-internal/settings.json`

**What rew adds:**
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
- Runs when Claude Code fires that event
- Passes JSON data via stdin to the rew CLI
- Receives exit code (0 = success, 2 = deny for scope checks)

#### 2. macOS LaunchAgent

**Location:** `~/Library/LaunchAgents/com.rew.daemon.plist`

**Purpose:** Starts `rew daemon` automatically on login

**To check if running:**
```bash
launchctl list | grep rew
```

**To manually control:**
```bash
# Start now
launchctl start com.rew.daemon

# Stop
launchctl stop com.rew.daemon

# View logs
log show --predicate 'process == "rew"' --last 1h
```

#### 3. Scope Rules File

**Location:** `.rewscope` in current directory

**Default rules:**
```yaml
allow:
  - "./**"

deny:
  - "~/Desktop/**"
  - "~/Documents/**"
  - "~/Downloads/**"
  - "~/.ssh/**"
  - "~/.aws/**"
  - "/**/.env"
  - "/**/.env.*"

alert:
  - pattern: "rm -rf"
  - pattern: "> /dev/"
```

**To customize:** Edit `.rewscope` in your project directory

---

## Installation Verification Checklist

- [ ] `./target/release/rew` binary exists and is executable
- [ ] `rew --version` works
- [ ] Claude Code settings file contains rew hooks
- [ ] LaunchAgent is installed: `launchctl list | grep rew`
- [ ] Daemon is running: `ps aux | grep "rew daemon"`
- [ ] `~/.rew/snapshots.db` exists (created on first run)
- [ ] `.rewscope` file exists in project directory

### Troubleshooting the Verification

**Q: `rew --version` not found**

```bash
# Make sure binary is built
cargo build -p rew-cli --release

# Use full path
/Users/kuqili/Desktop/project/rew/target/release/rew --version
```

**Q: Hooks not in Claude Code settings**

```bash
# Check if install was successful
./target/release/rew install

# Check settings file
cat ~/.claude/settings.json | jq '.hooks'

# If empty, try the other location
cat ~/.claude-internal/settings.json | jq '.hooks'
```

**Q: LaunchAgent shows as not installed**

```bash
# Install it manually
launchctl load ~/Library/LaunchAgents/com.rew.daemon.plist

# Check it's there
launchctl list | grep rew
```

**Q: Daemon not running**

```bash
# Start it manually
./target/release/rew daemon &

# Or start via LaunchAgent
launchctl start com.rew.daemon

# Check process
ps aux | grep "rew daemon"
```

**Q: Database not initialized**

```bash
# Check if it exists
ls -la ~/.rew/snapshots.db

# If not, daemon will create it on first run
# or run a query to initialize:
./target/release/rew list
```

---

## Multi-Machine Setup

If you want to track Claude Code on multiple machines:

### On Each Machine

1. Clone/build rew project
2. Run `rew install`
3. Verify daemon is running

Each machine maintains its own:
- `~/.rew/snapshots.db` (local SQLite database)
- `~/.rew/objects/` (local file backups)
- LaunchAgent (auto-starts on login)

The rew desktop app shows tasks from the **current machine only**.

---

## Integration with Existing Workflows

### In VS Code

If you use VS Code with Remote-SSH or Dev Containers:
- Install rew on the **local machine** (macOS)
- Hooks apply to local Claude Code sessions
- Remote files are handled via rew's file monitoring

### In Git Projects

rew creates a `.rewscope` file in your project root.

**Best practice:** Commit this to your repository

```bash
# Example .rewscope for a web project
allow:
  - "./src/**"
  - "./public/**"
  - "./config/**"
  - "./tests/**"

deny:
  - "~/**"           # Block home directory
  - ".env"
  - ".env.production"
  - "secrets/**"
  - "**/.aws/**"

alert:
  - pattern: "rm -rf"
  - pattern: "git push --force"
```

Team members can then share the same scope rules.

---

## What Happens After Installation

### First Claude Code Session with Hooks

```
1. You submit a prompt in Claude Code
   → UserPromptSubmit hook fires
   → rew creates a Task record

2. Claude Code decides to modify file.ts
   → PreToolUse hook fires
   → rew checks .rewscope rules
   → rew backs up file.ts to ~/.rew/objects/
   → Claude Code proceeds to modify

3. Claude Code finishes modification
   → PostToolUse hook fires
   → rew records the Change in database
   → rew computes diff + line counts

4. Claude Code finishes responding
   → Stop hook fires
   → rew marks Task as Completed
   → rew computes summary stats

5. Task appears in rew desktop GUI
   → "AI 任务" tab
   → Timeline shows your session
   → Click to see all files changed
```

### Performance Impact

Hooks add minimal overhead:
- **UserPromptSubmit:** ~1.5ms
- **PreToolUse:** ~2ms (allow) / <1ms (deny)
- **PostToolUse:** ~3ms
- **Stop:** ~2.5ms

**Total per response:** 10-20ms (imperceptible to user)

---

## Uninstalling Hooks

If you want to remove rew hooks from Claude Code:

```bash
./target/release/rew uninstall
```

This removes:
- Hooks from Claude Code settings
- Hooks from Cursor settings (if installed)
- LaunchAgent (daemon won't auto-start)

**Note:** Your existing task data in `~/.rew/snapshots.db` is preserved.

To fully clean up:

```bash
# Remove database and backups
rm -rf ~/.rew

# Remove any .rewscope files
find ~ -name ".rewscope" -type f -delete
```

---

## Advanced: Manual Hook Injection

If `rew install` doesn't work for you, you can manually add hooks:

### Manual Claude Code Hook Injection

1. Find your rew binary path:
```bash
which rew
# or
/Users/kuqili/Desktop/project/rew/target/release/rew
```

2. Open `~/.claude/settings.json` (or create if missing)

3. Add this structure (preserve other settings!):
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

4. Save and restart Claude Code

### Manual Cursor Hook Injection

1. Create `~/.cursor/hooks.json`:
```json
{
  "beforeSubmitPrompt": [{
    "command": "/path/to/rew hook prompt",
    "description": "rew: create task"
  }],
  "beforeShellExecution": [{
    "command": "/path/to/rew hook pre-tool",
    "description": "rew: scope check"
  }],
  "afterFileEdit": [{
    "command": "/path/to/rew hook post-tool",
    "description": "rew: record change"
  }],
  "stop": [{
    "command": "/path/to/rew hook stop",
    "description": "rew: finalize task"
  }]
}
```

2. Save and restart Cursor

---

## Next Steps

1. ✅ Install hooks: `rew install`
2. ✅ Start daemon: `rew daemon`
3. ✅ Open rew desktop app
4. ✅ Use Claude Code normally
5. ✅ View sessions in rew timeline
6. ✅ Click sessions to view diffs
7. ✅ Restore files if needed with one click

---

## FAQ

**Q: Can I use rew without installing hooks?**

A: Yes! rew works as a file monitor without hooks. But hooks give you:
- Accurate prompt text for each session
- Per-tool tracking (Claude Code vs Cursor)
- Scope-based access control
- Precise session boundaries

**Q: Do hooks work with Claude Code in VS Code?**

A: Not yet. The VS Code extension doesn't support the same hook system. But file monitoring still tracks changes.

**Q: What if Claude Code updates and breaks hooks?**

A: You'll need to re-run `rew install` after Claude Code updates. The install logic checks for hook compatibility.

**Q: Can I customize hook behavior?**

A: Partially. You can customize `.rewscope` rules to control which files Claude Code can modify. Hook events themselves are fired by Claude Code automatically.

**Q: Does rew collect or send data?**

A: No. All data stays on your machine:
- `~/.rew/snapshots.db` (local SQLite)
- `~/.rew/objects/` (local file backups)
- No network calls
- No telemetry

---

## Getting Help

If hooks aren't working:

1. **Check logs:**
   ```bash
   log show --predicate 'process == "rew"' --last 1h
   ```

2. **Verify installation:**
   ```bash
   grep "rew hook" ~/.claude/settings.json
   ps aux | grep "rew daemon"
   ```

3. **Test hooks manually:**
   ```bash
   echo '{"tool_name":"Edit","file_path":"test.txt"}' | rew hook pre-tool
   ```

4. **Check rew status:**
   ```bash
   rew status
   rew list --limit 5
   ```

---

## See Also

- `CLAUDE_CODE_HOOK_INTEGRATION.md` — Deep dive into hook architecture
- `REW_ARCHITECTURE.md` — Full system architecture
- `README.md` — Project overview

