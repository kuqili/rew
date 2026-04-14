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
use rew_core::baseline::resolve_baseline;
use rew_core::db::Database;
use rew_core::error::RewResult;
use rew_core::file_index::sync_file_index_after_change;
use rew_core::objects::{sha256_file, ObjectStore};
use rew_core::scope::{ScopeEngine, ScopeResult};
use rew_core::types::{Change, ChangeType, Task, TaskStats, TaskStatus};
use rew_core::watcher::filter::PathFilter;
use rew_core::rew_home_dir;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

const HOOK_DEBUG_ENV: &str = "REW_HOOK_DEBUG";
const HOOK_DEBUG_MAX_BYTES: u64 = 5 * 1024 * 1024;

// ================================================================
// Normalized input types (tool-agnostic)
// ================================================================

/// Normalized prompt input extracted from any AI tool's JSON.
struct NormalizedPrompt {
    prompt: String,
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    conversation_id: Option<String>,
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
    command: Option<String>,
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

    // Cursor-specific fields
    model: Option<String>,
    conversation_id: Option<String>,
    generation_id: Option<String>,
    /// Per-tool-call duration in seconds (Cursor)
    duration: Option<f64>,
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
    command: Option<String>,
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
            model: input.model,
            conversation_id: input.conversation_id,
        };
    }

    // Fallback: plain text (legacy or unknown tools)
    NormalizedPrompt {
        prompt: raw.to_string(),
        session_id: None,
        cwd: None,
        model: None,
        conversation_id: None,
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
            let (file_path, command) = extract_from_tool_input(&input.tool_input);
            let success = input.tool_response.as_ref().and_then(|r| {
                r.get("success").and_then(|v| v.as_bool())
            });
            return Some(NormalizedPostTool {
                tool_name,
                file_path,
                command,
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
                    command: None,
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
            command: input.command,
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

/// Parse a shell command and predict which file paths it might write to.
///
/// Recognizes common patterns: redirects (>/>>/tee), cp/mv targets,
/// sed -i, touch, mkdir -p, git checkout --, etc.
fn predict_bash_file_paths(cmd: &str, cwd: Option<&str>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let tokens: Vec<&str> = cmd.split_whitespace().collect();

    let resolve = |p: &str| -> PathBuf {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            path
        } else if let Some(dir) = cwd {
            PathBuf::from(dir).join(p)
        } else {
            path
        }
    };

    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];

        // Redirect: cmd > file  or  cmd >> file
        if (t == ">" || t == ">>") && i + 1 < tokens.len() {
            paths.push(resolve(tokens[i + 1]));
            i += 2;
            continue;
        }
        // Inline redirect: >file or >>file
        if (t.starts_with(">>") || t.starts_with('>')) && t.len() > 2 {
            let file = t.trim_start_matches('>');
            if !file.is_empty() {
                paths.push(resolve(file));
            }
        }

        // cp/mv: last argument is the destination
        if (t == "cp" || t == "mv") && i + 2 < tokens.len() {
            let last = tokens[tokens.len() - 1];
            if !last.starts_with('-') {
                paths.push(resolve(last));
            }
            break;
        }

        // tee file
        if t == "tee" && i + 1 < tokens.len() {
            for arg in &tokens[i + 1..] {
                if !arg.starts_with('-') {
                    paths.push(resolve(arg));
                }
            }
            break;
        }

        // sed -i
        if t == "sed" {
            let has_inplace = tokens[i..].iter().any(|a| *a == "-i" || a.starts_with("-i'") || a.starts_with("-i\""));
            if has_inplace {
                if let Some(last) = tokens.last() {
                    if !last.starts_with('-') && !last.starts_with('s') {
                        paths.push(resolve(last));
                    }
                }
            }
            break;
        }

        // touch / mkdir
        if t == "touch" {
            for arg in &tokens[i + 1..] {
                if !arg.starts_with('-') {
                    paths.push(resolve(arg));
                }
            }
            break;
        }

        // git checkout -- file
        if t == "git" && tokens.get(i + 1) == Some(&"checkout") {
            if let Some(dash_pos) = tokens[i..].iter().position(|a| *a == "--") {
                for arg in &tokens[i + dash_pos + 1..] {
                    paths.push(resolve(arg));
                }
            }
            break;
        }

        i += 1;
    }

    paths
}

fn is_shell_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Bash" | "bash" | "Shell" | "shell" | "execute_command")
}

fn is_write_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Write" | "write" | "write_to_file")
}

fn is_edit_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Edit" | "edit" | "MultiEdit" | "replace_in_file")
}

