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
//! Performance: hot path via Unix socket to daemon (<1ms),
//! cold path inline when daemon not running (<5ms).

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

/// Input for pre-tool hook (read from stdin as JSON).
#[derive(Debug, Deserialize)]
pub struct PreToolInput {
    /// Tool name: "Write", "Edit", "Bash", etc.
    pub tool_name: String,
    /// File path being operated on (for Write/Edit)
    pub file_path: Option<String>,
    /// Command being executed (for Bash)
    pub command: Option<String>,
}

/// Input for post-tool hook (read from stdin as JSON).
#[derive(Debug, Deserialize)]
pub struct PostToolInput {
    /// Tool name
    pub tool_name: String,
    /// File path that was operated on
    pub file_path: Option<String>,
    /// Whether the operation succeeded
    pub success: Option<bool>,
}

/// Output for pre-tool hook (written to stdout as JSON, if needed).
#[derive(Debug, Serialize)]
pub struct PreToolOutput {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Handle `rew hook prompt` — called when user submits a prompt.
///
/// Reads prompt text from stdin, creates a new Task record.
/// Exit code: always 0 (prompt recording should never block AI).
pub fn handle_prompt() -> RewResult<()> {
    let prompt = read_stdin_text();

    let rew_dir = rew_home_dir();
    std::fs::create_dir_all(&rew_dir)?;

    let db = open_db()?;

    let task_id = generate_task_id();
    let task = Task {
        id: task_id.clone(),
        prompt: if prompt.is_empty() { None } else { Some(prompt) },
        tool: None, // Will be set by post-tool events
        started_at: Utc::now(),
        completed_at: None,
        status: TaskStatus::Active,
        risk_level: None,
        summary: None,
    };

    db.create_task(&task)?;

    // Write current task ID to a marker file so pre-tool/post-tool know which task
    let marker = rew_dir.join(".current_task");
    std::fs::write(&marker, &task_id)?;

    Ok(())
}

/// Handle `rew hook pre-tool` — called before AI executes a tool.
///
/// Reads JSON from stdin with tool_name, file_path, command.
/// 1. Checks .rewscope for deny rules → exit 2 if blocked
/// 2. For Write/Edit: clonefile backup of target → exit 0
/// 3. For Bash: check command against scope → exit 0 or 2
///
/// Exit code:
/// - 0 = allow (continue AI operation)
/// - 2 = deny (block AI operation, stderr has reason)
pub fn handle_pre_tool() -> RewResult<i32> {
    let input_str = read_stdin_text();
    let input: PreToolInput = match serde_json::from_str(&input_str) {
        Ok(v) => v,
        Err(_) => {
            // If we can't parse input, allow by default (fail open)
            return Ok(0);
        }
    };

    // Load scope engine
    let scope = load_scope_engine();

    // Check based on tool type
    match input.tool_name.as_str() {
        "Write" | "Edit" | "write" | "edit" => {
            if let Some(ref path_str) = input.file_path {
                let path = canonicalize_path(path_str);

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
    let input_str = read_stdin_text();
    let input: PostToolInput = match serde_json::from_str(&input_str) {
        Ok(v) => v,
        Err(_) => return Ok(()), // Can't parse = skip silently
    };

    let rew_dir = rew_home_dir();
    let task_id = read_current_task_id(&rew_dir);

    if let (Some(task_id), Some(ref path_str)) = (task_id, input.file_path) {
        let path = canonicalize_path(path_str);
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
                "Edit" | "edit" => ChangeType::Modified,
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

        // Clean up marker
        let marker = rew_dir.join(".current_task");
        let _ = std::fs::remove_file(marker);
    }

    Ok(())
}

// === Helper functions ===

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
    let marker = rew_dir.join(".current_task");
    std::fs::read_to_string(marker).ok().map(|s| s.trim().to_string())
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

/// Record pre-tool old_hash for a file path (so post-tool can reference it).
fn record_pre_tool_hash(path: &Path, hash: &str) {
    let rew_dir = rew_home_dir();
    let pre_dir = rew_dir.join(".pre_tool");
    let _ = std::fs::create_dir_all(&pre_dir);

    // Use a hash of the path as the filename to avoid path separator issues
    let path_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        path.hash(&mut h);
        format!("{:016x}", h.finish())
    };

    let _ = std::fs::write(pre_dir.join(path_hash), hash);
}

/// Read pre-tool old_hash for a file path.
fn read_pre_tool_hash(path: &Path) -> Option<String> {
    let rew_dir = rew_home_dir();
    let pre_dir = rew_dir.join(".pre_tool");

    let path_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        path.hash(&mut h);
        format!("{:016x}", h.finish())
    };

    let marker = pre_dir.join(&path_hash);
    let hash = std::fs::read_to_string(&marker).ok()?;
    // Clean up after reading
    let _ = std::fs::remove_file(marker);
    Some(hash.trim().to_string())
}
