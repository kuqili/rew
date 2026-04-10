//! Hook CLI subcommands for AI tool integration.
//!
//! These commands are called by AI tool hooks (Claude Code, Cursor, etc.)
//! to integrate with rew's protection system.
//!
//! Commands:
//! - `rew hook prompt`    — Record user prompt, create task (sync, <2ms)
//! - `rew hook pre-tool`  — Scope check + backup before AI writes (sync, <3ms)
//! - `rew hook post-tool` — Record change after AI operation (async)
//! - `rew hook stop`      — Close current task (async)
//!
//! Supports multiple AI tool formats:
//! - Claude Code: nested `tool_input`/`tool_response` with `session_id`, `cwd`
//! - Cursor: similar but different field names
//! - Copilot: `toolName`/`toolArgs` format
//! - Generic: flat `tool_name`/`file_path` (legacy rew format)

use rew_core::backup::clonefile;
use rew_core::db::Database;
use rew_core::error::RewResult;
use rew_core::objects::{sha256_file, ObjectStore};
use rew_core::scope::{ScopeEngine, ScopeResult};
use rew_core::types::{Change, ChangeType, Task, TaskStatus};
use rew_core::rew_home_dir;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

// ================================================================
// Normalized input types (tool-agnostic)
// ================================================================

/// Normalized prompt input extracted from any AI tool's JSON.
struct NormalizedPrompt {
    prompt: String,
    session_id: Option<String>,
    cwd: Option<String>,
}

/// Normalized pre-tool input extracted from any AI tool's JSON.
struct NormalizedPreTool {
    tool_name: String,
    file_path: Option<String>,
    command: Option<String>,
    cwd: Option<String>,
}

/// Normalized post-tool input extracted from any AI tool's JSON.
struct NormalizedPostTool {
    tool_name: String,
    file_path: Option<String>,
    success: Option<bool>,
    cwd: Option<String>,
}

// ================================================================
// Raw deserialization structs (Claude Code format)
// ================================================================

/// Claude Code stdin envelope — common fields present in all hook events.
#[derive(Debug, Deserialize, Default)]
struct ClaudeCodeInput {
    session_id: Option<String>,
    cwd: Option<String>,
    transcript_path: Option<String>,
    hook_event_name: Option<String>,

    // UserPromptSubmit
    prompt: Option<String>,

    // PreToolUse / PostToolUse
    tool_name: Option<String>,
    tool_input: Option<serde_json::Value>,
    tool_response: Option<serde_json::Value>,

    // Stop
    stop_hook_active: Option<bool>,
}

