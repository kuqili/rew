# Claude Code Integration Guide

Complete guide for integrating Claude Code with rew to track all your AI-assisted development sessions.

## Quick Start (3 minutes)

```bash
# 1. Install and configure
/Users/kuqili/Desktop/project/rew/target/release/rew install

# 2. Start daemon
/Users/kuqili/Desktop/project/rew/target/release/rew daemon

# 3. Verify setup
cat ~/.claude-internal/settings.json | jq '.hooks | keys'

# 4. Restart Claude Code completely (important!)
# Then create a test file in Claude Code and watch rew capture it
```

## What You'll Get

✅ **Automatic tracking** - Every Claude Code session captured  
✅ **File change history** - See exactly what was created/modified/deleted  
✅ **File restoration** - One-click undo to restore files to any previous state  
✅ **Diff viewer** - Review changes before restoring  
✅ **Desktop app** - Beautiful UI to browse all sessions  
✅ **Safety first** - Scope rules prevent AI from modifying protected files  

## Complete Documentation

### For Installation & Setup
→ **[IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md)**
- Step-by-step installation checklist
- Verification commands
- End-to-end testing
- Troubleshooting guide

### For Understanding How It Works
→ **[HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md)**
- How hooks capture AI operations
- Data flow diagrams
- Database integration
- Adding support for new tools
- Performance characteristics

### For Detailed Installation Help
→ **[CLAUDE_CODE_HOOK_INSTALL.md](CLAUDE_CODE_HOOK_INSTALL.md)**
- Prerequisite checks
- Two installation options (automatic & manual)
- Verification procedures
- Troubleshooting by issue type
- Advanced configuration

### For Hook System Research
→ **[COMPETITOR_HOOK_RESEARCH.md](COMPETITOR_HOOK_RESEARCH.md)**
- Hook capabilities of Claude Code, Cursor, GitHub Copilot, Aider, Windsurf
- Comparison of hook systems
- Research limitations
- Recommendations for future improvements

### For System Architecture Overview
→ **[ARCHITECTURE_SUMMARY.md](ARCHITECTURE_SUMMARY.md)**
- Complete system architecture
- Data model details
- Database schema
- Frontend components
- Tauri IPC commands

## The Problem We're Solving

When you use Claude Code for development:

❌ **Before rew**: AI modifications disappear from view. If Claude Code makes a mistake, you're manually fixing it and don't have a record of what went wrong.

✅ **After rew**: Every Claude Code session is automatically recorded with all file changes. You can instantly see what was changed, review the modifications, and restore any file to any previous state with one click.

## How It Works (30-Second Summary)

```
Claude Code Session
    ↓
rew hook triggers at 4 points:
    ├─ UserPromptSubmit → Create task record
    ├─ PreToolUse → Check scope rules & backup file
    ├─ PostToolUse → Record file change
    └─ Stop → Mark task complete

    ↓
Database updated with task & changes
    ↓
rew desktop app displays task in timeline
    ↓
User can inspect files and restore as needed
```

## Installation Methods

### Method 1: Automatic (Recommended)
```bash
/Users/kuqili/Desktop/project/rew/target/release/rew install
```
Automatically detects Claude Code and injects hooks in one command.

### Method 2: Manual
Edit `~/.claude-internal/settings.json` and add hook commands.
See CLAUDE_CODE_HOOK_INSTALL.md for full instructions.

## First Time Setup

1. **Install hooks**: Run `rew install` or manually add to Claude Code settings
2. **Start daemon**: `rew daemon` (or automatically via LaunchAgent)
3. **Restart Claude Code**: Close and reopen completely
4. **Create test file**: Use Claude Code to create a simple test file
5. **Verify in rew app**: Open rew desktop app and look for the task

## Key Features Explained

### 📝 Task Recording
Each Claude Code session creates a "task" with:
- Your original prompt
- Tool name (Claude Code)
- Start/end timestamps
- Summary of changes (e.g., "3 files changed +15 -8")

### 🔍 Change Tracking
Each file change includes:
- File path
- Change type (created, modified, deleted, renamed)
- Lines added/removed
- SHA-256 hash of old/new content
- Timestamp of when file was restored (if applicable)

### 💾 File Backup
Before Claude Code modifies a file:
- rew makes a backup to `~/.rew/objects/`
- Uses macOS clonefile for efficient CoW storage
- Original content available for comparison

### 🛡️ Scope Rules
Define what files Claude Code can access:
- `.rewscope` file in your project
- Whitelisting (allow) and blacklisting (deny) patterns
- Pre-tool hook can deny risky operations
- Exit code 2 blocks the operation

### 📊 Desktop App
View all Claude Code sessions:
- Timeline view with filtering
- Date range selector
- View mode switcher (scheduled vs AI tasks)
- Click to inspect individual files
- One-click file restoration

### ↩️ Restore Files
Restore any file to a previous state:
- View diff before confirming
- Restore individual files or entire task
- See restoration history
- Unlimited restores (unlike game save points)

## Typical Workflow

### Day-to-Day Usage

```bash
# Every morning:
rew daemon  # Ensure daemon is running

# During development:
# Use Claude Code normally - rew captures everything

# Review your work:
open http://localhost:8080  # View rew desktop app

# If Claude Code made a mistake:
# Click the task → Select file → Click "↩ 读档" → File restored!
```

### After Claude Code Breaks Something

1. **Identify the problem**: rew app shows exactly what changed
2. **Review the diff**: Compare old vs new content in diff viewer
3. **Restore the file**: One-click restore to previous state
4. **Continue working**: No manual undo needed

