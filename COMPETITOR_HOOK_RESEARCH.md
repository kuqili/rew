# AI Tool Hook Systems Research

This document summarizes the hook/event capabilities of various AI coding tools, as of April 2026.

## Executive Summary

| Tool | Hook System | Events | Use Cases | Status |
|------|-------------|--------|-----------|--------|
| **Claude Code** | ✅ Yes | 4 events | Scope check, backup, tracking | Production-ready |
| **Cursor** | ✅ Yes | 4 events | File editing, shell execution | Production-ready |
| **GitHub Copilot** | ✅ Yes (Agent) | 8 events | Session lifecycle, tool use | Public Beta |
| **Aider** | ❌ No | N/A | N/A | Not available |
| **Windsurf/Codeium** | ❓ Unknown | N/A | N/A | Undocumented |

## Detailed Analysis

### 1. Claude Code ✅

**Status**: Fully supported by rew

**Hook Events**:
- `UserPromptSubmit` - User submits a prompt
- `PreToolUse` - Before tool execution (can deny)
- `PostToolUse` - After tool execution
- `Stop` - AI finishes responding

**Configuration Format**:
```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/script"
          }
        ]
      }
    ]
  }
}
```

**Config Location**: `~/.claude/settings.json` or `~/.claude-internal/settings.json`

**Data Format**: 
- Input via stdin (JSON for most events)
- Exit code determines allow/deny
- Exit 0 = allow, Exit 2 = deny

**rew Integration**: ✅ Complete
- Four hooks implemented
- Scope checking on PreToolUse
- File backup on pre-tool
- Change tracking on post-tool
- Task lifecycle management on stop

---

### 2. Cursor ✅

**Status**: Fully supported by rew

**Hook Events**:
- `beforeSubmitPrompt` - Before user prompt is sent
- `beforeShellExecution` - Before shell command execution
- `afterFileEdit` - After file is edited
- `stop` - When AI finishes

**Configuration Format**:
```json
{
  "beforeShellExecution": [
    {
      "command": "/path/to/script",
      "description": "description"
    }
  ]
}
```

**Config Location**: `~/.cursor/hooks.json`

**Data Format**:
- Input via stdin (JSON)
- Exit code determines allow/deny
- Exit 0 = allow, Exit non-zero = deny

**rew Integration**: ✅ Complete
- Four hooks implemented (mapped to Claude Code equivalents)
- Compatible with rew's existing hook handlers

---

### 3. GitHub Copilot (Agent) ✅

**Status**: Public Beta, documented but not widely adopted

**Source**: [GitHub Copilot Coding Agent Documentation](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-hooks)

**Hook Events** (8 total):
1. `sessionStart` - When agent session begins or resumes
2. `sessionEnd` - When session completes or terminates
3. `userPromptSubmitted` - When user submits a prompt to the agent
4. `preToolUse` - Before the agent uses any tool
5. `postToolUse` - After a tool completes execution
6. `agentStop` - When the main agent finishes responding
7. `subagentStop` - When a subagent completes
8. `errorOccurred` - When an error occurs during execution

**Configuration Format**:
```json
{
  "version": 1,
  "hooks": {
    "hookType": [
      {
        "type": "command",
        "bash": "/path/to/bash/script",
        "powershell": "/path/to/powershell/script",
        "cwd": "/working/directory",
        "env": {"KEY": "value"},
        "timeoutSec": 30
      }
    ]
  }
}
```

**Key Features**:
- Version-controlled configuration schema
- Platform-specific command support (bash/powershell)
- Configurable timeout (defaults to some value)
- Environment variables pass-through
- Sessions have unique identifiers (implied)

**Important Limitations**:
- Documentation doesn't specify the exact JSON data passed to hooks
- Whether `session_id` is included in hook context is unclear
- Stdin vs environment variable usage not documented
- Actual field names for session/context data not detailed

**rew Integration**: ⚠️ Partial
- Hook events map well to rew's lifecycle
- Would need to test actual implementation
- Configuration format is more complex
- May need new hook handlers for additional events

---

### 4. Aider ❌

**Status**: No native hook system