fn infer_path_change_type(
    tool_name: &str,
    path_exists: bool,
    baseline_existed: bool,
) -> ChangeType {
    if !path_exists {
        return ChangeType::Deleted;
    }

    if is_edit_tool(tool_name) {
        ChangeType::Modified
    } else if baseline_existed {
        ChangeType::Modified
    } else {
        ChangeType::Created
    }
}

// ================================================================
// Hook handlers
// ================================================================

/// Extract the session key for a given AI tool. Each tool has its own logic,
/// no fallback chains — new tools add a new match branch.
fn extract_session_key(raw: &str, tool_source: &str) -> String {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
        match tool_source {
            "claude-code" => {
                if let Some(sid) = val.get("session_id").and_then(|v| v.as_str()) {
                    return format!("claude-code:{}", sid);
                }
            }
            "cursor" => {
                if let Some(cid) = val.get("conversation_id").and_then(|v| v.as_str()) {
                    return format!("cursor:{}", cid);
                }
            }
            "codebuddy" => {
                if let Some(sid) = val.get("session_id").and_then(|v| v.as_str()) {
                    return format!("codebuddy:{}", sid);
                }
            }
            "workbuddy" => {
                if let Some(sid) = val.get("session_id").and_then(|v| v.as_str()) {
                    return format!("workbuddy:{}", sid);
                }
            }
            _ => {}
        }
    }
    format!("{}_default", tool_source)
}