## Performance

Each hook call is optimized for speed:
- `rew hook prompt`: 2ms
- `rew hook pre-tool`: 3-5ms (includes file backup)
- `rew hook post-tool`: 1ms (async)
- `rew hook stop`: 2ms

Total overhead on Claude Code operations: < 10ms per operation

## Troubleshooting Quick Links

| Problem | Solution |
|---------|----------|
| Hooks not working | [IMPLEMENTATION_CHECKLIST.md#troubleshooting](IMPLEMENTATION_CHECKLIST.md#troubleshooting) |
| Tasks not appearing | [CLAUDE_CODE_HOOK_INSTALL.md#tasks-not-appearing](CLAUDE_CODE_HOOK_INSTALL.md#tasks-not-appearing) |
| Pre-tool returning exit 2 | [CLAUDE_CODE_HOOK_INSTALL.md#hooks-are-blocking](CLAUDE_CODE_HOOK_INSTALL.md#hooks-are-blocking) |
| Desktop app shows nothing | [IMPLEMENTATION_CHECKLIST.md#troubleshooting](IMPLEMENTATION_CHECKLIST.md#troubleshooting) |
| Scope rules too restrictive | [HOOK_ARCHITECTURE.md#scope-engine](HOOK_ARCHITECTURE.md#scope-engine) |

## Next Steps

1. **Read**: Pick a guide above based on your needs
2. **Install**: Follow the installation checklist
3. **Verify**: Run the verification commands
4. **Test**: Create a file in Claude Code and watch rew capture it
5. **Explore**: Open the rew desktop app and review your tasks

## Advanced Topics

- **Adding new AI tools**: See [HOOK_ARCHITECTURE.md#adding-support](HOOK_ARCHITECTURE.md#adding-support-for-new-ai-tools)
- **Custom scope rules**: See [HOOK_ARCHITECTURE.md#scope-engine](HOOK_ARCHITECTURE.md#scope-engine)
- **Hook performance**: See [HOOK_ARCHITECTURE.md#performance](HOOK_ARCHITECTURE.md#performance-characteristics)
- **Extending hooks**: See [HOOK_ARCHITECTURE.md#future-enhancements](HOOK_ARCHITECTURE.md#future-enhancements)

## Project Structure

```
rew/
├── CLAUDE_CODE_INTEGRATION_GUIDE.md  ← You are here
├── IMPLEMENTATION_CHECKLIST.md        ← Start here for installation
├── CLAUDE_CODE_HOOK_INSTALL.md        ← Detailed installation guide
├── HOOK_ARCHITECTURE.md               ← How it works
├── COMPETITOR_HOOK_RESEARCH.md        ← Other tools' hook systems
│
├── crates/
│   ├── rew-cli/
│   │   ├── src/commands/
│   │   │   ├── install.rs             ← Hook injection code
│   │   │   └── hook.rs                ← Hook handlers
│   │   └── ...
│   ├── rew-core/
│   │   ├── src/
│   │   │   ├── types.rs               ← Task/Change data models
│   │   │   ├── db.rs                  ← Database operations
│   │   │   ├── scope.rs               ← Scope rule engine
│   │   │   ├── objects.rs             ← Content-addressed storage
│   │   │   └── ...
│   │   └── ...
│   └── ...
│
├── gui/
│   ├── src/
│   │   ├── components/
│   │   │   ├── TaskTimeline.tsx        ← Task list UI
│   │   │   └── TaskDetail.tsx          ← Task details UI
│   │   ├── hooks/
│   │   │   └── useTasks.ts             ← Frontend data fetching
│   │   └── lib/
│   │       └── tauri.ts                ← IPC type definitions
│   └── ...
│
└── src-tauri/
    └── src/
        └── commands.rs                  ← Tauri IPC handlers
```

## Getting Help

1. **Check the checklists**: [IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md)
2. **Read the architecture**: [HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md)
3. **See detailed guide**: [CLAUDE_CODE_HOOK_INSTALL.md](CLAUDE_CODE_HOOK_INSTALL.md)
4. **Research competitors**: [COMPETITOR_HOOK_RESEARCH.md](COMPETITOR_HOOK_RESEARCH.md)

## FAQ

**Q: Will this slow down Claude Code?**  
A: No, hook overhead is <10ms per operation (imperceptible).

**Q: Can I restore individual files?**  
A: Yes, click any task → select file → "↩ 读档" button.

**Q: What if Claude Code is still running?**  
A: Hooks work during the session. Task marked as complete when Claude Code finishes.

**Q: Can I share rew between users?**  
A: Each user needs their own `~/.rew/` directory. Hooks are per-user.

**Q: Does rew work offline?**  
A: Yes, everything is local. No cloud storage or network calls needed.

**Q: Can I move the rew binary?**  
A: Yes, but you'll need to re-run `rew install` or manually update Claude Code settings.

## Support

For issues or questions:
1. Check troubleshooting section in relevant guide
2. Run diagnostics script: `./diagnose-rew.sh`
3. Review database directly: `sqlite3 ~/.rew/snapshots.db`
4. Check logs: `log stream --predicate 'process == "rew"'`

---

**Ready to get started?** → Go to [IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md)

**Want to understand the architecture?** → Read [HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md)

**Need detailed installation help?** → See [CLAUDE_CODE_HOOK_INSTALL.md](CLAUDE_CODE_HOOK_INSTALL.md)