**Source**: [GitHub Issue #802 - Pre-commit hooks are ignored by Aider](https://github.com/Aider-AI/aider/issues/802)

**Findings**:
- Aider bypasses Git pre-commit hooks when auto-committing
- No built-in event/hook system
- No plugin or extension architecture
- Git integration uses `--no-verify` flag (ignores hooks)

**Workarounds for Integration**:
1. Run external tools manually before/after Aider commands
2. Use community fork `augment-aider` which adds some hooks
3. Monitor directory separately with file watcher
4. No direct integration possible with native Aider

**rew Integration**: ❌ Not possible
- Aider doesn't support hooks
- Would require file system monitoring only
- Cannot guarantee file backup before AI operation

---

### 5. Windsurf (Codeium) ❓

**Status**: Undocumented, likely exists but not publicly available

**What We Know**:
- Windsurf is built on similar architecture to other modern IDEs
- Code editing likely triggers events
- Command execution likely observable
- No public documentation found

**Possible Approaches**:
- Check `~/.windsurf/` or `~/.config/windsurf/` for config files
- Look for hook/event configuration in settings
- Contact Codeium for documentation

**rew Integration**: ❓ Unknown
- Would need to research actual configuration format
- Likely similar to Cursor (also Chromium-based IDE)

---

## Hook System Design Comparison

### Scope of Events

**rew's Model** (Minimal, sufficient):
```
UserPromptSubmit → PreToolUse → PostToolUse → Stop
```
Simple 4-event lifecycle, easy to reason about.

**GitHub Copilot's Model** (Comprehensive):
```
SessionStart → UserPromptSubmitted → PreToolUse → PostToolUse → 
AgentStop/SubagentStop → SessionEnd ± ErrorOccurred
```
Covers more scenarios, enables more complex workflows.

### Data Passing

**Claude Code/Cursor**: 
- Stdin with JSON payload
- Simple, standard approach
- Works on any OS

**GitHub Copilot**:
- Stdin with JSON (implied)
- Also environment variables (implied)
- Cross-platform executable support (bash/powershell)

### Deny/Allow Mechanism

**All tools**:
- Exit codes determine success/failure
- Exit 0 = allow/success
- Exit non-zero = deny/failure
- Standard Unix convention

### Configuration

**Claude Code**: 
- Per-workspace settings
- Inline hook commands

**Cursor**: 
- Standalone hooks.json file
- Minimal metadata

**GitHub Copilot**: 
- Versioned schema
- Rich metadata (timeout, env, cwd)
- Platform-specific execution

## Research Limitations

### Information Not Available

1. **Exact Data Formats**: 
   - GitHub Copilot documentation references hook data but doesn't show schema
   - May need to test empirically

2. **Session Identifiers**:
   - Whether tools pass session_id/conversation_id unclear
   - Important for correlating multiple hooks from same session

3. **Timing Guarantees**:
   - No documentation on hook timing guarantees
   - Are hooks synchronous or asynchronous?
   - What if hook takes too long?

4. **Error Handling**:
   - What happens if hook fails?
   - Can the tool recover gracefully?
   - Retry logic?

### Workarounds

1. **Test empirically**: Write test hooks that log stdin/stderr/env
2. **Monitor HTTP traffic**: Some tools may use local HTTP servers
3. **Check source code**: Some tools are open-source (Aider, others)
4. **Contact vendors**: Request documentation for undocumented tools

## Recommendations for rew

### Immediate (Current Implementation)

✅ Claude Code and Cursor are well-supported and working.

### Short-term (Next 3-6 months)

1. **Test GitHub Copilot integration** - May be good backup option
2. **Document actual Copilot data format** - Currently unclear
3. **Add configuration wizard** - Help users discover installed tools
4. **Add hook testing commands** - `rew hook test` to validate setup

### Medium-term (6-12 months)

1. **Support Windsurf** - Once documentation is available
2. **Standardize hook format** - Propose unified schema
3. **Hook marketplace** - Share hooks across projects
4. **Remote hooks** - Support webhooks, not just local commands

### Long-term (12+ months)

1. **Universal hook adapter** - Abstract away tool differences
2. **Hook orchestration** - Run multiple tools together
3. **Hook composition** - Chain hooks like Unix pipes
4. **Industry standards** - Work with vendors on hook standards

## Sources

- [Claude Code Documentation](https://claude.com/claude-code) (implied)
- [Cursor IDE Documentation](https://cursor.com/docs)
- [GitHub Copilot Agents - Hooks Documentation](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-hooks)
- [Aider GitHub Repository - Issue #802](https://github.com/Aider-AI/aider/issues/802)
- [CLI Coding Assistant Hooks: The Overlooked Gold Rush](https://zircote.github.io/blog/2026/01/cli-coding-assistant-hooks-the-gold-rush-no-one-seems-to-be-chasing/)
- [augment-aider - Aider Fork with Hook Support](https://github.com/augmentedcode/augment-aider)

## Appendix: Hook Testing Script

To test a new tool's hook capabilities:

```bash
#!/bin/bash
# test_hooks.sh - Test hook implementation for unknown tool

TOOL_NAME=$1
CONFIG_DIR=$2
HOOK_PATH=$3

echo "Testing hooks for: $TOOL_NAME"
echo "Config dir: $CONFIG_DIR"
echo "Hook path: $HOOK_PATH"

# 1. Check config file exists
if [ ! -f "$CONFIG_DIR" ]; then
    echo "❌ Config file not found: $CONFIG_DIR"
    exit 1
fi

echo "✓ Config file found"

# 2. Parse hook configuration
echo "Hooks found:"
cat "$CONFIG_DIR" | jq '.hooks' 2>/dev/null || echo "⚠ Could not parse hooks"

# 3. Test each hook type
for hook_event in "prompt" "pre" "post" "stop"; do
    echo ""
    echo "Testing hook: $hook_event"
    
    # Test with stdin
    echo "test data" | timeout 5 "$HOOK_PATH" hook "$hook_event"
    EXIT_CODE=$?
    
    echo "Exit code: $EXIT_CODE"
    echo "Env vars: $(env | grep -i $TOOL_NAME | head -3)"
done
```

Usage:
```bash
chmod +x test_hooks.sh
./test_hooks.sh "MyNewTool" ~/.config/mynewt tool/config.json ~/.mynewt/bin/hook
```
