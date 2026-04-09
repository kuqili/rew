# Getting Started: Installing Claude Code Hooks

This is your **quick-start guide** to get Claude Code hooks working with rew. By the end of this, all Claude Code sessions will appear in your rew desktop app.

## What You're About to Do

You're going to:
1. ✅ Build the rew CLI tool (it's already built)
2. ✅ Run a single command to inject hooks
3. ✅ Restart Claude Code
4. ✅ Test it with a quick verification

**Expected time: 3-5 minutes**

---

## Step 1: Verify Prerequisites

Make sure you have:

```bash
# Check rew CLI is built
ls -la /Users/kuqili/Desktop/project/rew/target/release/rew
/Users/kuqili/Desktop/project/rew/target/release/rew --version
# Should show: rew 0.1.0

# Check Claude Code is installed
ls -la ~/.claude-internal/ || echo "Claude Code not found"
```

---

## Step 2: Install Hooks (One Command)

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

**What happened:**
- ✅ LaunchAgent installed (runs rew daemon at login)
- ✅ Hooks injected into `~/.claude-internal/settings.json`
- ✅ `.rewscope` default rules created in your current directory

---

## Step 3: Start the rew Daemon

Choose one approach:

### Option A: LaunchAgent (Automatic, runs at login)
```bash
# The LaunchAgent was installed above. It will auto-start rew daemon on next login.
# To test it immediately without rebooting:
launchctl start com.kuqili.rew.daemon
```

### Option B: Manual (Foreground, for testing)
```bash
/Users/kuqili/Desktop/project/rew/target/release/rew daemon
# Keep this terminal window open
```

---

## Step 4: Restart Claude Code

Close Claude Code completely and reopen it.

**Why:** Claude Code reads hooks from settings.json at startup.

---

## Step 5: Quick Verification

Open Claude Code and create a simple test:

```
Create a file called test.txt with content "hello world"
```

Then check if rew captured it:

```bash
# Check rew is running
/Users/kuqili/Desktop/project/rew/target/release/rew status

# List recent tasks
/Users/kuqili/Desktop/project/rew/target/release/rew list --limit 5

# Show the latest task details
/Users/kuqili/Desktop/project/rew/target/release/rew show --latest
```

You should see the task with your file changes!

---

## What's Happening Behind the Scenes

When you use Claude Code:

1. **User submits prompt** → Hook runs: `rew hook prompt`
   - Creates a Task record in rew's database
   - Records your prompt text
   
2. **Claude Code writes a file** → Hook runs: `rew hook pre-tool` (before) and `rew hook post-tool` (after)
   - Backs up the file's old version
   - Records changes (created/modified/deleted)
   - Computes file hashes
   
3. **Claude Code finishes** → Hook runs: `rew hook stop`
   - Marks task as completed
   - Computes summary statistics

All of this happens **asynchronously** and **doesn't slow down Claude Code**.

---

## Troubleshooting

### Hooks aren't working
**Check 1: Verify hooks are injected**
```bash
grep "rew hook" ~/.claude-internal/settings.json
# Should show 4 matches (prompt, pre-tool, post-tool, stop)
```

**Check 2: Verify rew daemon is running**
```bash
ps aux | grep rew
# Should show: rew daemon (if running)
```

**Check 3: Restart Claude Code completely**
- Close Claude Code via Cmd+Q (not just minimize)
- Reopen it fresh
- Hooks are loaded at startup only

### Still not working?
See the detailed troubleshooting section in **IMPLEMENTATION_CHECKLIST.md**

---

## Next Steps

### See Your Tasks in the Desktop App
```bash
# Build and run the desktop app (requires Tauri)
cd /Users/kuqili/Desktop/project/rew
npm run tauri dev
```

Then open the rew desktop app and:
- Browse the timeline of all Claude Code sessions
- Click on any task to see what files were changed
- View diffs for each file
- Rollback any changes with one click

### Understand the Architecture
- Read **HOOK_ARCHITECTURE.md** — detailed explanation of the four-phase hook system
- Read **QUICK_REFERENCE.md** — exit codes, hook events, debugging commands
- Read **COMPETITOR_HOOK_RESEARCH.md** — how this compares to Cursor, Copilot, Windsurf

### Customize Scope Rules
Edit `.rewscope` in your project root to:
- Restrict which files Claude Code can modify
- Add warning alerts for risky commands
- Create team-wide policies

See **HOOK_ARCHITECTURE.md** section "Scope Rules (`.rewscope`)" for details.

---

## Key Documentation

| Document | Purpose |
|----------|---------|
| **IMPLEMENTATION_CHECKLIST.md** | Step-by-step verification & testing |
| **HOOK_ARCHITECTURE.md** | Complete technical explanation |
| **QUICK_REFERENCE.md** | Exit codes, debugging, quick lookup |
| **COMPETITOR_HOOK_RESEARCH.md** | Compare with Cursor, Copilot, Windsurf |
| **CLAUDE_CODE_HOOK_INSTALL.md** | Installation methods (auto & manual) |
| **CLAUDE_CODE_INTEGRATION_GUIDE.md** | Navigation hub for all docs |

---

## Commands at a Glance

```bash
# Install hooks
/Users/kuqili/Desktop/project/rew/target/release/rew install

# Start daemon (manual)
/Users/kuqili/Desktop/project/rew/target/release/rew daemon

# Check status
/Users/kuqili/Desktop/project/rew/target/release/rew status

# List tasks
/Users/kuqili/Desktop/project/rew/target/release/rew list

# Show task details
/Users/kuqili/Desktop/project/rew/target/release/rew show <task-id>

# Rollback a task
/Users/kuqili/Desktop/project/rew/target/release/rew undo <task-id>

# View unified diff
/Users/kuqili/Desktop/project/rew/target/release/rew diff <task-id> <file-path>

# Uninstall hooks
/Users/kuqili/Desktop/project/rew/target/release/rew uninstall
```

---

**You're ready to go! Run `rew install` and restart Claude Code.**

Have questions? Check the troubleshooting section or dive into the detailed documentation.