/// Legacy rew format for backward compatibility.
#[derive(Debug, Deserialize)]
struct LegacyPreToolInput {
    tool_name: String,
    file_path: Option<String>,
    command: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyPostToolInput {
    tool_name: String,
    file_path: Option<String>,
    success: Option<bool>,
}

// ================================================================
// Normalization: convert any AI tool format → rew internal types
// ================================================================

fn normalize_prompt(raw: &str) -> NormalizedPrompt {
    // Try JSON first (Claude Code, Copilot, Windsurf)
    if let Ok(input) = serde_json::from_str::<ClaudeCodeInput>(raw) {
        return NormalizedPrompt {
            prompt: input.prompt.unwrap_or_default(),
            session_id: input.session_id,
            cwd: input.cwd,
        };
    }

    // Fallback: plain text (legacy or unknown tools)
    NormalizedPrompt {
        prompt: raw.to_string(),
        session_id: None,
        cwd: None,
    }
}

fn normalize_pre_tool(raw: &str) -> Option<NormalizedPreTool> {
    // Try Claude Code format first (nested tool_input)
    if let Ok(input) = serde_json::from_str::<ClaudeCodeInput>(raw) {
        if let Some(tool_name) = input.tool_name {
            let (file_path, command) = extract_from_tool_input(&input.tool_input);
            return Some(NormalizedPreTool {
                tool_name,
                file_path,
                command,
                cwd: input.cwd,
            });
        }
    }

    // Try legacy flat format
    if let Ok(input) = serde_json::from_str::<LegacyPreToolInput>(raw) {
        return Some(NormalizedPreTool {
            tool_name: input.tool_name,
            file_path: input.file_path,
            command: input.command,
            cwd: None,
        });
    }

    None // Unparseable — fail open
}

fn normalize_post_tool(raw: &str) -> Option<NormalizedPostTool> {
    // Try Claude Code format first
    if let Ok(input) = serde_json::from_str::<ClaudeCodeInput>(raw) {
        if let Some(tool_name) = input.tool_name {
            let (file_path, _) = extract_from_tool_input(&input.tool_input);
            let success = input.tool_response.as_ref().and_then(|r| {
                r.get("success").and_then(|v| v.as_bool())
            });
            return Some(NormalizedPostTool {
                tool_name,
                file_path,
                success,
                cwd: input.cwd,
            });
        }
    }

    // Try legacy flat format
    if let Ok(input) = serde_json::from_str::<LegacyPostToolInput>(raw) {
        return Some(NormalizedPostTool {
            tool_name: input.tool_name,
            file_path: input.file_path,
            success: input.success,
            cwd: None,
        });
    }

    None
}

/// Extract file_path and command from Claude Code's nested `tool_input` object.
///
/// Claude Code passes tool arguments like:
/// - Write/Edit: `{ "file_path": "/path/to/file", "content": "..." }`
/// - Bash: `{ "command": "ls -la" }`
fn extract_from_tool_input(tool_input: &Option<serde_json::Value>) -> (Option<String>, Option<String>) {
    match tool_input {
        Some(obj) => {
            let file_path = obj.get("file_path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let command = obj.get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (file_path, command)
        }
        None => (None, None),
    }
}

// ================================================================
// Hook handlers
// ================================================================

/// Handle `rew hook prompt` — called when user submits a prompt.
///
/// Reads prompt from stdin (JSON or plain text), creates a new Task record.
/// Exit code: always 0 (prompt recording should never block AI).
pub fn handle_prompt() -> RewResult<()> {
    let raw = read_stdin_text();
    let input = normalize_prompt(&raw);

    let rew_dir = rew_home_dir();
    std::fs::create_dir_all(&rew_dir)?;

    let db = open_db()?;

    let task_id = generate_task_id();
    let task = Task {
        id: task_id.clone(),
        prompt: if input.prompt.is_empty() { None } else { Some(input.prompt) },
        tool: Some("claude-code".to_string()),
        started_at: Utc::now(),
        completed_at: None,
        status: TaskStatus::Active,
        risk_level: None,
        summary: None,
        cwd: input.cwd,
    };

    db.create_task(&task)?;

    // Write session-scoped marker so multiple concurrent AI sessions
    // (Claude Code + Cursor + Codebuddy etc.) each track their own task.
    //
    // Layout:  ~/.rew/sessions/{session_id}  →  task_id (plain text)
    //
    // Falls back to a stable synthetic session ID derived from the tool
    // name when the caller doesn't provide one, so every prompt always
    // registers a session file — even for tools without session tracking.
    let sessions_dir = rew_dir.join("sessions");
    let _ = std::fs::create_dir_all(&sessions_dir);

    let effective_sid = input.session_id.clone()
        .unwrap_or_else(|| "default_session".to_string());

    let _ = std::fs::write(sessions_dir.join(&effective_sid), &task_id);

    // Keep legacy .current_task for backward-compat (tools/hooks that read it directly).
    let marker = rew_dir.join(".current_task");
    std::fs::write(&marker, &task_id)?;

    // Keep .current_session for existing code that reads it.
    if let Some(ref sid) = input.session_id {
        let sid_marker = rew_dir.join(".current_session");
        let _ = std::fs::write(&sid_marker, sid);
    }

    Ok(())
}

/// Handle `rew hook pre-tool` — called before AI executes a tool.
///
/// Reads JSON from stdin, normalizes from any AI tool format.
/// 1. Checks .rewscope for deny rules → exit 2 if blocked
/// 2. For Write/Edit: clonefile backup of target → exit 0
/// 3. For Bash: check command against scope → exit 0 or 2
///
/// Exit code:
/// - 0 = allow (continue AI operation)
/// - 2 = deny (block AI operation, stderr has reason)
pub fn handle_pre_tool() -> RewResult<i32> {
    let raw = read_stdin_text();
    let input = match normalize_pre_tool(&raw) {
        Some(v) => v,
        None => return Ok(0), // Can't parse → fail open
    };

    // Load scope engine
    let scope = load_scope_engine();

    // Check based on tool type
    match input.tool_name.as_str() {
        "Write" | "Edit" | "write" | "edit" | "MultiEdit" => {
            if let Some(ref path_str) = input.file_path {
                let path = canonicalize_path(path_str);

                // Skip temp files (e.g. Claude Code's .tmp.XXXXX staging files)
                if is_temp_file(&path) {
                    return Ok(0);
                }

                // Scope check
                match scope.check_path(&path) {
                    ScopeResult::Deny(reason) => {
                        eprintln!("rew: {}", reason);
                        return Ok(2);
                    }
                    ScopeResult::Alert(reason) => {
                        eprintln!("rew: warning: {}", reason);
                        // Continue — alert doesn't block
                    }
                    ScopeResult::Allow => {}
                }

                // Backup the file before AI modifies it
                if path.exists() {
                    if let Ok(hash) = backup_file_to_objects(&path) {
                        // Record the old_hash so post-tool can reference it
                        record_pre_tool_hash(&path, &hash);
                    }
                }
            }
            Ok(0)
        }
        "Bash" | "bash" => {
            if let Some(ref cmd) = input.command {
                match scope.check_command(cmd) {
                    ScopeResult::Deny(reason) => {
                        eprintln!("rew: {}", reason);
                        return Ok(2);
                    }
                    ScopeResult::Alert(reason) => {
                        eprintln!("rew: warning: {}", reason);
                    }
                    ScopeResult::Allow => {}
                }
            }
            Ok(0)
        }
        _ => Ok(0), // Unknown tool types: allow
    }
}

/// Handle `rew hook post-tool` — called after AI completes a tool operation.
///
/// Records the change in the database (async, doesn't block AI).
pub fn handle_post_tool() -> RewResult<()> {
    let raw = read_stdin_text();
    let input = match normalize_post_tool(&raw) {
        Some(v) => v,
        None => return Ok(()), // Can't parse → skip silently
    };

    let rew_dir = rew_home_dir();
    let task_id = read_current_task_id(&rew_dir);

    if let (Some(task_id), Some(ref path_str)) = (task_id, input.file_path) {
        let path = canonicalize_path(path_str);

        // Skip temp files (e.g. Claude Code's .tmp.XXXXX staging files)
        if is_temp_file(&path) {
            return Ok(());
        }
        let db = open_db()?;

        // Determine change type
        let change_type = if path.exists() {
            match input.tool_name.as_str() {
                "Write" | "write" => {
                    let prev = db.get_latest_change_for_file(&path)?;
                    if prev.is_some() {
                        ChangeType::Modified
                    } else {
                        ChangeType::Created
                    }
                }
                "Edit" | "edit" | "MultiEdit" => ChangeType::Modified,
                _ => ChangeType::Modified,
            }
        } else {
            ChangeType::Deleted
        };

        // Get old_hash from pre-tool record
        let old_hash = read_pre_tool_hash(&path);

        // Compute new hash if file exists
        let new_hash = if path.exists() {
            sha256_file(&path).ok()
        } else {
            None
        };

        // Store new content in objects if it exists
        if path.exists() {
            let rew_dir = rew_home_dir();
            let obj_store = ObjectStore::new(rew_dir.join("objects"))?;
            obj_store.store(&path).ok();
        }

        let change = Change {
            id: None,
            task_id,
            file_path: path,
            change_type,
            old_hash,
            new_hash,
            diff_text: None,
            lines_added: 0,
            lines_removed: 0,
            restored_at: None,
        };

        db.upsert_change(&change)?;
    }

    Ok(())
}

/// Handle `rew hook stop` — called when AI finishes responding.
///
/// Closes the current task, computes summary stats.
pub fn handle_stop() -> RewResult<()> {
    let rew_dir = rew_home_dir();
    let task_id = read_current_task_id(&rew_dir);

    // Read and discard stdin (Claude Code sends JSON, but we don't need it)
    let _ = read_stdin_text();

    if let Some(task_id) = task_id {
        let db = open_db()?;

        // Update task status
        db.update_task_status(&task_id, &TaskStatus::Completed, Some(Utc::now()))?;

        // Get changes for summary
        let changes = db.get_changes_for_task(&task_id)?;
        if !changes.is_empty() {
            let created = changes.iter().filter(|c| c.change_type == ChangeType::Created).count();
            let modified = changes.iter().filter(|c| c.change_type == ChangeType::Modified).count();
            let deleted = changes.iter().filter(|c| c.change_type == ChangeType::Deleted).count();

            let summary = format!(
                "{} files changed (+{} created, ~{} modified, -{} deleted)",
                changes.len(), created, modified, deleted
            );
            db.update_task_summary(&task_id, &summary)?;
        }

        // Write a suppress file so the daemon can ignore delayed FSEvents for
        // the paths that were just modified by this AI task.  Without this,
        // FSEvents delivered ≤ 90 s after `hook stop` would be mis-attributed
        // to a new monitoring window and show as duplicate entries.
        if !changes.is_empty() {
            let paths: Vec<String> = changes
                .iter()
                .map(|c| c.file_path.to_string_lossy().to_string())
                .collect();
            // expires_secs = now + 90 s — daemon ignores the file after this
            let expires_secs = Utc::now().timestamp() + 90;
            let json = format!(
                "{{\"expires_secs\":{},\"paths\":[{}]}}",
                expires_secs,
                paths
                    .iter()
                    .map(|p| format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let _ = std::fs::write(rew_dir.join(".stop_suppress"), json);
        }

        // Write a short-lived grace-period file so the daemon can still
        // attribute delayed FSEvents (e.g. from Bash tool file ops) to this
        // AI task for a few seconds after the session ends.
        // Grace window must be > pipeline accumulation window (3 s) + margin.
        // 10 s gives ~7 s of headroom while keeping mis-attribution risk low.
        let grace_expires = Utc::now().timestamp() + 10;
        let grace_json = format!(
            "{{\"task_id\":\"{}\",\"expires_secs\":{}}}",
            task_id, grace_expires
        );
        let _ = std::fs::write(rew_dir.join(".grace_task"), grace_json);

        // Clean up session file and legacy markers.
        // Remove the session-scoped file first so concurrent sessions
        // stay intact (only this session's entry is removed).
        if let Some(sid) = read_current_session_id(&rew_dir) {
            let _ = std::fs::remove_file(rew_dir.join("sessions").join(&sid));
            // Clean up session-scoped pre_tool dir
            let _ = std::fs::remove_dir_all(rew_dir.join(".pre_tool").join(&sid));
        }
        let _ = std::fs::remove_file(rew_dir.join(".current_task"));
        let _ = std::fs::remove_file(rew_dir.join(".current_session"));
    }

    Ok(())
}

// === Helper functions ===

/// Check if a path looks like a temporary file created by AI tools or editors.
///
/// Claude Code's Write tool creates `.tmp.XXXXX.XXXXX` staging files;
/// macOS apps create `.sb-XXXXXXXX-YYYYYY` for atomic writes;
/// editors create `.swp`, `~`, `.#*` files, etc.
/// These should be silently ignored by hooks.
fn is_temp_file(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };

    // Claude Code Write tool: "foo.rs.tmp.73919.177..."
    if name.contains(".tmp.") {
        return true;
    }

    // macOS safe-save (atomic write): "original.sb-XXXXXXXX-YYYYYY"
    // The ".sb-" marker can appear anywhere after the real filename.
    if name.contains(".sb-") {
        return true;
    }

    // Generic temp extensions
    if name.ends_with(".tmp") || name.ends_with(".temp") {
        return true;
    }

    // Emacs lock files: ".#symlink"
    if name.starts_with(".#") {
        return true;
    }

    // Editor swap / backup files
    if name.ends_with(".swp") || name.ends_with(".swo") || name.ends_with('~') {
        return true;
    }

    false
}

fn read_stdin_text() -> String {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).unwrap_or_default();
    buf.trim().to_string()
}

/// Resolve a path to absolute. Handles relative paths (./foo) and already-absolute paths.
fn canonicalize_path(path_str: &str) -> PathBuf {
    let path = PathBuf::from(path_str);
    if path.is_absolute() {
        path
    } else if let Ok(cwd) = std::env::current_dir() {
        // Join with cwd and normalize
        let joined = cwd.join(&path);
        // Try to canonicalize (resolve symlinks), fall back to joined
        joined.canonicalize().unwrap_or(joined)
    } else {
        path
    }
}

fn open_db() -> RewResult<Database> {
    let db_path = rew_home_dir().join("snapshots.db");
    let db = Database::open(&db_path)?;
    db.initialize()?;
    Ok(db)
}

fn generate_task_id() -> String {
    // Short nanoid-style ID: timestamp + random suffix
    let ts = Utc::now().format("%m%d%H%M").to_string();
    let rand: u32 = rand_u32() % 10000;
    format!("t{}_{:04}", ts, rand)
}

fn rand_u32() -> u32 {
    // Simple random using std (no extra dependency)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish() as u32
}

fn read_current_task_id(rew_dir: &Path) -> Option<String> {
    // Prefer session-scoped lookup: find the task_id for the current session.
    if let Some(sid) = read_current_session_id(rew_dir) {
        let session_file = rew_dir.join("sessions").join(&sid);
        if let Ok(tid) = std::fs::read_to_string(&session_file) {
            let tid = tid.trim().to_string();
            if !tid.is_empty() {
                return Some(tid);
            }
        }
    }
    // Legacy fallback: plain .current_task file.
    let marker = rew_dir.join(".current_task");
    std::fs::read_to_string(marker).ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_current_session_id(rew_dir: &Path) -> Option<String> {
    let marker = rew_dir.join(".current_session");
    std::fs::read_to_string(marker).ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn load_scope_engine() -> ScopeEngine {
    // Try to load .rewscope from current directory, then project root
    let cwd = std::env::current_dir().ok();

    if let Some(ref dir) = cwd {
        let scope_file = dir.join(".rewscope");
        if scope_file.exists() {
            if let Ok(engine) = ScopeEngine::from_file(&scope_file) {
                return engine;
            }
        }
    }

    // Fallback: default rules
    ScopeEngine::default_rules(cwd).unwrap_or_else(|_| {
        // Absolute fallback: no rules (allow all)
        ScopeEngine::from_config(
            rew_core::scope::RewScopeFile::default(),
            None,
        ).unwrap()
    })
}

fn backup_file_to_objects(path: &Path) -> RewResult<String> {
    let rew_dir = rew_home_dir();
    let obj_store = ObjectStore::new(rew_dir.join("objects"))?;
    obj_store.store(path)
}

fn path_key(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Record pre-tool old_hash for a file path (so post-tool can reference it).
///
/// Stored under `.pre_tool/{session_id}/{path_hash}` so that concurrent
/// sessions (Claude Code + Cursor etc.) don't overwrite each other's state.
/// Falls back to a flat `.pre_tool/{path_hash}` when no session is active.
fn record_pre_tool_hash(path: &Path, hash: &str) {
    let rew_dir = rew_home_dir();
    let session_id = read_current_session_id(&rew_dir);
    let pre_dir = match session_id {
        Some(ref sid) => rew_dir.join(".pre_tool").join(sid),
        None => rew_dir.join(".pre_tool"),
    };
    let _ = std::fs::create_dir_all(&pre_dir);
    let _ = std::fs::write(pre_dir.join(path_key(path)), hash);
}

/// Read pre-tool old_hash for a file path.
///
/// Tries the session-scoped directory first, then falls back to the flat
/// layout for backward compatibility with older records.
fn read_pre_tool_hash(path: &Path) -> Option<String> {
    let rew_dir = rew_home_dir();
    let key = path_key(path);

    // Try session-scoped first
    if let Some(sid) = read_current_session_id(&rew_dir) {
        let session_pre = rew_dir.join(".pre_tool").join(&sid).join(&key);
        if let Ok(h) = std::fs::read_to_string(&session_pre) {
            let _ = std::fs::remove_file(session_pre);
            return Some(h.trim().to_string());
        }
    }

    // Flat fallback (legacy)
    let marker = rew_dir.join(".pre_tool").join(&key);
    let hash = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(marker);
    Some(hash.trim().to_string())
}
