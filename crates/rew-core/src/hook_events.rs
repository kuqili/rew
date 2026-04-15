use crate::baseline::resolve_baseline;
use crate::db::Database;
use crate::error::RewResult;
use crate::file_index::sync_file_index_after_change;
use crate::objects::ObjectStore;
use crate::types::{Change, ChangeType, Task, TaskStats, TaskStatus};
use crate::rew_home_dir;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

const HOOK_SPOOL_DIR: &str = "hook-spool";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptStartedPayload {
    pub tool_source: String,
    pub session_key: String,
    pub prompt: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub conversation_id: Option<String>,
    pub generation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedPathChange {
    pub file_path: String,
    pub file_exists_after: bool,
    pub new_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolObservedPayload {
    pub tool_source: String,
    pub session_key: String,
    pub tool_name: String,
    pub cwd: Option<String>,
    pub observations: Vec<ObservedPathChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStopRequestedPayload {
    pub tool_source: String,
    pub session_key: String,
    pub generation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", content = "payload", rename_all = "kebab-case")]
pub enum HookEventPayload {
    PromptStarted(PromptStartedPayload),
    PostToolObserved(PostToolObservedPayload),
    TaskStopRequested(TaskStopRequestedPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventEnvelope {
    pub event_id: String,
    pub idempotency_key: String,
    pub created_at: String,
    pub payload_version: u32,
    #[serde(flatten)]
    pub payload: HookEventPayload,
}

#[derive(Debug, Clone)]
pub struct HookEventProcessOutcome {
    pub task_id: Option<String>,
}

fn hook_spool_state_dir(state: &str) -> PathBuf {
    rew_home_dir().join(HOOK_SPOOL_DIR).join(state)
}

pub fn hook_spool_pending_dir() -> PathBuf {
    hook_spool_state_dir("pending")
}

fn hook_spool_processing_dir() -> PathBuf {
    hook_spool_state_dir("processing")
}

fn hook_spool_done_dir() -> PathBuf {
    hook_spool_state_dir("done")
}

fn hook_spool_failed_dir() -> PathBuf {
    hook_spool_state_dir("failed")
}

pub fn ensure_hook_spool_dirs() -> RewResult<()> {
    fs::create_dir_all(hook_spool_pending_dir())?;
    fs::create_dir_all(hook_spool_processing_dir())?;
    fs::create_dir_all(hook_spool_done_dir())?;
    fs::create_dir_all(hook_spool_failed_dir())?;
    Ok(())
}

pub fn append_hook_event(event: &HookEventEnvelope) -> RewResult<PathBuf> {
    ensure_hook_spool_dirs()?;
    let ts = Utc::now().timestamp_micros();
    let file_name = format!("{:020}_{}.json", ts, event.event_id);
    let pending_path = hook_spool_pending_dir().join(&file_name);
    let tmp_path = hook_spool_pending_dir().join(format!("{file_name}.tmp"));
    fs::write(&tmp_path, serde_json::to_vec(event)?)?;
    fs::rename(&tmp_path, &pending_path)?;
    Ok(pending_path)
}

pub fn requeue_hook_spool_processing_files() -> RewResult<usize> {
    ensure_hook_spool_dirs()?;
    let mut moved = 0usize;
    for entry in fs::read_dir(hook_spool_processing_dir())? {
        let entry = entry?;
        let src = entry.path();
        if src.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let dest = hook_spool_pending_dir().join(entry.file_name());
        if fs::rename(&src, &dest).is_ok() {
            moved += 1;
        }
    }
    Ok(moved)
}

pub fn claim_oldest_hook_event() -> RewResult<Option<(PathBuf, HookEventEnvelope)>> {
    ensure_hook_spool_dirs()?;
    let mut files = fs::read_dir(hook_spool_pending_dir())?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();

    let Some(src) = files.into_iter().next() else {
        return Ok(None);
    };

    let dest = hook_spool_processing_dir().join(
        src.file_name()
            .ok_or_else(|| crate::error::RewError::Snapshot("invalid spool file name".into()))?,
    );
    fs::rename(&src, &dest)?;
    let raw = fs::read(&dest)?;
    let envelope = match serde_json::from_slice::<HookEventEnvelope>(&raw) {
        Ok(envelope) => envelope,
        Err(err) => {
            let _ = mark_hook_event_failed(&dest, &err.to_string());
            return Err(err.into());
        }
    };
    Ok(Some((dest, envelope)))
}

pub fn mark_hook_event_done(processing_path: &Path) -> RewResult<()> {
    ensure_hook_spool_dirs()?;
    let dest = hook_spool_done_dir().join(
        processing_path
            .file_name()
            .ok_or_else(|| crate::error::RewError::Snapshot("invalid done spool file".into()))?,
    );
    fs::rename(processing_path, dest)?;
    Ok(())
}

pub fn mark_hook_event_failed(processing_path: &Path, error: &str) -> RewResult<()> {
    ensure_hook_spool_dirs()?;
    let file_name = processing_path
        .file_name()
        .ok_or_else(|| crate::error::RewError::Snapshot("invalid failed spool file".into()))?;
    let dest = hook_spool_failed_dir().join(file_name);
    fs::rename(processing_path, &dest)?;
    let sidecar = dest.with_extension("error.txt");
    let _ = fs::write(sidecar, error.as_bytes());
    Ok(())
}

pub fn deterministic_event_id(seed: &str) -> String {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn generate_task_id() -> String {
    let ts = Utc::now().format("%m%d%H%M").to_string();
    let rand = deterministic_event_id(&format!("{}-{:?}", ts, std::thread::current().id()));
    format!("t{}_{:04}", ts, u16::from_str_radix(&rand[0..4], 16).unwrap_or(0) % 10000)
}

fn is_shell_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Bash" | "bash" | "Shell" | "shell" | "execute_command")
}

fn is_edit_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Edit" | "edit" | "MultiEdit" | "replace_in_file")
}

fn infer_path_change_type(tool_name: &str, file_exists_after: bool, baseline_existed: bool) -> ChangeType {
    if !file_exists_after {
        return ChangeType::Deleted;
    }
    if is_edit_tool(tool_name) || baseline_existed {
        ChangeType::Modified
    } else {
        ChangeType::Created
    }
}

fn build_change_attribution(tool_name: &str) -> &'static str {
    if is_shell_tool(tool_name) {
        "bash_predicted"
    } else {
        "hook"
    }
}

pub fn process_hook_event(
    db: &Database,
    envelope: &HookEventEnvelope,
) -> RewResult<HookEventProcessOutcome> {
    if !db.claim_hook_event_receipt(&envelope.idempotency_key, envelope.payload.event_type_name())? {
        return Ok(HookEventProcessOutcome { task_id: None });
    }

    match &envelope.payload {
        HookEventPayload::PromptStarted(payload) => process_prompt_started(db, envelope, payload),
        HookEventPayload::PostToolObserved(payload) => process_post_tool_observed(db, payload),
        HookEventPayload::TaskStopRequested(payload) => process_task_stop_requested(db, envelope, payload),
    }
}

fn process_prompt_started(
    db: &Database,
    _envelope: &HookEventEnvelope,
    payload: &PromptStartedPayload,
) -> RewResult<HookEventProcessOutcome> {
    if let Ok(Some(old_tid)) = db.get_active_task_for_session(&payload.session_key) {
        let _ = db.update_task_status(&old_tid, &TaskStatus::Completed, Some(Utc::now()));
        let _ = db.deactivate_session(&payload.session_key);
    }

    let task_id = generate_task_id();
    let task = Task {
        id: task_id.clone(),
        prompt: payload.prompt.clone().filter(|s| !s.is_empty()),
        tool: Some(payload.tool_source.clone()),
        started_at: Utc::now(),
        completed_at: None,
        status: TaskStatus::Active,
        risk_level: None,
        summary: None,
        cwd: payload.cwd.clone(),
    };
    db.create_task(&task)?;

    let stats = TaskStats {
        task_id: task_id.clone(),
        model: payload.model.clone(),
        duration_secs: None,
        tool_calls: 0,
        files_changed: 0,
        input_tokens: None,
        output_tokens: None,
        total_cost_usd: None,
        session_id: Some(payload.session_key.clone()),
        conversation_id: payload.conversation_id.clone(),
        extra_json: None,
    };
    let _ = db.create_task_stats(&stats);
    let _ = db.insert_active_session(&payload.session_key, &task_id, &payload.tool_source, Utc::now());
    if let Some(gid) = payload.generation_id.as_deref() {
        let _ = db.set_stop_guard(&payload.session_key, gid);
    }

    Ok(HookEventProcessOutcome {
        task_id: Some(task_id),
    })
}

fn process_post_tool_observed(
    db: &Database,
    payload: &PostToolObservedPayload,
) -> RewResult<HookEventProcessOutcome> {
    let Some(task_id) = db.get_active_task_for_session(&payload.session_key)? else {
        return Ok(HookEventProcessOutcome { task_id: None });
    };

    let _ = db.increment_tool_calls(&task_id);
    let obj_store = ObjectStore::new(rew_home_dir().join("objects"))?;
    let attribution = build_change_attribution(&payload.tool_name);
    let seen_at = Utc::now().to_rfc3339();

    for observed in &payload.observations {
        let path = PathBuf::from(&observed.file_path);
        let baseline = resolve_baseline(db, &task_id, &path, Some(&payload.session_key));
        let change_type = infer_path_change_type(
            &payload.tool_name,
            observed.file_exists_after,
            baseline.existed,
        );
        let old_hash = if change_type == ChangeType::Created {
            None
        } else {
            baseline.hash
        };
        let (lines_added, lines_removed) = crate::diff::count_changed_lines_from_store(
            &obj_store,
            old_hash.as_deref(),
            observed.new_hash.as_deref(),
        );
        let change = Change {
            id: None,
            task_id: task_id.clone(),
            file_path: path,
            change_type,
            old_hash,
            new_hash: observed.new_hash.clone(),
            diff_text: None,
            lines_added,
            lines_removed,
            attribution: Some(attribution.to_string()),
            old_file_path: None,
        };
        db.upsert_change(&change)?;
        let _ = sync_file_index_after_change(db, &change, attribution, &seen_at);
    }

    Ok(HookEventProcessOutcome {
        task_id: Some(task_id),
    })
}

fn process_task_stop_requested(
    db: &Database,
    envelope: &HookEventEnvelope,
    payload: &TaskStopRequestedPayload,
) -> RewResult<HookEventProcessOutcome> {
    let Some(task_id) = db.get_active_task_for_session(&payload.session_key)? else {
        return Ok(HookEventProcessOutcome { task_id: None });
    };

    if let Some(ref stop_gid) = payload.generation_id {
        if let Ok(Some(stored_gid)) = db.get_stop_guard(&payload.session_key) {
            if stored_gid != *stop_gid {
                return Ok(HookEventProcessOutcome { task_id: None });
            }
        }
    } else if let Ok(Some(task)) = db.get_task(&task_id) {
        let age_ms = (Utc::now() - task.started_at).num_milliseconds();
        if age_ms < 3_000 {
            return Ok(HookEventProcessOutcome { task_id: None });
        }
    }

    let completed_at = Utc::now();
    db.update_task_status(&task_id, &TaskStatus::Completed, Some(completed_at))?;
    db.deactivate_sessions_for_task(&task_id)?;
    db.delete_stop_guard(&payload.session_key)?;
    db.delete_pre_tool_hashes_for_session(&payload.session_key)?;
    db.enqueue_task_finalization(&task_id, Some(&envelope.event_id))?;

    Ok(HookEventProcessOutcome {
        task_id: Some(task_id),
    })
}

impl HookEventPayload {
    pub fn event_type_name(&self) -> &'static str {
        match self {
            HookEventPayload::PromptStarted(_) => "prompt-started",
            HookEventPayload::PostToolObserved(_) => "post-tool-observed",
            HookEventPayload::TaskStopRequested(_) => "task-stop-requested",
        }
    }
}
