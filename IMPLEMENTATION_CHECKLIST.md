# Claude Code Hook Implementation Checklist

This checklist outlines exactly what you need to do to get Claude Code hooks working with rew.

## ✅ Pre-Flight Checks

- [ ] **rew binary is built**
  ```bash
  ls -la /Users/kuqili/Desktop/project/rew/target/release/rew
  /Users/kuqili/Desktop/project/rew/target/release/rew --version
  ```

- [ ] **Claude Code is installed**
  ```bash
  ls -la ~/.claude-internal/ || ls -la ~/.claude/
  ```

- [ ] **rew home directory exists**
  ```bash
  ls -la ~/.rew/
  ```

## 🔧 Installation

### Option A: Automatic Installation (Recommended)

**Step 1: Run the install command**
```bash
/Users/kuqili/Desktop/project/rew/target/release/rew install
```

**Expected output:**
```
ℹ 正在安装 LaunchAgent...
✓ LaunchAgent 已安装

ℹ 正在检测 AI 工具并注入 hook...
✓ Claude Code hook 已注入

✓ 已生成 .rewscope 规则文件

  使用 rew daemon 立即启动守护进程
```

**Verification:**
```bash
grep -c "rew hook" ~/.claude-internal/settings.json
# Should output: 4 (four hook types)
```

### Option B: Manual Installation

**Step 1: Backup current Claude Code settings**
```bash
cp ~/.claude-internal/settings.json ~/.claude-internal/settings.json.backup
```

**Step 2: Get the rew binary path**
```bash
which rew
# Or if not in PATH:
/Users/kuqili/Desktop/project/rew/target/release/rew
```

**Step 3: Edit settings.json**
```bash
# Open in your editor
nano ~/.claude-internal/settings.json
```

