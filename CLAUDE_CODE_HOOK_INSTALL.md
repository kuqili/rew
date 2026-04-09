# Installing rew Hooks for Claude Code

This guide explains how to install rew hooks so that all Claude Code sessions automatically appear in the rew desktop app.

## What Does the Hook Do?

When installed, the rew hook system intercepts Claude Code's operations at key moments:
- **UserPromptSubmit** - When you submit a prompt to Claude Code
- **PreToolUse** - Before Claude Code modifies a file (scope check & backup)
- **PostToolUse** - After Claude Code completes a tool operation (record the change)
- **Stop** - When Claude Code finishes responding (close the task)

Each hook call invokes `rew hook <command>` which:
1. Reads JSON from stdin describing the operation
2. Updates the rew database with task/change information
3. Handles file backups and scope checks
4. Returns exit codes (0=allow, 2=deny)

## Prerequisites

1. **rew binary installed** - You need the `rew` CLI binary built
2. **Claude Code installed** - Claude Code with an active `~/.claude/` or `~/.claude-internal/` directory
3. **At least one project configured** - rew must have `~/.rew/` initialized with a database

## Installation Steps

### Option 1: Automatic Installation (Recommended)

The easiest way is to use the `rew install` command:

```bash
/Users/kuqili/Desktop/project/rew/target/release/rew install
```

This command:
1. Installs the LaunchAgent for the rew daemon (background file monitoring)
2. Detects Claude Code installation
3. Injects hooks into `~/.claude/settings.json` (or `~/.claude-internal/settings.json`)
4. Generates a default `.rewscope` rules file if one doesn't exist
5. Prints success/failure for each step

Expected output:
```
ℹ 正在安装 LaunchAgent...
✓ LaunchAgent 已安装

ℹ 正在检测 AI 工具并注入 hook...
✓ Claude Code hook 已注入

✓ 已生成 .rewscope 规则文件

  使用 rew daemon 立即启动守护进程
```

### Option 2: Manual Hook Installation

If automatic installation fails, you can manually add hooks to Claude Code settings:

1. **Locate Claude Code settings:**
   ```bash
   ls -la ~/.claude/settings.json
   # or
   ls -la ~/.claude-internal/settings.json
   ```

2. **Edit the settings file** and add or update the `hooks` section:

   ```json
   {
     "hooks": {
       "UserPromptSubmit": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Users/kuqili/Desktop/project/rew/target/release/rew hook prompt"
             }
           ]
         }
       ],
       "PreToolUse": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Users/kuqili/Desktop/project/rew/target/release/rew hook pre-tool"
             }
           ]
         }
       ],
       "PostToolUse": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Users/kuqili/Desktop/project/rew/target/release/rew hook post-tool"
             }
           ]
         }
       ],
       "Stop": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Users/kuqili/Desktop/project/rew/target/release/rew hook stop"
             }
           ]
         }
       ]
     }
   }
   ```

3. **Save the file** and restart Claude Code

## Verification

After installation, verify the hooks are working:

### 1. Check that hooks were injected

```bash
cat ~/.claude-internal/settings.json | jq '.hooks'
```

You should see the four hook definitions (UserPromptSubmit, PreToolUse, PostToolUse, Stop).

### 2. Test with Claude Code

1. Open Claude Code
2. Start a new conversation with a simple prompt: "Create a test file called test.md with 'hello world'"
3. Let Claude Code execute the Write operation
4. Open the rew desktop app and look for a new task in the "AI 任务" (AI Tasks) tab

Expected result:
- A new row appears in the timeline with:
  - "Claude Code" badge
  - Your prompt text truncated
  - "1 文件" (1 file)
  - Timestamp of when it was created

### 3. View task details

Click on the task to see:
- Full prompt text
- List of files changed
- For each file: change type (Created/Modified/Deleted), lines added/removed
- Diff viewer for each file

## Hook Troubleshooting

### Hooks aren't being called

1. **Verify Claude Code settings were updated:**
   ```bash
   grep "rew hook" ~/.claude-internal/settings.json
   ```

2. **Check that the rew binary path is correct:**
   ```bash
   /Users/kuqili/Desktop/project/rew/target/release/rew --version
   ```

3. **Restart Claude Code** - it may cache settings on startup

4. **Check Claude Code logs** - if hooks fail silently, there may be error output

### Tasks not appearing in rew desktop app

1. **Verify rew daemon is running:**
   ```bash
   pgrep -f "rew daemon"
   ```
   
   If not running, start it:
   ```bash
   /Users/kuqili/Desktop/project/rew/target/release/rew daemon
   ```