/// Handle `rew hook prompt` — called when user submits a prompt.
///
/// Reads prompt from stdin (JSON or plain text), creates a new Task record.
/// All state is stored in DB (no temp files).
/// Exit code: always 0 (prompt recording should never block AI).
pub fn handle_prompt(source: Option<&str>) -> RewResult<()> {
    let raw = read_stdin_text();
    append_hook_debug_log("prompt", source, &raw);

    let input = normalize_prompt(&raw);
    let generation_id: Option<String> = serde_json::from_str::<ClaudeCodeInput>(&raw)
        .ok()
        .and_then(|v| v.generation_id);
    let tool_source = resolve_tool_source(source, &raw);

    let rew_dir = rew_home_dir();
    std::fs::create_dir_all(&rew_dir)?;

    let db = open_db()?;

    let effective_sid = extract_session_key(&raw, &tool_source);

    // Close any orphan active task left by a previous session
    if let Ok(Some(old_tid)) = db.get_active_task_for_session(&effective_sid) {
        let _ = db.update_task_status(&old_tid, &TaskStatus::Completed, Some(Utc::now()));
        let _ = db.deactivate_session(&effective_sid);
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

    let stats = TaskStats {
        task_id: task_id.clone(),
        model: input.model.clone(),
        duration_secs: None,
        tool_calls: 0,
        files_changed: 0,
        input_tokens: None,
        output_tokens: None,
        total_cost_usd: None,
        session_id: Some(effective_sid.clone()),
        conversation_id: input.conversation_id.clone(),
        extra_json: None,
    };
    let _ = db.create_task_stats(&stats);

    // Register session -> task mapping in DB (replaces session files)
    let _ = db.insert_active_session(&effective_sid, &task_id, &tool_source, Utc::now());

    // Store generation_id for Cursor stop-hook dedup (replaces .genid file)
    if let Some(ref gid) = generation_id {
        let _ = db.set_stop_guard(&effective_sid, gid);
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
pub fn handle_pre_tool(source: Option<&str>) -> RewResult<i32> {
    let raw = read_stdin_text();
    let input = match normalize_pre_tool(&raw) {
        Some(v) => v,
        None => return Ok(0),
    };

    let scope = load_scope_engine_for_path(
        input.file_path.as_deref(),
        input.cwd.as_deref(),
    );

    match input.tool_name.as_str() {
        t if is_write_tool(t) || is_edit_tool(t) => {
            if let Some(ref path_str) = input.file_path {
                let path = canonicalize_path(path_str);

                if is_temp_file(&path) || should_ignore_path(&path) {
                    return Ok(0);
                }

                match scope.check_path(&path) {
                    ScopeResult::Deny(reason) => {
                        eprintln!("rew: {}", reason);
                        return Ok(2);
                    }
                    ScopeResult::Alert(reason) => {
                        eprintln!("rew: warning: {}", reason);
                    }
                    ScopeResult::Allow => {}
                }

                if path.exists() {
                    if let Ok(hash) = backup_file_to_objects(&path) {
                        let tool_source = resolve_tool_source(source, &raw);
                        let session_key = extract_session_key(&raw, &tool_source);
                        let path_str = path.to_string_lossy().to_string();
                        if let Ok(db) = open_db() {
                            let _ = db.set_pre_tool_hash(&session_key, &path_str, &hash);
                        }
                    }
                }
            }
            Ok(0)
        }
        _ if is_shell_tool(&input.tool_name) => {
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
        _ => Ok(0),
    }
}

/// Handle `rew hook post-tool` — called after AI completes a tool operation.
///
/// Records the change in the database. All state lookups via DB.
pub fn handle_post_tool(source: Option<&str>) -> RewResult<()> {
    let raw = read_stdin_text();
    append_hook_debug_log("post-tool", source, &raw);

    let input = match normalize_post_tool(&raw) {
        Some(v) => v,
        None => return Ok(()),
    };

    let tool_source = resolve_tool_source(source, &raw);
    let session_key = extract_session_key(&raw, &tool_source);
    let db = open_db()?;
    let task_id = db.get_active_task_for_session(&session_key)?;

    if let Some(ref tid) = task_id {
        let _ = db.increment_tool_calls(tid);
    }

    if let (Some(task_id), Some(path_str)) = (task_id.as_ref(), input.file_path.as_ref()) {
        let path = canonicalize_path(path_str);

        if is_temp_file(&path) || should_ignore_path(&path) {
            return Ok(());
        }

        let baseline = resolve_baseline(&db, task_id, &path, Some(&session_key));

        let change_type = infer_path_change_type(
            &input.tool_name,
            path.exists(),
            baseline.existed,
        );

        let new_hash = if path.exists() {
            sha256_file(&path).ok()
        } else {
            None
        };

        let rew_dir = rew_home_dir();
        let obj_store = ObjectStore::new(rew_dir.join("objects"))?;
        if path.exists() {
            obj_store.store(&path).ok();
        }

        let old_hash = if change_type == ChangeType::Created {
            None
        } else {
            baseline.hash
        };

        let (lines_added, lines_removed) = {
            let read = |h: Option<&str>| -> Vec<u8> {
                h.and_then(|hash| obj_store.retrieve(hash))
                    .and_then(|p| std::fs::read(p).ok())
                    .unwrap_or_default()
            };
            let old_bytes = read(old_hash.as_deref());
            let new_bytes = read(new_hash.as_deref());
            rew_core::diff::count_changed_lines(&old_bytes, &new_bytes)
        };

        let change = Change {
            id: None,
            task_id: task_id.clone(),
            file_path: path,
            change_type,
            old_hash,
            new_hash,
            diff_text: None,
            lines_added,
            lines_removed,
            attribution: Some("hook".into()),
            old_file_path: None,
        };

        db.upsert_change(&change)?;
        let _ = sync_file_index_after_change(&db, &change, "hook", &Utc::now().to_rfc3339());
    } else if let (Some(task_id), Some(cmd)) = (task_id.as_ref(), input.command.as_ref()) {
        if is_shell_tool(&input.tool_name) {
            let predicted = predict_bash_file_paths(cmd, input.cwd.as_deref());
            if !predicted.is_empty() {
                let rew_dir = rew_home_dir();
                let obj_store = ObjectStore::new(rew_dir.join("objects"))?;

                for path in predicted {
                    if !path.exists() || is_temp_file(&path) || should_ignore_path(&path) {
                        continue;
                    }
                    let new_hash = sha256_file(&path).ok();
                    if path.exists() {
                        obj_store.store(&path).ok();
                    }
                    let bash_baseline = resolve_baseline(&db, task_id, &path, Some(&session_key));
                    let old_hash = bash_baseline.hash.clone();

                    let change_type =
                        infer_path_change_type(&input.tool_name, path.exists(), bash_baseline.existed);

                    let (lines_added, lines_removed) = {
                        let read = |h: Option<&str>| -> Vec<u8> {
                            h.and_then(|hash| obj_store.retrieve(hash))
                                .and_then(|p| std::fs::read(p).ok())
                                .unwrap_or_default()
                        };
                        let old_bytes = read(old_hash.as_deref());
                        let new_bytes = read(new_hash.as_deref());
                        rew_core::diff::count_changed_lines(&old_bytes, &new_bytes)
                    };

                    let change = Change {
                        id: None,
                        task_id: task_id.clone(),
                        file_path: path,
                        change_type,
                        old_hash,
                        new_hash,
                        diff_text: None,
                        lines_added,
                        lines_removed,
                        attribution: Some("bash_predicted".into()),
                        old_file_path: None,
                    };
                    if db.upsert_change(&change).is_ok() {
                        let _ = sync_file_index_after_change(
                            &db,
                            &change,
                            "bash_predicted",
                            &Utc::now().to_rfc3339(),
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle `rew hook stop` — called when AI finishes responding.
///
/// All state is looked up and cleaned via DB. No temp file operations.
pub fn handle_stop(source: Option<&str>) -> RewResult<()> {
    let raw = read_stdin_text();

    let stop_input = serde_json::from_str::<ClaudeCodeInput>(&raw).unwrap_or_default();
    let stop_generation_id = stop_input.generation_id;

    let tool_source = resolve_tool_source(source, &raw);
    let session_key = extract_session_key(&raw, &tool_source);

    let db = open_db()?;
    let task_id = match db.get_active_task_for_session(&session_key)? {
        Some(tid) => tid,
        None => return Ok(()),
    };

    // generation_id guard (Cursor): compare stop's generation_id with stored one
    if let Some(ref stop_gid) = stop_generation_id {
        if let Ok(Some(stored_gid)) = db.get_stop_guard(&session_key) {
            if stored_gid != *stop_gid {
                return Ok(());
            }
        }
    } else {
        // No generation_id (Claude Code etc.): time-based guard
        if let Ok(Some(t)) = db.get_task(&task_id) {
            let age_ms = (Utc::now() - t.started_at).num_milliseconds();
            if age_ms < 3_000 {
                return Ok(());
            }
        }
    }

    db.update_task_status(&task_id, &TaskStatus::Completed, Some(Utc::now()))?;

    let objects_root = rew_home_dir().join("objects");
    let _ = rew_core::reconcile::reconcile_task(&db, &task_id, &objects_root);

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

    if let Ok(Some(task)) = db.get_task(&task_id) {
        let duration_secs = (Utc::now() - task.started_at).num_milliseconds() as f64 / 1000.0;
        let _ = db.finalize_task_stats(&task_id, duration_secs, changes.len() as i32);
    }

    // Deactivate session + cleanup (replaces 6+ file delete operations)
    let _ = db.deactivate_sessions_for_task(&task_id);
    let _ = db.delete_stop_guard(&session_key);
    let _ = db.delete_pre_tool_hashes_for_session(&session_key);

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
    let rew_dir = rew_home_dir();
    let _ = std::fs::remove_file(rew_dir.join(".scan_manifest.json"));
    let db_path = rew_dir.join("snapshots.db");
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

fn append_hook_debug_log(kind: &str, source: Option<&str>, raw: &str) {
    if !hook_debug_enabled() {
        return;
    }

    let debug_log = rew_home_dir().join("hook_debug.log");
    prune_hook_debug_log(&debug_log);

    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&debug_log)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "[{} {}] source={:?} raw={}", kind, Utc::now(), source, raw)
        });
}

fn hook_debug_enabled() -> bool {
    matches!(
        std::env::var(HOOK_DEBUG_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

fn prune_hook_debug_log(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    if meta.len() <= HOOK_DEBUG_MAX_BYTES {
        return;
    }
    let _ = std::fs::remove_file(path);
}

/// Determine the AI tool source. Priority:
/// 1. Explicit --source flag (most reliable)
/// 2. Conservative auto-detect for tools with distinctive payload shape
/// 3. Fallback to "ai-tool" instead of guessing from generic `session_id`
fn resolve_tool_source(explicit: Option<&str>, raw_input: &str) -> String {
    if let Some(s) = explicit {
        return s.to_string();
    }

    // Auto-detect from JSON structure
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw_input) {
        // Cursor exposes hook event names and conversation ids.
        if val.get("hook_event_name").is_some() || val.get("conversation_id").is_some() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_aliases_are_recognized() {
        assert!(is_shell_tool("Bash"));
        assert!(is_shell_tool("bash"));
        assert!(is_shell_tool("Shell"));
        assert!(is_shell_tool("shell"));
        assert!(is_shell_tool("execute_command"));
        assert!(!is_shell_tool("Write"));
        assert!(!is_shell_tool("Edit"));
    }

    #[test]
    fn write_aliases_are_recognized() {
        assert!(is_write_tool("Write"));
        assert!(is_write_tool("write"));
        assert!(is_write_tool("write_to_file"));
        assert!(!is_write_tool("Edit"));
        assert!(!is_write_tool("Bash"));
    }

    #[test]
    fn edit_aliases_are_recognized() {
        assert!(is_edit_tool("Edit"));
        assert!(is_edit_tool("edit"));
        assert!(is_edit_tool("MultiEdit"));
        assert!(is_edit_tool("replace_in_file"));
        assert!(!is_edit_tool("Write"));
        assert!(!is_edit_tool("Bash"));
    }

    #[test]
    fn shell_new_file_is_created() {
        assert_eq!(
            infer_path_change_type("Shell", true, false),
            ChangeType::Created
        );
        assert_eq!(
            infer_path_change_type("shell", true, false),
            ChangeType::Created
        );
    }

    #[test]
    fn shell_existing_file_is_modified() {
        assert_eq!(
            infer_path_change_type("Shell", true, true),
            ChangeType::Modified
        );
        assert_eq!(
            infer_path_change_type("bash", true, true),
            ChangeType::Modified
        );
    }

    #[test]
    fn edit_tools_remain_modified() {
        assert_eq!(
            infer_path_change_type("Edit", true, false),
            ChangeType::Modified
        );
        assert_eq!(
            infer_path_change_type("MultiEdit", true, false),
            ChangeType::Modified
        );
        assert_eq!(
            infer_path_change_type("replace_in_file", true, false),
            ChangeType::Modified
        );
    }

    #[test]
    fn ide_style_tool_names_work() {
        // write_to_file — new file
        assert_eq!(
            infer_path_change_type("write_to_file", true, false),
            ChangeType::Created
        );
        // write_to_file — existing file
        assert_eq!(
            infer_path_change_type("write_to_file", true, true),
            ChangeType::Modified
        );
        // execute_command — new file
        assert_eq!(
            infer_path_change_type("execute_command", true, false),
            ChangeType::Created
        );
        // execute_command — existing file
        assert_eq!(
            infer_path_change_type("execute_command", true, true),
            ChangeType::Modified
        );
    }

    #[test]
    fn predicted_shell_paths_resolve_from_cwd() {
        let paths = predict_bash_file_paths("touch notes.txt", Some("/tmp/project"));
        assert_eq!(paths, vec![PathBuf::from("/tmp/project/notes.txt")]);
    }

    #[test]
    fn session_keys_have_tool_source_prefix() {
        let claude = r#"{"session_id":"abc-123","cwd":"/tmp"}"#;
        assert_eq!(
            extract_session_key(claude, "claude-code"),
            "claude-code:abc-123"
        );

        let cursor = r#"{"conversation_id":"conv-456"}"#;
        assert_eq!(
            extract_session_key(cursor, "cursor"),
            "cursor:conv-456"
        );

        let codebuddy = r#"{"session_id":"cb-789","cwd":"/project"}"#;
        assert_eq!(
            extract_session_key(codebuddy, "codebuddy"),
            "codebuddy:cb-789"
        );

        let workbuddy = r#"{"session_id":"wb-012","cwd":"/project"}"#;
        assert_eq!(
            extract_session_key(workbuddy, "workbuddy"),
            "workbuddy:wb-012"
        );
    }

    #[test]
    fn explicit_source_wins_over_session_shaped_payload() {
        let workbuddy_like = r#"{"session_id":"wb-100","tool_name":"write_to_file"}"#;
        let codebuddy_like = r#"{"session_id":"cb-200","tool_name":"execute_command"}"#;

        assert_eq!(
            resolve_tool_source(Some("workbuddy"), workbuddy_like),
            "workbuddy"
        );
        assert_eq!(
            resolve_tool_source(Some("codebuddy"), codebuddy_like),
            "codebuddy"
        );
    }

    #[test]
    fn generic_session_payload_without_explicit_source_does_not_guess_claude() {
        let generic = r#"{"session_id":"shared-123","tool_name":"write_to_file"}"#;
        assert_eq!(resolve_tool_source(None, generic), "ai-tool");
    }

    #[test]
    fn cursor_payload_is_still_auto_detected_without_explicit_source() {
        let cursor = r#"{"conversation_id":"conv-456","hook_event_name":"afterFileEdit"}"#;
        assert_eq!(resolve_tool_source(None, cursor), "cursor");
    }

    #[test]
    fn ide_style_post_tool_payload_is_normalized() {
        let raw = r#"{
            "session_id":"wb-300",
            "cwd":"/tmp/project",
            "tool_name":"write_to_file",
            "tool_input":{"file_path":"notes.md"},
            "tool_response":{"success":true}
        }"#;

        let normalized = normalize_post_tool(raw).expect("should parse ide-style post tool");
        assert_eq!(normalized.tool_name, "write_to_file");
        assert_eq!(normalized.file_path.as_deref(), Some("notes.md"));
        assert_eq!(normalized.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(normalized.success, Some(true));
    }
}