Add these hooks under the `"hooks"` section (create if doesn't exist):

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

**Step 4: Verify formatting**
```bash
python3 -m json.tool ~/.claude-internal/settings.json > /dev/null
echo $? # Should output: 0
```

## 🚀 Start the rew Daemon

The daemon monitors files and runs in the background:

```bash
/Users/kuqili/Desktop/project/rew/target/release/rew daemon
```

Or use the LaunchAgent (installed by `rew install`):
```bash
# LaunchAgent automatically starts rew daemon at login
launchctl list | grep rew
```

## ✅ Verify Installation

### 1. Check hook configuration

```bash
cat ~/.claude-internal/settings.json | jq '.hooks'
```

You should see four hook definitions with paths pointing to rew.

### 2. Test hooks manually

```bash
# Test prompt hook
echo "test prompt" | /Users/kuqili/Desktop/project/rew/target/release/rew hook prompt

# Test pre-tool hook (should output 0 for success)
echo '{"tool_name":"Write","file_path":"./test.txt"}' | \
  /Users/kuqili/Desktop/project/rew/target/release/rew hook pre-tool
echo $? # Should be 0
```

### 3. Check database

```bash
sqlite3 ~/.rew/snapshots.db "SELECT COUNT(*) FROM tasks;"
# If you ran the manual tests above, should show: 1
```

### 4. List tasks

```bash
/Users/kuqili/Desktop/project/rew/target/release/rew list
```

Should show at least the test task you created.

## 🧪 End-to-End Test

### Step 1: Restart Claude Code

Close and reopen Claude Code completely (important - it caches settings).

### Step 2: Create a test file

In Claude Code, create a new conversation with this prompt:

```
Create a test file called hello.md with the content:
# Hello World

This is a test file created by Claude Code.
```

Let Claude Code execute the Write operation.

### Step 3: Check rew desktop app

Open the rew desktop app and:
- Switch to "AI 任务" tab
- Look for a new task in the timeline
- The task should show:
  - Claude Code badge
  - Your prompt text (truncated)
  - "1 文件" indicator
  - Current timestamp

### Step 4: Inspect task details

Click on the task to see:
- Full prompt text
- File list showing hello.md
- Change type: "A" (Added/Created)
- Diff viewer showing the new content

## 🔍 Troubleshooting

### Issue: Hooks not being called

**Diagnosis:**
```bash
# 1. Check settings were updated
grep "rew hook" ~/.claude-internal/settings.json || echo "❌ Hooks not in settings"

# 2. Verify JSON is valid
python3 -m json.tool ~/.claude-internal/settings.json > /dev/null || echo "❌ JSON invalid"

# 3. Check rew binary is accessible
/Users/kuqili/Desktop/project/rew/target/release/rew --version || echo "❌ rew binary not accessible"
```

**Solution:**
- Restart Claude Code completely
- Re-run `rew install`
- Check that ~/.claude-internal/ directory exists

### Issue: Tasks not appearing in desktop app

**Diagnosis:**
```bash
# 1. Check daemon is running
pgrep -f "rew daemon" || echo "❌ Daemon not running"

# 2. Check database has tasks
sqlite3 ~/.rew/snapshots.db "SELECT COUNT(*) FROM tasks;"

# 3. Try listing tasks directly
/Users/kuqili/Desktop/project/rew/target/release/rew list
```

**Solution:**
```bash
# Start daemon if not running
/Users/kuqili/Desktop/project/rew/target/release/rew daemon

# Or use LaunchAgent
launchctl start com.rew.daemon
```

### Issue: Pre-tool hook returning exit code 2 (denied)

This means `.rewscope` rules are blocking the operation.

**Diagnosis:**
```bash
cat .rewscope
```

**Solution:** Review and relax the deny rules:
```yaml
allow:
  - "./**"           # Allow all in current dir

deny:
  - "~/.ssh/**"      # Only block SSH keys
```

### Issue: Desktop app shows "暂无 AI 任务" (No AI tasks)

**Common causes:**
1. **Daemon not running** - See above
2. **Date filter** - Check you're looking at the right date
3. **Directory filter** - Make sure you're not filtering by directory
4. **Tasks in database but not showing** - Try refreshing the app

**Solution:**
```bash
# Check date filter - set to "近24h" (Last 24h)
# Or use CLI:
/Users/kuqili/Desktop/project/rew/target/release/rew list
```

## 📋 Step-by-Step Quick Start

For quick reference, here's the minimal setup:

```bash
# 1. Install (one-time)
/Users/kuqili/Desktop/project/rew/target/release/rew install

# 2. Start daemon (or it starts automatically via LaunchAgent)
/Users/kuqili/Desktop/project/rew/target/release/rew daemon

# 3. Verify
cat ~/.claude-internal/settings.json | jq '.hooks | keys'

# 4. Test
echo "test" | /Users/kuqili/Desktop/project/rew/target/release/rew hook prompt

# 5. Open rew app and look for new tasks
# URL: http://localhost:8080 (or wherever rew app is running)
```

## 🎯 Expected Behavior

When everything is working correctly:

1. **Submit prompt in Claude Code** → Task appears in rew database immediately
2. **Claude Code performs Write/Edit** → File change recorded in database
3. **Claude Code finishes** → Task marked as completed
4. **rew app polls database** → New task appears in timeline within 1-2 seconds
5. **Click task** → See all files changed and diffs
6. **Click "↩ 读档"** → Restore any file to previous state

## 📚 Additional Resources

- **CLAUDE_CODE_HOOK_INSTALL.md** - Detailed installation guide
- **HOOK_ARCHITECTURE.md** - Deep dive into how hooks work
- **ARCHITECTURE_SUMMARY.md** - Overall system architecture
- **QUICK_REFERENCE.md** - Data model quick reference

## 🆘 Getting Help

If you encounter issues:

1. **Check the diagnostics script:**
   ```bash
   # Save as diagnose-rew.sh and run
   chmod +x diagnose-rew.sh
   ./diagnose-rew.sh
   ```

2. **Check logs:**
   ```bash
   # LaunchAgent logs
   log stream --predicate 'process == "rew"'
   
   # Database errors
   sqlite3 ~/.rew/snapshots.db "SELECT * FROM sqlite_master WHERE type='table';"
   ```

3. **Manual hook testing:**
   ```bash
   # Create a test task
   echo "test prompt" | /Users/kuqili/Desktop/project/rew/target/release/rew hook prompt
   
   # Verify it's in database
   sqlite3 ~/.rew/snapshots.db "SELECT id, prompt FROM tasks ORDER BY started_at DESC LIMIT 1;"
   ```

## 🎉 Success Criteria

You'll know it's working when:

- [ ] ✓ `rew install` completes without errors
- [ ] ✓ Claude Code settings contain rew hook commands
- [ ] ✓ `rew list` shows tasks created from Claude Code
- [ ] ✓ rew desktop app displays tasks in "AI 任务" tab
- [ ] ✓ Clicking a task shows file changes and diffs
- [ ] ✓ "↩ 读档" button successfully restores files

Once all these are checked, you're good to go! 🚀
