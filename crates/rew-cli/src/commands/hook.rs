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
use rew_core::watcher::filter::PathFilter;
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
    session_id: Option<String>,
}

/// Normalized post-tool input extracted from any AI tool's JSON.
struct NormalizedPostTool {
    tool_name: String,
    file_path: Option<String>,
    success: Option<bool>,
    cwd: Option<String>,
    session_id: Option<String>,
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
                session_id: input.session_id,
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
            session_id: None,
        });
    }

    None // Unparseable — fail open
}

fn normalize_post_tool(raw: &str) -> Option<NormalizedPostTool> {
    // Try Claude Code / Cursor postToolUse format (has tool_name + tool_input)
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
                session_id: input.session_id,
            });
        }
    }

    // Cursor afterFileEdit format: has file_path at top level but no tool_name
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
        if val.get("hook_event_name").and_then(|v| v.as_str()) == Some("afterFileEdit") {
            if let Some(fp) = val.get("file_path").and_then(|v| v.as_str()) {
                return Some(NormalizedPostTool {
                    tool_name: "Write".to_string(),
                    file_path: Some(fp.to_string()),
                    success: Some(true),
                    cwd: val.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    session_id: val.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                });
            }
        }
    }

    // Try legacy flat format
    if let Ok(input) = serde_json::from_str::<LegacyPostToolInput>(raw) {
        return Some(NormalizedPostTool {
            tool_name: input.tool_name,
            file_path: input.file_path,
            success: input.success,
            cwd: None,
            session_id: None,
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
pub fn handle_prompt(source: Option<&str>) -> RewResult<()> {
    let raw = read_stdin_text();

    let debug_log = rew_home_dir().join("hook_debug.log");
    let _ = std::fs::OpenOptions::new()
        .create(true).append(true).open(&debug_log)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "[prompt {}] source={:?} raw={}", Utc::now(), source, &raw)
        });

    let input = normalize_prompt(&raw);
    let tool_source = resolve_tool_source(source, &raw);

    let rew_dir = rew_home_dir();
    std::fs::create_dir_all(&rew_dir)?;

    let db = open_db()?;

    let sessions_dir = rew_dir.join("sessions");
    let _ = std::fs::create_dir_all(&sessions_dir);

    let effective_sid = input.session_id.clone()
        .unwrap_or_else(|| format!("{}_default", tool_source));

    // Close any orphan active task left by a previous session that never
    // got a proper `hook stop` (e.g. user pressed Ctrl-C).
    if let Some(old_tid) = read_task_id_for_session(&rew_dir, &effective_sid) {
        let _ = db.update_task_status(&old_tid, &TaskStatus::Completed, Some(Utc::now()));
        // Clean up the per-task marker for the old task
        let _ = std::fs::remove_file(sessions_dir.join(format!(".task_{}", old_tid)));
    }

    let task_id = generate_task_id();
    let task = Task {
        id: task_id.clone(),
        prompt: if input.prompt.is_empty() { None } else { Some(input.prompt) },
        tool: Some(tool_source.clone()),
        started_at: Utc::now(),
        completed_at: None,
        status: TaskStatus::Active,
        risk_level: None,
        summary: None,
        cwd: input.cwd,
    };

    db.create_task(&task)?;

    let _ = std::fs::write(sessions_dir.join(&effective_sid), &task_id);

    // Write a per-task marker so `hook stop` (running in the same process
    // context) can look up the exact session file to remove, even when
    // multiple tools are running concurrently and `.current_session` has
    // been overwritten by another tool's prompt hook.
    // Layout: ~/.rew/sessions/.task_{task_id} → effective_sid
    let task_sid_file = sessions_dir.join(format!(".task_{}", task_id));
    let _ = std::fs::write(&task_sid_file, &effective_sid);

    // Keep legacy .current_task for backward-compat (tools/hooks that read it directly).
    let marker = rew_dir.join(".current_task");
    std::fs::write(&marker, &task_id)?;

    // Keep .current_session for existing code that reads it.
    // Note: this may be overwritten by the next concurrent prompt call, so
    // stop() now prefers the per-task marker above.
    let sid_marker = rew_dir.join(".current_session");
    let _ = std::fs::write(&sid_marker, &effective_sid);

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
pub fn handle_pre_tool(_source: Option<&str>) -> RewResult<i32> {
    let raw = read_stdin_text();
    let input = match normalize_pre_tool(&raw) {
        Some(v) => v,
        None => return Ok(0), // Can't parse → fail open
    };

    // Load scope engine — use file_path and cwd from input to find .rewscope
    let scope = load_scope_engine_for_path(
        input.file_path.as_deref(),
        input.cwd.as_deref(),
    );

    // Check based on tool type
    match input.tool_name.as_str() {
        "Write" | "Edit" | "write" | "edit" | "MultiEdit" => {
            if let Some(ref path_str) = input.file_path {
                let path = canonicalize_path(path_str);

                if is_temp_file(&path) || should_ignore_path(&path) {
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
                        record_pre_tool_hash(&path, &hash, input.session_id.as_deref());
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
pub fn handle_post_tool(_source: Option<&str>) -> RewResult<()> {
    let raw = read_stdin_text();

    // Debug: log raw input for diagnosing format issues
    let debug_log = rew_home_dir().join("hook_debug.log");
    let _ = std::fs::OpenOptions::new()
        .create(true).append(true).open(&debug_log)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "[post-tool {}] source={:?} raw={}", Utc::now(), _source, &raw)
        });

    let input = match normalize_post_tool(&raw) {
        Some(v) => v,
        None => return Ok(()), // Can't parse → skip silently
    };

    let rew_dir = rew_home_dir();
    let task_id = resolve_task_id(&rew_dir, input.session_id.as_deref());

    if let (Some(task_id), Some(ref path_str)) = (task_id, input.file_path) {
        let path = canonicalize_path(path_str);

        if is_temp_file(&path) || should_ignore_path(&path) {
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

        // Get old_hash from pre-tool record — use session_id to read from
        // the correct session-scoped directory.
        let old_hash = read_pre_tool_hash(&path, input.session_id.as_deref());

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
/// Parses stdin to extract `session_id`, then precisely closes the task
/// associated with that session.  Falls back to the global `.current_session`
/// when the payload doesn't contain a session_id (e.g. legacy tools).
pub fn handle_stop(_source: Option<&str>) -> RewResult<()> {
    let rew_dir = rew_home_dir();
    let raw = read_stdin_text();

    // Extract session_id from the stop payload (Claude Code always sends it).
    let stop_session_id: Option<String> = serde_json::from_str::<ClaudeCodeInput>(&raw)
        .ok()
        .and_then(|input| input.session_id);

    let task_id = resolve_task_id(&rew_dir, stop_session_id.as_deref());

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

        // Append to the grace-period list so multiple concurrent AI sessions
        // (Claude Code + Cursor + …) can each register their own grace window
        // without overwriting each other.
        // Layout: ~/.rew/.grace_tasks (JSON array of {task_id, expires_secs})
        // The daemon reads all unexpired entries; daemon-side read_grace_task_id
        // still exists for backward-compat but the array file takes precedence.
        // 15 s = 2 s FSEvents latency + 3 s pipeline window + 10 s headroom.
        let grace_expires = Utc::now().timestamp() + 15;
        let grace_file = rew_dir.join(".grace_tasks");
        let mut entries: Vec<serde_json::Value> = {
            std::fs::read_to_string(&grace_file)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        };
        // Purge already-expired entries before appending.
        let now_ts = Utc::now().timestamp();
        entries.retain(|e| e.get("expires_secs").and_then(|v| v.as_i64()).unwrap_or(0) > now_ts);
        entries.push(serde_json::json!({"task_id": task_id, "expires_secs": grace_expires}));
        if let Ok(json) = serde_json::to_string(&entries) {
            let tmp = grace_file.with_extension("tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &grace_file);
            }
        }

        // Clean up session file and legacy markers.
        // Prefer the per-task marker written by `hook prompt` so we remove
        // only THIS session's file even when concurrent tools have since
        // overwritten `.current_session`.
        let sessions_dir = rew_dir.join("sessions");
        let task_sid_file = sessions_dir.join(format!(".task_{}", task_id));
        let sid_to_remove = std::fs::read_to_string(&task_sid_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            // Fallback: legacy .current_session (may be wrong when concurrent)
            .or_else(|| read_current_session_id(&rew_dir));

        if let Some(sid) = sid_to_remove {
            let _ = std::fs::remove_file(sessions_dir.join(&sid));
            let _ = std::fs::remove_dir_all(rew_dir.join(".pre_tool").join(&sid));
        }
        // Always remove the per-task marker regardless.
        let _ = std::fs::remove_file(&task_sid_file);

        // Only remove legacy markers if no other sessions are still active.
        let remaining_sessions = std::fs::read_dir(&sessions_dir)
            .ok()
            .map(|rd| rd.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count())
            .unwrap_or(0);
        if remaining_sessions == 0 {
            let _ = std::fs::remove_file(rew_dir.join(".current_task"));
            let _ = std::fs::remove_file(rew_dir.join(".current_session"));
        }
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

    // Claude Code staging: "foo.rs.tmp.73919" (dot-separated)
    if name.contains(".tmp.") {
        return true;
    }

    // Atomic-write temps with hex suffix: "foo.json.tmp-5885abc"
    if let Some(idx) = name.find(".tmp-") {
        if idx > 0 {
            return true;
        }
    }

    // macOS safe-save (atomic write): "original.sb-XXXXXXXX-YYYYYY"
    if name.contains(".sb-") {
        return true;
    }

    // Generic temp extensions
    if name.ends_with(".tmp") || name.ends_with(".temp") {
        return true;
    }

    // Transient process-lock files (same heuristic as PathFilter::should_ignore)
    if name.ends_with(".LOCK") {
        return true;
    }
    if name.ends_with(".lock") {
        if name.starts_with('.') {
            return true;
        }
        let stem = &name[..name.len() - 5];
        if std::path::Path::new(stem).extension().is_some() {
            return true;
        }
        // Bare stem (Cargo.lock, yarn.lock) → keep
    }

    // SQLite WAL / journal
    if name.ends_with("-journal") || name.ends_with("-wal") || name.ends_with("-shm") {
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

/// Check if a path should be ignored by hooks, using the same PathFilter as
/// the daemon/scanner/watcher. Loads user config (ignore_patterns + dir_ignore)
/// from config.toml so that all exclusions apply to AI hook operations too.
fn should_ignore_path(path: &Path) -> bool {
    use rew_core::config::RewConfig;

    thread_local! {
        static CONFIG: Option<RewConfig> = {
            let config_path = rew_home_dir().join("config.toml");
            RewConfig::load(&config_path).ok()
        };
        static FILTER: PathFilter = {
            CONFIG.with(|cfg| {
                let patterns = cfg.as_ref()
                    .map(|c| c.ignore_patterns.clone())
                    .unwrap_or_default();
                PathFilter::new(&patterns).unwrap_or_default()
            })
        };
    }

    // Global pattern check
    if FILTER.with(|f| f.should_ignore(path)) {
        return true;
    }

    // Per-directory ignore check (exclude_dirs + exclude_extensions)
    CONFIG.with(|cfg| {
        if let Some(ref cfg) = cfg {
            for (dir_str, dir_cfg) in &cfg.dir_ignore {
                let dir_path = Path::new(dir_str);
                if path.starts_with(dir_path) {
                    if PathFilter::should_ignore_by_dir_config(path, dir_path, dir_cfg) {
                        return true;
                    }
                }
            }
        }
        false
    })
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

/// Look up the task_id for a specific session, bypassing .current_session.
fn read_task_id_for_session(rew_dir: &Path, session_id: &str) -> Option<String> {
    let session_file = rew_dir.join("sessions").join(session_id);
    std::fs::read_to_string(session_file).ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve task_id: prefer explicit session_id from stdin, fall back to
/// the global .current_session / .current_task chain.
fn resolve_task_id(rew_dir: &Path, session_id: Option<&str>) -> Option<String> {
    if let Some(sid) = session_id {
        if let Some(tid) = read_task_id_for_session(rew_dir, sid) {
            return Some(tid);
        }
    }
    read_current_task_id(rew_dir)
}

/// Determine the AI tool source. Priority:
/// 1. Explicit --source flag (most reliable)
/// 2. Auto-detect from stdin JSON structure
/// 3. Fallback to "ai-tool"
fn resolve_tool_source(explicit: Option<&str>, raw_input: &str) -> String {
    if let Some(s) = explicit {
        return s.to_string();
    }

    // Auto-detect from JSON structure
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw_input) {
        // Claude Code: has session_id and typically tool_input/tool_response
        if val.get("session_id").is_some() {
            return "claude-code".to_string();
        }
        // Cursor: has hook_event_name or specific cursor-style fields
        if val.get("hook_event_name").is_some() {
            return "cursor".to_string();
        }
        // Windsurf: has windsurf-specific fields
        if val.get("windsurf_session").is_some() {
            return "windsurf".to_string();
        }
    }

    "ai-tool".to_string()
}

fn load_scope_engine_for_path(file_path: Option<&str>, cwd_hint: Option<&str>) -> ScopeEngine {
    // Walk up from the target file path to find the nearest .rewscope.
    // This works regardless of the hook process's CWD (which may be ~/.cursor/
    // for user-level Cursor hooks).
    let search_roots: Vec<PathBuf> = [
        file_path.map(|p| PathBuf::from(p)),
        cwd_hint.map(|p| PathBuf::from(p)),
        std::env::current_dir().ok(),
    ]
    .into_iter()
    .flatten()
    .collect();

    for start in &search_roots {
        let mut dir = if start.is_file() || !start.exists() {
            start.parent().map(|p| p.to_path_buf())
        } else {
            Some(start.clone())
        };

        // Walk up at most 20 levels
        for _ in 0..20 {
            if let Some(ref d) = dir {
                let scope_file = d.join(".rewscope");
                if scope_file.exists() {
                    if let Ok(engine) = ScopeEngine::from_file(&scope_file) {
                        return engine;
                    }
                }
                dir = d.parent().map(|p| p.to_path_buf());
            } else {
                break;
            }
        }
    }

    // Fallback: default rules with best-guess project root
    let root = cwd_hint.map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    ScopeEngine::default_rules(root).unwrap_or_else(|_| {
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
fn record_pre_tool_hash(path: &Path, hash: &str, session_id: Option<&str>) {
    let rew_dir = rew_home_dir();
    let pre_dir = match session_id {
        Some(sid) => rew_dir.join(".pre_tool").join(sid),
        None => rew_dir.join(".pre_tool"),
    };
    let _ = std::fs::create_dir_all(&pre_dir);
    let _ = std::fs::write(pre_dir.join(path_key(path)), hash);
}

/// Read pre-tool old_hash for a file path.
///
/// Uses the explicit `session_id` to locate the correct session-scoped
/// directory, then falls back to the flat layout for backward compatibility.
fn read_pre_tool_hash(path: &Path, session_id: Option<&str>) -> Option<String> {
    let rew_dir = rew_home_dir();
    let key = path_key(path);

    // Try explicit session-scoped directory first
    if let Some(sid) = session_id {
        let session_pre = rew_dir.join(".pre_tool").join(sid).join(&key);
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
