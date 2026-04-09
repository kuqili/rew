# 📚 rew Documentation — Quick Navigation

This document is your **index** to all rew documentation. Choose based on your goal:

---

## 🚀 I Want to Get Started NOW

**→ Read: [GETTING_STARTED_WITH_HOOKS.md](GETTING_STARTED_WITH_HOOKS.md)**

5-minute guide with 5 simple steps:
1. Verify prerequisites
2. Run `rew install`
3. Start daemon
4. Restart Claude Code
5. Test it

---

## 🔧 I Want to Install Hooks (Detailed)

**→ Read: [IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md)**

Complete verification and testing procedures:
- ✅ Pre-flight checks
- ✅ Automatic installation (recommended)
- ✅ Manual installation (step-by-step)
- ✅ Verification tests
- ✅ Troubleshooting

**Alternative: [CLAUDE_CODE_HOOK_INSTALL.md](CLAUDE_CODE_HOOK_INSTALL.md)**
- Explains what each hook does
- Manual JSON editing if needed
- Default `.rewscope` rules

---

## 🏗️ I Want to Understand the Architecture

**→ Read: [HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md)**

Complete technical explanation:
- Four-phase hook lifecycle (prompt → pre-tool → post-tool → stop)
- Data flow (stdin/stdout JSON format)
- Task and Change database models
- Exit code semantics (0 = allow, 2 = deny)
- Scope rules engine (`.rewscope`)
- Error handling and edge cases

**For a high-level overview:**
→ Read: [ARCHITECTURE_SUMMARY.md](ARCHITECTURE_SUMMARY.md)

---

## 📊 I Want a Quick Reference

**→ Read: [QUICK_REFERENCE.md](QUICK_REFERENCE.md)**

One-page lookup tables:
- Hook events and data formats
- Exit codes and their meanings
- File paths and config locations
- Common commands
- Debugging checklist

---

## 🔍 I Want to Compare with Competitors

**→ Read: [COMPETITOR_HOOK_RESEARCH.md](COMPETITOR_HOOK_RESEARCH.md)**

Hook systems analysis:
- **Claude Code** — Four-phase hooks with stdin/stdout JSON
- **Cursor** — Similar four-phase system (pre/post for each tool)
- **GitHub Copilot** — Eight hook events with command execution
- **Windsurf** — Twelve Cascade hooks with trajectory tracking
- **Aider** — No native hook system

Summary table comparing:
- Hook event count
- Data format (JSON/stdin/env vars)
- Blocking capability (exit codes)
- Session tracking

---

## 🧭 I'm Confused About Which Doc to Read

**→ Read: [CLAUDE_CODE_INTEGRATION_GUIDE.md](CLAUDE_CODE_INTEGRATION_GUIDE.md)**

This is a navigation guide that explains:
- What each document covers
- Which documents depend on each other
- Common questions and which doc answers them
- Recommended reading order

---

## 📘 Project Architecture (All Subsystems)

**→ Read: [REW_ARCHITECTURE.md](REW_ARCHITECTURE.md)**

Complete system architecture:
- rew-core (backup engine, database, scope rules, etc.)
- rew-cli (command-line tool)
- Tauri backend (desktop app backend)
- React frontend (UI)
- Data flow between subsystems
- File paths and storage

---

## 🎯 Quick Problem Solver

### Problem: "I don't know where to start"
→ [GETTING_STARTED_WITH_HOOKS.md](GETTING_STARTED_WITH_HOOKS.md)

### Problem: "Installation failed"
→ [IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md) (Troubleshooting section)

### Problem: "Hooks aren't running"
→ [QUICK_REFERENCE.md](QUICK_REFERENCE.md) (Debugging section)

### Problem: "I want to customize scope rules"
→ [HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md) (Scope Rules section)

### Problem: "I need to understand the whole system"
→ [REW_ARCHITECTURE.md](REW_ARCHITECTURE.md)

### Problem: "How does this compare to Cursor?"
→ [COMPETITOR_HOOK_RESEARCH.md](COMPETITOR_HOOK_RESEARCH.md)

---

## 📋 Documentation Index

| Document | Pages | Purpose | Audience |
|----------|-------|---------|----------|
| **GETTING_STARTED_WITH_HOOKS.md** | 1 | 5-step setup guide | Everyone |
| **IMPLEMENTATION_CHECKLIST.md** | 3 | Verification & testing | Technical users |
| **HOOK_ARCHITECTURE.md** | 5 | Complete technical spec | Developers |
| **QUICK_REFERENCE.md** | 3 | Lookup tables | Everyone |
| **COMPETITOR_HOOK_RESEARCH.md** | 3 | Competitive analysis | Curious users |
| **CLAUDE_CODE_HOOK_INSTALL.md** | 2 | Installation methods | Users wanting manual control |
| **CLAUDE_CODE_INTEGRATION_GUIDE.md** | 3 | Navigation guide | Users confused about docs |
| **ARCHITECTURE_SUMMARY.md** | 5 | System overview | Technical users |
| **REW_ARCHITECTURE.md** | 7 | Complete architecture | Developers |

---

## 🎓 Reading Paths

### Path 1: "Just Make It Work" (15 minutes)
1. [GETTING_STARTED_WITH_HOOKS.md](GETTING_STARTED_WITH_HOOKS.md)
2. Run `rew install`
3. Test it

### Path 2: "I Want to Understand Everything" (1-2 hours)
1. [ARCHITECTURE_SUMMARY.md](ARCHITECTURE_SUMMARY.md) — Big picture
2. [HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md) — Deep dive
3. [REW_ARCHITECTURE.md](REW_ARCHITECTURE.md) — System design
4. [COMPETITOR_HOOK_RESEARCH.md](COMPETITOR_HOOK_RESEARCH.md) — Context

### Path 3: "I'm a Developer/Contributor" (2-3 hours)
1. [REW_ARCHITECTURE.md](REW_ARCHITECTURE.md) — System design
2. [HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md) — Hook details
3. Read the source code in `/crates/rew-cli/src/commands/hook.rs`
4. Read the source code in `/crates/rew-cli/src/commands/install.rs`

### Path 4: "Something's Broken" (30 minutes)
1. [GETTING_STARTED_WITH_HOOKS.md](GETTING_STARTED_WITH_HOOKS.md) (Troubleshooting section)
2. [IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md) (Troubleshooting section)
3. [QUICK_REFERENCE.md](QUICK_REFERENCE.md) (Debugging section)

---

## 💾 One-Command Setup

```bash
# Everything you need to do
/Users/kuqili/Desktop/project/rew/target/release/rew install

# Then restart Claude Code
# Then test it:
/Users/kuqili/Desktop/project/rew/target/release/rew list
```

---

## 🆘 Need Help?

1. **Quick answer:** [QUICK_REFERENCE.md](QUICK_REFERENCE.md)
2. **Stuck on installation:** [IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md)
3. **Want to understand:** [HOOK_ARCHITECTURE.md](HOOK_ARCHITECTURE.md)
4. **Confused about docs:** [CLAUDE_CODE_INTEGRATION_GUIDE.md](CLAUDE_CODE_INTEGRATION_GUIDE.md)
5. **System not working:** Troubleshooting section in any of the above

---

**Start here: [GETTING_STARTED_WITH_HOOKS.md](GETTING_STARTED_WITH_HOOKS.md)**