2. **Check database exists:**
   ```bash
   ls -la ~/.rew/snapshots.db
   ```

3. **List tasks directly:**
   ```bash
   /Users/kuqili/Desktop/project/rew/target/release/rew list
   ```

4. **Check logs** for any errors during hook execution

### Hooks are blocking operations (pre-tool returns exit 2)

This means the `.rewscope` rules are denying the operation. Check:

1. **View current scope rules:**
   ```bash
   cat .rewscope
   ```

2. **Review deny patterns** - they may be too restrictive

3. **Temporarily disable scope check** (dev only):
   Edit `.rewscope` to allow all paths:
   ```yaml
   allow:
     - "./**"
   
   deny: []
   ```

## Hook Data Flow

When you submit a prompt in Claude Code:

```
1. Claude Code: User types prompt → triggers "UserPromptSubmit" hook
   ↓
   rew hook prompt: Creates a new Task record (tool=null initially)
   
2. Claude Code: User clicks "Write test.md" → triggers "PreToolUse" hook
   ↓
   rew hook pre-tool: Checks scope rules, backs up existing file to objects store
   
3. Claude Code: Writes the file → triggers "PostToolUse" hook
   ↓
   rew hook post-tool: Records the Change (created/modified/deleted)
   
4. Claude Code: Finishes response → triggers "Stop" hook
   ↓
   rew hook stop: Marks task as completed, computes summary
   
5. rew desktop app: Polls list_tasks() → displays the task in timeline
```

## Uninstallation

To remove rew hooks from Claude Code:

```bash
/Users/kuqili/Desktop/project/rew/target/release/rew uninstall
```

This will:
- Remove all hook definitions from `~/.claude/settings.json`
- Remove the LaunchAgent
- Clean up temporary files

You can verify by checking that `~/.claude-internal/settings.json` no longer contains rew hooks.

## Advanced: Custom Hook Commands

If you want to run the hook from a different location (e.g., via npm/yarn binary):

1. **Create a wrapper script** at `/usr/local/bin/rew-hook`:
   ```bash
   #!/bin/bash
   /Users/kuqili/Desktop/project/rew/target/release/rew "$@"
   ```

2. **Make it executable:**
   ```bash
   chmod +x /usr/local/bin/rew-hook
   ```

3. **Update Claude Code settings** to use `rew-hook` instead of the full path

This approach makes it easier to move the rew binary later without updating Claude Code settings.

## Environment Variables

The hook commands accept these environment variables:

- `REW_HOME` - Override ~/.rew directory (default: ~/.rew)
- `REW_VERBOSE` - Enable debug output (0=off, 1=on)

Example:
```bash
REW_VERBOSE=1 /Users/kuqili/Desktop/project/rew/target/release/rew hook prompt
```

## Performance Notes

Each hook invocation is designed to be very fast:
- `rew hook prompt` - <2ms (writes task record)
- `rew hook pre-tool` - <3ms (scope check + file backup)
- `rew hook post-tool` - <1ms (async, doesn't block)
- `rew hook stop` - <1ms (async, doesn't block)

The hot path uses a Unix socket connection to the daemon (when running), reducing overhead to <1ms per call.

## Related Commands

Once hooks are installed, you may also want to use:

```bash
# Start the rew daemon (file monitoring)
rew daemon

# View all tasks
rew list

# Restore a task
rew restore <task_id>

# Show help for hook commands
rew hook --help
```

## Troubleshooting Script

If you're having issues, run this diagnostic script:

```bash
#!/bin/bash

echo "=== rew Installation Diagnostics ==="

echo ""
echo "1. rew binary:"
rew --version || echo "❌ rew binary not found in PATH"
which rew || echo "❌ rew not in PATH"

echo ""
echo "2. rew home directory:"
test -d ~/.rew && echo "✓ ~/.rew exists" || echo "❌ ~/.rew missing"
test -f ~/.rew/snapshots.db && echo "✓ database exists" || echo "❌ database missing"

echo ""
echo "3. Claude Code settings:"
test -f ~/.claude-internal/settings.json && echo "✓ Claude Code settings found" || echo "❌ Claude Code settings not found"
grep -q "rew hook" ~/.claude-internal/settings.json 2>/dev/null && echo "✓ rew hooks injected" || echo "❌ rew hooks not found in settings"

echo ""
echo "4. rew daemon:"
pgrep -f "rew daemon" > /dev/null && echo "✓ rew daemon running" || echo "❌ rew daemon not running"

echo ""
echo "5. Recent tasks:"
rew list | head -5 || echo "❌ Failed to list tasks"
```

Save this as `diagnose-rew.sh`, make it executable (`chmod +x`), and run it to check your setup.
