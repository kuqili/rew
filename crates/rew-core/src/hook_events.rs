use crate::baseline::{resolve_baseline, resolve_baseline_with_objects_root};
use crate::db::Database;
use crate::error::RewResult;
use crate::file_index::sync_file_index_after_change;
use crate::objects::ObjectStore;
use crate::pre_tool_store::{
    delete_pre_tool_hashes_for_session, delete_pre_tool_hashes_for_session_in,
    pre_tool_store_root_for_objects_root,
};
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

fn hook_spool_state_dir_in(rew_home: &Path, state: &str) -> PathBuf {
    rew_home.join(HOOK_SPOOL_DIR).join(state)
}

fn hook_spool_state_dir(state: &str) -> PathBuf {
    hook_spool_state_dir_in(&rew_home_dir(), state)
}

pub fn hook_spool_pending_dir() -> PathBuf {
    hook_spool_state_dir("pending")
}

fn ensure_hook_spool_dirs_in(rew_home: &Path) -> RewResult<()> {
    fs::create_dir_all(hook_spool_state_dir_in(rew_home, "pending"))?;
    fs::create_dir_all(hook_spool_state_dir_in(rew_home, "processing"))?;
    fs::create_dir_all(hook_spool_state_dir_in(rew_home, "done"))?;
    fs::create_dir_all(hook_spool_state_dir_in(rew_home, "failed"))?;
    Ok(())
}

pub fn ensure_hook_spool_dirs() -> RewResult<()> {
    ensure_hook_spool_dirs_in(&rew_home_dir())
}

fn append_hook_event_in(rew_home: &Path, event: &HookEventEnvelope) -> RewResult<PathBuf> {
    ensure_hook_spool_dirs_in(rew_home)?;
    let ts = Utc::now().timestamp_micros();
    let file_name = format!("{:020}_{}.json", ts, event.event_id);
    let pending_dir = hook_spool_state_dir_in(rew_home, "pending");
    let pending_path = pending_dir.join(&file_name);
    let tmp_path = pending_dir.join(format!("{file_name}.tmp"));
    fs::write(&tmp_path, serde_json::to_vec(event)?)?;
    fs::rename(&tmp_path, &pending_path)?;
    Ok(pending_path)
}

pub fn append_hook_event(event: &HookEventEnvelope) -> RewResult<PathBuf> {
    append_hook_event_in(&rew_home_dir(), event)
}

fn requeue_hook_spool_processing_files_in(rew_home: &Path) -> RewResult<usize> {
    ensure_hook_spool_dirs_in(rew_home)?;
    let mut moved = 0usize;
    for entry in fs::read_dir(hook_spool_state_dir_in(rew_home, "processing"))? {
        let entry = entry?;
        let src = entry.path();
        if src.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let dest = hook_spool_state_dir_in(rew_home, "pending").join(entry.file_name());
        if fs::rename(&src, &dest).is_ok() {
            moved += 1;
        }
    }
    Ok(moved)
}

pub fn requeue_hook_spool_processing_files() -> RewResult<usize> {
    requeue_hook_spool_processing_files_in(&rew_home_dir())
}

fn claim_oldest_hook_event_in(rew_home: &Path) -> RewResult<Option<(PathBuf, HookEventEnvelope)>> {
    ensure_hook_spool_dirs_in(rew_home)?;
    let mut files = fs::read_dir(hook_spool_state_dir_in(rew_home, "pending"))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();

    let Some(src) = files.into_iter().next() else {
        return Ok(None);
    };

    let dest = hook_spool_state_dir_in(rew_home, "processing").join(
        src.file_name()
            .ok_or_else(|| crate::error::RewError::Snapshot("invalid spool file name".into()))?,
    );
    fs::rename(&src, &dest)?;
    let raw = fs::read(&dest)?;
    let envelope = match serde_json::from_slice::<HookEventEnvelope>(&raw) {
        Ok(envelope) => envelope,
        Err(err) => {
            let _ = mark_hook_event_failed_in(rew_home, &dest, &err.to_string());
            return Err(err.into());
        }
    };
    Ok(Some((dest, envelope)))
}

pub fn claim_oldest_hook_event() -> RewResult<Option<(PathBuf, HookEventEnvelope)>> {
    claim_oldest_hook_event_in(&rew_home_dir())
}

fn mark_hook_event_done_in(rew_home: &Path, processing_path: &Path) -> RewResult<()> {
    ensure_hook_spool_dirs_in(rew_home)?;
    let dest = hook_spool_state_dir_in(rew_home, "done").join(
        processing_path
            .file_name()
            .ok_or_else(|| crate::error::RewError::Snapshot("invalid done spool file".into()))?,
    );
    fs::rename(processing_path, dest)?;
    Ok(())
}

pub fn mark_hook_event_done(processing_path: &Path) -> RewResult<()> {
    mark_hook_event_done_in(&rew_home_dir(), processing_path)
}

fn mark_hook_event_failed_in(rew_home: &Path, processing_path: &Path, error: &str) -> RewResult<()> {
    ensure_hook_spool_dirs_in(rew_home)?;
    let file_name = processing_path
        .file_name()
        .ok_or_else(|| crate::error::RewError::Snapshot("invalid failed spool file".into()))?;
    let dest = hook_spool_state_dir_in(rew_home, "failed").join(file_name);
    fs::rename(processing_path, &dest)?;
    let sidecar = dest.with_extension("error.txt");
    let _ = fs::write(sidecar, error.as_bytes());
    Ok(())
}

pub fn mark_hook_event_failed(processing_path: &Path, error: &str) -> RewResult<()> {
    mark_hook_event_failed_in(&rew_home_dir(), processing_path, error)
}

pub fn deterministic_event_id(seed: &str) -> String {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn generate_task_id(seed: &str) -> String {
    let ts = Utc::now().format("%m%d%H%M").to_string();
    let suffix = deterministic_event_id(seed);
    format!("t{}_{}", ts, &suffix[..8])
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
    let seed = envelope.event_id.clone();
    let mut next_task_id = move || generate_task_id(&seed);
    process_hook_event_internal(db, envelope, &mut next_task_id, None)
}

fn process_hook_event_internal<F>(
    db: &Database,
    envelope: &HookEventEnvelope,
    next_task_id: &mut F,
    objects_root_override: Option<&Path>,
) -> RewResult<HookEventProcessOutcome>
where
    F: FnMut() -> String,
{
    if db.hook_event_receipt_exists(&envelope.idempotency_key)? {
        return Ok(HookEventProcessOutcome { task_id: None });
    }

    let outcome = match &envelope.payload {
        HookEventPayload::PromptStarted(payload) => {
            process_prompt_started(db, envelope, payload, next_task_id)
        }
        HookEventPayload::PostToolObserved(payload) => {
            process_post_tool_observed(db, payload, objects_root_override)
        }
        HookEventPayload::TaskStopRequested(payload) => {
            process_task_stop_requested(db, envelope, payload, objects_root_override)
        }
    }?;

    db.record_hook_event_receipt(&envelope.idempotency_key, envelope.payload.event_type_name())?;
    Ok(outcome)
}

fn process_prompt_started<F>(
    db: &Database,
    _envelope: &HookEventEnvelope,
    payload: &PromptStartedPayload,
    next_task_id: &mut F,
) -> RewResult<HookEventProcessOutcome>
where
    F: FnMut() -> String,
{
    if let Ok(Some(old_tid)) = db.get_active_task_for_session(&payload.session_key) {
        let _ = db.update_task_status(&old_tid, &TaskStatus::Completed, Some(Utc::now()));
        let _ = db.deactivate_session(&payload.session_key);
    }

    let task_id = next_task_id();
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

    db.create_task_bundle(
        &task,
        &stats,
        &payload.session_key,
        &payload.tool_source,
        payload.generation_id.as_deref(),
    )?;

    Ok(HookEventProcessOutcome {
        task_id: Some(task_id),
    })
}

fn process_post_tool_observed(
    db: &Database,
    payload: &PostToolObservedPayload,
    objects_root_override: Option<&Path>,
) -> RewResult<HookEventProcessOutcome> {
    let Some(task_id) = db.get_active_task_for_session(&payload.session_key)? else {
        return Ok(HookEventProcessOutcome { task_id: None });
    };

    let _ = db.increment_tool_calls(&task_id);
    let objects_root = objects_root_override
        .map(PathBuf::from)
        .unwrap_or_else(|| rew_home_dir().join("objects"));
    let obj_store = ObjectStore::new(objects_root)?;
    let attribution = build_change_attribution(&payload.tool_name);
    let seen_at = Utc::now().to_rfc3339();

    for observed in &payload.observations {
        let path = PathBuf::from(&observed.file_path);
        let baseline = if let Some(objects_root) = objects_root_override {
            resolve_baseline_with_objects_root(
                db,
                &task_id,
                &path,
                Some(&payload.session_key),
                objects_root.to_path_buf(),
            )
        } else {
            resolve_baseline(db, &task_id, &path, Some(&payload.session_key))
        };
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
    objects_root_override: Option<&Path>,
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
    if let Some(objects_root) = objects_root_override {
        let pre_tool_root = pre_tool_store_root_for_objects_root(objects_root);
        delete_pre_tool_hashes_for_session_in(&pre_tool_root, &payload.session_key)?;
    } else {
        delete_pre_tool_hashes_for_session(&payload.session_key)?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pre_tool_store::{get_pre_tool_hash_in, set_pre_tool_hash_in};
    use tempfile::{tempdir, TempDir};
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};

    struct HookFlowEnv {
        _dir: TempDir,
        db: Database,
        objects_root: PathBuf,
        fixtures_root: PathBuf,
    }

    impl HookFlowEnv {
        fn new() -> Self {
            let dir = tempdir().unwrap();
            let db = Database::open(&dir.path().join("test.db")).unwrap();
            db.initialize().unwrap();
            let objects_root = dir.path().join("objects");
            let fixtures_root = dir.path().join("fixtures");
            std::fs::create_dir_all(&objects_root).unwrap();
            std::fs::create_dir_all(&fixtures_root).unwrap();
            Self {
                _dir: dir,
                db,
                objects_root,
                fixtures_root,
            }
        }

        fn store_content(&self, name: &str, content: &str) -> String {
            let path = self.fixtures_root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
            ObjectStore::new(self.objects_root.clone())
                .unwrap()
                .store(&path)
                .unwrap()
        }

        fn live_path(&self, relative: &str) -> PathBuf {
            self.fixtures_root.join("live").join(relative)
        }

        fn pre_tool_root(&self) -> PathBuf {
            pre_tool_store_root_for_objects_root(&self.objects_root)
        }

        fn write_live_file(&self, relative: &str, content: &str) -> PathBuf {
            let path = self.live_path(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
            path
        }

        fn delete_live_file(&self, relative: &str) {
            let path = self.live_path(relative);
            let _ = std::fs::remove_file(path);
        }

        fn seed_pre_tool_hash(&self, session_key: &str, file_path: &str, hash: &str) {
            set_pre_tool_hash_in(&self.pre_tool_root(), session_key, file_path, hash).unwrap();
        }

        fn age_task_for_guard(&self, task_id: &str, seconds: i64) {
            let started_at = (Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339();
            self.db
                .connection()
                .execute(
                    "UPDATE tasks SET started_at = ?1 WHERE id = ?2",
                    rusqlite::params![started_at, task_id],
                )
                .unwrap();
        }

        fn finalize_next_job(&self) -> String {
            let job = self
                .db
                .claim_next_task_finalization_job()
                .unwrap()
                .expect("queued finalization job");
            crate::reconcile::reconcile_task(&self.db, &job.task_id, &self.objects_root).unwrap();
            self.db.refresh_task_rollup_from_changes(&job.task_id).unwrap();
            self.db.mark_task_finalization_done(&job.task_id).unwrap();
            job.task_id
        }

        fn finalize_all_jobs(&self) -> Vec<String> {
            let mut task_ids = Vec::new();
            while let Some(job) = self.db.claim_next_task_finalization_job().unwrap() {
                crate::reconcile::reconcile_task(&self.db, &job.task_id, &self.objects_root)
                    .unwrap();
                self.db.refresh_task_rollup_from_changes(&job.task_id).unwrap();
                self.db.mark_task_finalization_done(&job.task_id).unwrap();
                task_ids.push(job.task_id);
            }
            task_ids
        }
    }

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        db
    }

    fn prompt_envelope(
        tool_source: &str,
        session_key: &str,
        prompt: &str,
        conversation_id: Option<&str>,
        generation_id: Option<&str>,
    ) -> HookEventEnvelope {
        HookEventEnvelope {
            event_id: format!(
                "prompt_{}",
                deterministic_event_id(&format!("{tool_source}:{session_key}:{prompt}"))
            ),
            idempotency_key: format!("prompt:{session_key}:{}", deterministic_event_id(prompt)),
            created_at: Utc::now().to_rfc3339(),
            payload_version: 1,
            payload: HookEventPayload::PromptStarted(PromptStartedPayload {
                tool_source: tool_source.to_string(),
                session_key: session_key.to_string(),
                prompt: Some(prompt.to_string()),
                cwd: Some("/tmp/project".to_string()),
                model: Some("test".to_string()),
                conversation_id: conversation_id.map(str::to_string),
                generation_id: generation_id.map(str::to_string),
            }),
        }
    }

    fn post_envelope(
        tool_source: &str,
        session_key: &str,
        tool_name: &str,
        observations: Vec<ObservedPathChange>,
    ) -> HookEventEnvelope {
        HookEventEnvelope {
            event_id: format!(
                "post_{}",
                deterministic_event_id(&format!("{tool_source}:{session_key}:{tool_name}:{observations:?}"))
            ),
            idempotency_key: format!(
                "post:{session_key}:{}",
                deterministic_event_id(&format!("{tool_name}:{observations:?}"))
            ),
            created_at: Utc::now().to_rfc3339(),
            payload_version: 1,
            payload: HookEventPayload::PostToolObserved(PostToolObservedPayload {
                tool_source: tool_source.to_string(),
                session_key: session_key.to_string(),
                tool_name: tool_name.to_string(),
                cwd: Some("/tmp/project".to_string()),
                observations,
            }),
        }
    }

    fn stop_envelope(
        tool_source: &str,
        session_key: &str,
        generation_id: Option<&str>,
    ) -> HookEventEnvelope {
        HookEventEnvelope {
            event_id: format!(
                "stop_{}",
                deterministic_event_id(&format!("{tool_source}:{session_key}:{generation_id:?}"))
            ),
            idempotency_key: format!(
                "stop:{session_key}:{}",
                deterministic_event_id(&format!("{generation_id:?}"))
            ),
            created_at: Utc::now().to_rfc3339(),
            payload_version: 1,
            payload: HookEventPayload::TaskStopRequested(TaskStopRequestedPayload {
                tool_source: tool_source.to_string(),
                session_key: session_key.to_string(),
                generation_id: generation_id.map(str::to_string),
            }),
        }
    }

    fn process_event_with_objects(
        db: &Database,
        envelope: &HookEventEnvelope,
        objects_root: &Path,
    ) -> HookEventProcessOutcome {
        let seed = envelope.event_id.clone();
        let mut next_task_id = move || generate_task_id(&seed);
        process_hook_event_internal(db, envelope, &mut next_task_id, Some(objects_root)).unwrap()
    }

    #[test]
    fn prompt_events_create_distinct_task_ids() {
        let db = test_db();

        let first = prompt_envelope(
            "cursor",
            "cursor:session-a",
            "first prompt",
            Some("conv-a"),
            Some("gen-a"),
        );
        let second = prompt_envelope(
            "cursor",
            "cursor:session-b",
            "second prompt",
            Some("conv-b"),
            Some("gen-b"),
        );

        let first_outcome = process_hook_event(&db, &first).unwrap();
        let second_outcome = process_hook_event(&db, &second).unwrap();

        let first_id = first_outcome.task_id.expect("first task id");
        let second_id = second_outcome.task_id.expect("second task id");

        assert_ne!(first_id, second_id, "distinct prompt events should create distinct task ids");
    }

    #[test]
    fn failed_prompt_does_not_consume_receipt_before_retry() {
        let db = test_db();
        let envelope = prompt_envelope(
            "cursor",
            "cursor:session-retry",
            "retry prompt",
            Some("conv-retry"),
            Some("gen-retry"),
        );

        db.create_task(&Task {
            id: "fixed-task".to_string(),
            prompt: Some("existing".to_string()),
            tool: Some("cursor".to_string()),
            started_at: Utc::now(),
            completed_at: None,
            status: TaskStatus::Active,
            risk_level: None,
            summary: None,
            cwd: None,
        }).unwrap();

        let mut fixed_generator = || "fixed-task".to_string();
        assert!(process_hook_event_internal(&db, &envelope, &mut fixed_generator, None).is_err());

        let mut retry_generator = || "retry-task".to_string();
        let retry_outcome = process_hook_event_internal(&db, &envelope, &mut retry_generator, None)
            .expect("retry should still be processed after a failed first attempt");

        assert_eq!(retry_outcome.task_id.as_deref(), Some("retry-task"));
        assert!(db.get_task("retry-task").unwrap().is_some());
    }

    #[test]
    fn generate_task_id_is_stable_per_event_and_distinct_across_events() {
        let a = generate_task_id("event-a");
        let b = generate_task_id("event-b");
        let a_again = generate_task_id("event-a");

        assert_eq!(a, a_again);
        assert_ne!(a, b);
    }

    #[test]
    fn claude_code_hook_flow_modifies_existing_file_end_to_end() {
        let env = HookFlowEnv::new();
        let session_key = "claude-code:session-claude";
        let old_content = "fn main() {\n    println!(\"old\");\n}\n";
        let new_content = "fn main() {\n    println!(\"new\");\n}\n";
        let live_path = env.write_live_file("src/lib.rs", old_content);
        let file_path = live_path.to_string_lossy().to_string();
        let old_hash = env.store_content("claude_old.rs", old_content);
        std::fs::write(&live_path, new_content).unwrap();
        let new_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();

        let prompt = prompt_envelope(
            "claude-code",
            session_key,
            "update main output",
            None,
            None,
        );
        let prompt_outcome = process_event_with_objects(&env.db, &prompt, &env.objects_root);
        let task_id = prompt_outcome.task_id.expect("claude task");
        env.age_task_for_guard(&task_id, 5);
        env.seed_pre_tool_hash(session_key, &file_path, &old_hash);

        let post = post_envelope(
            "claude-code",
            session_key,
            "Edit",
            vec![ObservedPathChange {
                file_path: file_path.clone(),
                file_exists_after: true,
                new_hash: Some(new_hash.clone()),
            }],
        );
        process_event_with_objects(&env.db, &post, &env.objects_root);

        let stop = stop_envelope("claude-code", session_key, None);
        process_event_with_objects(&env.db, &stop, &env.objects_root);
        let finalized_task_id = env.finalize_next_job();
        assert_eq!(finalized_task_id, task_id);

        let changes = env.db.get_changes_for_task(&task_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Modified);
        assert_eq!(changes[0].old_hash.as_deref(), Some(old_hash.as_str()));
        assert_eq!(changes[0].new_hash.as_deref(), Some(new_hash.as_str()));
        assert_eq!(changes[0].attribution.as_deref(), Some("hook"));
        assert!(changes[0].lines_added > 0 || changes[0].lines_removed > 0);
        assert!(env.db.get_active_task_for_session(session_key).unwrap().is_none());
    }

    #[test]
    fn cursor_hook_flow_creates_file_and_finishes_finalization() {
        let env = HookFlowEnv::new();
        let session_key = "cursor:conv-123";
        let live_path = env.write_live_file("notes/new.md", "# title\nhello cursor\n");
        let file_path = live_path.to_string_lossy().to_string();
        let new_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();

        let prompt = prompt_envelope(
            "cursor",
            session_key,
            "create note",
            Some("conv-123"),
            Some("gen-123"),
        );
        let prompt_outcome = process_event_with_objects(&env.db, &prompt, &env.objects_root);
        let task_id = prompt_outcome.task_id.expect("cursor task");

        let post = post_envelope(
            "cursor",
            session_key,
            "write_to_file",
            vec![ObservedPathChange {
                file_path: file_path.clone(),
                file_exists_after: true,
                new_hash: Some(new_hash.clone()),
            }],
        );
        process_event_with_objects(&env.db, &post, &env.objects_root);

        let stop = stop_envelope("cursor", session_key, Some("gen-123"));
        process_event_with_objects(&env.db, &stop, &env.objects_root);
        let finalized_task_id = env.finalize_next_job();
        assert_eq!(finalized_task_id, task_id);

        let changes = env.db.get_changes_for_task(&task_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Created);
        assert_eq!(changes[0].new_hash.as_deref(), Some(new_hash.as_str()));
        assert_eq!(changes[0].attribution.as_deref(), Some("hook"));
        assert_eq!(
            env.db.get_task_finalization_status(&task_id).unwrap().as_deref(),
            Some("done")
        );
    }

    #[test]
    fn cursor_existing_file_edit_without_pre_tool_uses_file_index_baseline() {
        let env = HookFlowEnv::new();
        let session_key = "cursor:conv-edit";
        let old_content = "hello\ncursor\n";
        let new_content = "hello\ncursor updated\nextra\n";
        let live_path = env.write_live_file("docs/existing.md", old_content);
        let file_path = live_path.to_string_lossy().to_string();
        let old_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();
        env.db
            .upsert_live_file_index_entry(
                &file_path,
                1,
                &old_hash,
                Some(&old_hash),
                "test",
                "seed",
                &Utc::now().to_rfc3339(),
                Some(1),
            )
            .unwrap();
        std::fs::write(&live_path, new_content).unwrap();
        let new_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();

        let prompt = prompt_envelope(
            "cursor",
            session_key,
            "edit existing note",
            Some("conv-edit"),
            Some("gen-edit"),
        );
        let task_id = process_event_with_objects(&env.db, &prompt, &env.objects_root)
            .task_id
            .expect("cursor edit task");

        let post = post_envelope(
            "cursor",
            session_key,
            "write_to_file",
            vec![ObservedPathChange {
                file_path: file_path.clone(),
                file_exists_after: true,
                new_hash: Some(new_hash.clone()),
            }],
        );
        process_event_with_objects(&env.db, &post, &env.objects_root);
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_key, Some("gen-edit")),
            &env.objects_root,
        );
        env.finalize_next_job();

        let changes = env.db.get_changes_for_task(&task_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Modified);
        assert_eq!(changes[0].old_hash.as_deref(), Some(old_hash.as_str()));
        assert_eq!(changes[0].new_hash.as_deref(), Some(new_hash.as_str()));
    }

    #[test]
    fn codebuddy_hook_flow_pairs_rename_after_finalize() {
        let env = HookFlowEnv::new();
        let session_key = "codebuddy:cb-rename";
        let old_content = "hello\nsame line\n";
        let new_content = "hello\nsame line updated\n";
        let old_live_path = env.write_live_file("docs/old.md", old_content);
        let old_path = old_live_path.to_string_lossy().to_string();
        let old_hash = env.store_content("codebuddy_old.md", old_content);
        env.delete_live_file("docs/old.md");
        let new_live_path = env.write_live_file("docs/new.md", new_content);
        let new_path = new_live_path.to_string_lossy().to_string();
        let new_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&new_live_path)
            .unwrap();

        let prompt = prompt_envelope(
            "codebuddy",
            session_key,
            "rename docs file",
            None,
            Some("cb-gen"),
        );
        let prompt_outcome = process_event_with_objects(&env.db, &prompt, &env.objects_root);
        let task_id = prompt_outcome.task_id.expect("codebuddy task");
        env.seed_pre_tool_hash(session_key, &old_path, &old_hash);

        let delete_old = post_envelope(
            "codebuddy",
            session_key,
            "delete_file",
            vec![ObservedPathChange {
                file_path: old_path.clone(),
                file_exists_after: false,
                new_hash: None,
            }],
        );
        process_event_with_objects(&env.db, &delete_old, &env.objects_root);

        let create_new = post_envelope(
            "codebuddy",
            session_key,
            "write_to_file",
            vec![ObservedPathChange {
                file_path: new_path.clone(),
                file_exists_after: true,
                new_hash: Some(new_hash.clone()),
            }],
        );
        process_event_with_objects(&env.db, &create_new, &env.objects_root);

        let stop = stop_envelope("codebuddy", session_key, Some("cb-gen"));
        process_event_with_objects(&env.db, &stop, &env.objects_root);
        env.finalize_next_job();

        let changes = env.db.get_changes_for_task(&task_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Renamed);
        assert_eq!(changes[0].old_file_path.as_deref(), Some(Path::new(&old_path)));
        assert_eq!(changes[0].file_path, PathBuf::from(&new_path));
        assert_eq!(changes[0].old_hash.as_deref(), Some(old_hash.as_str()));
        assert_eq!(changes[0].new_hash.as_deref(), Some(new_hash.as_str()));
    }

    #[test]
    fn workbuddy_hook_flow_deletes_existing_file_end_to_end() {
        let env = HookFlowEnv::new();
        let session_key = "workbuddy:wb-delete";
        let old_content = "line1\nline2\n";
        let live_path = env.write_live_file("tmp/old.log", old_content);
        let file_path = live_path.to_string_lossy().to_string();
        let old_hash = env.store_content("workbuddy_old.log", old_content);
        env.delete_live_file("tmp/old.log");

        let prompt = prompt_envelope(
            "workbuddy",
            session_key,
            "delete temp file",
            None,
            Some("wb-gen"),
        );
        let prompt_outcome = process_event_with_objects(&env.db, &prompt, &env.objects_root);
        let task_id = prompt_outcome.task_id.expect("workbuddy task");
        env.seed_pre_tool_hash(session_key, &file_path, &old_hash);

        let post = post_envelope(
            "workbuddy",
            session_key,
            "remove_file",
            vec![ObservedPathChange {
                file_path: file_path.clone(),
                file_exists_after: false,
                new_hash: None,
            }],
        );
        process_event_with_objects(&env.db, &post, &env.objects_root);

        let stop = stop_envelope("workbuddy", session_key, Some("wb-gen"));
        process_event_with_objects(&env.db, &stop, &env.objects_root);
        env.finalize_next_job();

        let changes = env.db.get_changes_for_task(&task_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Deleted);
        assert_eq!(changes[0].old_hash.as_deref(), Some(old_hash.as_str()));
        assert!(changes[0].new_hash.is_none());
        assert_eq!(changes[0].attribution.as_deref(), Some("hook"));
    }

    #[test]
    fn shell_delete_uses_pre_tool_hash_as_baseline() {
        let env = HookFlowEnv::new();
        let session_key = "cursor:conv-shell-delete";
        let old_content = "line1\nline2\n";
        let live_path = env.write_live_file("tmp/shell-old.log", old_content);
        let file_path = live_path.to_string_lossy().to_string();
        let old_hash = env.store_content("shell_old.log", old_content);
        env.delete_live_file("tmp/shell-old.log");

        let prompt = prompt_envelope(
            "cursor",
            session_key,
            "delete file via shell",
            Some("conv-shell-delete"),
            Some("gen-shell-delete"),
        );
        let task_id = process_event_with_objects(&env.db, &prompt, &env.objects_root)
            .task_id
            .expect("shell delete task");
        env.seed_pre_tool_hash(session_key, &file_path, &old_hash);

        let post = post_envelope(
            "cursor",
            session_key,
            "execute_command",
            vec![ObservedPathChange {
                file_path: file_path.clone(),
                file_exists_after: false,
                new_hash: None,
            }],
        );
        process_event_with_objects(&env.db, &post, &env.objects_root);

        let stop = stop_envelope("cursor", session_key, Some("gen-shell-delete"));
        process_event_with_objects(&env.db, &stop, &env.objects_root);
        env.finalize_next_job();

        let changes = env.db.get_changes_for_task(&task_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Deleted);
        assert_eq!(changes[0].old_hash.as_deref(), Some(old_hash.as_str()));
        assert!(changes[0].new_hash.is_none());
        assert_eq!(changes[0].attribution.as_deref(), Some("bash_predicted"));
    }

    #[test]
    fn shell_deleted_directory_can_be_restored_via_directory_undo() {
        let env = HookFlowEnv::new();
        let session_key = "cursor:conv-shell-delete-dir";
        let root = env.live_path("tmp/design_mockup");
        let file_a = env.write_live_file("tmp/design_mockup/00-overview.md", "# overview\n");
        let file_b = env.write_live_file("tmp/design_mockup/specs/a.txt", "alpha spec\n");
        let file_a_path = file_a.to_string_lossy().to_string();
        let file_b_path = file_b.to_string_lossy().to_string();
        let hash_a = env.store_content("shell_delete_dir/00-overview.md", "# overview\n");
        let hash_b = env.store_content("shell_delete_dir/specs-a.txt", "alpha spec\n");
        std::fs::remove_dir_all(&root).unwrap();

        let task_id = process_event_with_objects(
            &env.db,
            &prompt_envelope(
                "cursor",
                session_key,
                "delete directory via shell",
                Some("conv-shell-delete-dir"),
                Some("gen-shell-delete-dir"),
            ),
            &env.objects_root,
        )
        .task_id
        .expect("shell delete dir task");
        env.seed_pre_tool_hash(session_key, &file_a_path, &hash_a);
        env.seed_pre_tool_hash(session_key, &file_b_path, &hash_b);

        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                session_key,
                "execute_command",
                vec![
                    ObservedPathChange {
                        file_path: file_a_path.clone(),
                        file_exists_after: false,
                        new_hash: None,
                    },
                    ObservedPathChange {
                        file_path: file_b_path.clone(),
                        file_exists_after: false,
                        new_hash: None,
                    },
                ],
            ),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_key, Some("gen-shell-delete-dir")),
            &env.objects_root,
        );
        env.finalize_next_job();

        let changes = env.db.get_changes_for_task(&task_id).unwrap();
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().all(|change| change.change_type == ChangeType::Deleted));
        assert!(
            changes
                .iter()
                .all(|change| change.attribution.as_deref() == Some("bash_predicted"))
        );

        let engine = crate::restore::TaskRestoreEngine::new(env.objects_root.clone());
        let plan = engine.prepare_directory_undo(&env.db, &task_id, &root).unwrap();
        let outcome = engine.execute_prepared_directory_undo(&plan);

        assert_eq!(outcome.result.files_restored, 2);
        assert_eq!(outcome.result.files_deleted, 0);
        assert_eq!(outcome.result.failures.len(), 0);
        assert_eq!(outcome.suppression_entries.len(), 2);
        assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "# overview\n");
        assert_eq!(std::fs::read_to_string(&file_b).unwrap(), "alpha spec\n");
    }

    #[test]
    fn restored_ai_deleted_directory_file_can_be_modified_in_followup_task() {
        let env = HookFlowEnv::new();
        let delete_session = "cursor:conv-shell-delete-dir-followup";
        let root = env.live_path("tmp/design_mockup");
        let restored_file = env.write_live_file("tmp/design_mockup/00-overview.md", "# overview\n");
        let file_path = restored_file.to_string_lossy().to_string();
        let deleted_hash = env.store_content("shell_delete_followup/00-overview.md", "# overview\n");
        std::fs::remove_dir_all(&root).unwrap();

        let delete_task = process_event_with_objects(
            &env.db,
            &prompt_envelope(
                "cursor",
                delete_session,
                "delete directory via shell",
                Some("conv-shell-delete-dir-followup"),
                Some("gen-shell-delete-dir-followup"),
            ),
            &env.objects_root,
        )
        .task_id
        .expect("shell delete dir task");
        env.seed_pre_tool_hash(delete_session, &file_path, &deleted_hash);

        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                delete_session,
                "execute_command",
                vec![ObservedPathChange {
                    file_path: file_path.clone(),
                    file_exists_after: false,
                    new_hash: None,
                }],
            ),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", delete_session, Some("gen-shell-delete-dir-followup")),
            &env.objects_root,
        );
        env.finalize_next_job();

        let engine = crate::restore::TaskRestoreEngine::new(env.objects_root.clone());
        let outcome = engine.execute_prepared_directory_undo(
            &engine
                .prepare_directory_undo(&env.db, &delete_task, &root)
                .unwrap(),
        );
        assert_eq!(outcome.result.files_restored, 1);
        assert_eq!(std::fs::read_to_string(&restored_file).unwrap(), "# overview\n");

        let restored_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&restored_file)
            .unwrap();
        let edit_session = "cursor:conv-edit-restored-dir";
        let edit_task = process_event_with_objects(
            &env.db,
            &prompt_envelope(
                "cursor",
                edit_session,
                "edit restored file",
                Some("conv-edit-restored-dir"),
                Some("gen-edit-restored-dir"),
            ),
            &env.objects_root,
        )
        .task_id
        .expect("followup edit task");
        env.seed_pre_tool_hash(edit_session, &file_path, &restored_hash);

        std::fs::write(&restored_file, "# overview updated\n").unwrap();
        let new_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&restored_file)
            .unwrap();

        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                edit_session,
                "execute_command",
                vec![ObservedPathChange {
                    file_path: file_path.clone(),
                    file_exists_after: true,
                    new_hash: Some(new_hash.clone()),
                }],
            ),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", edit_session, Some("gen-edit-restored-dir")),
            &env.objects_root,
        );
        env.finalize_next_job();

        let changes = env.db.get_changes_for_task(&edit_task).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Modified);
        assert_eq!(changes[0].old_hash.as_deref(), Some(restored_hash.as_str()));
        assert_eq!(changes[0].new_hash.as_deref(), Some(new_hash.as_str()));
        assert_eq!(changes[0].file_path, restored_file);
        assert_eq!(changes[0].attribution.as_deref(), Some("bash_predicted"));
    }

    #[test]
    fn interleaved_multi_tool_sessions_remain_isolated() {
        let env = HookFlowEnv::new();
        let cursor_session = "cursor:conv-parallel";
        let workbuddy_session = "workbuddy:wb-parallel";

        let cursor_live = env.write_live_file("parallel/cursor.md", "cursor created\n");
        let cursor_path = cursor_live.to_string_lossy().to_string();
        let cursor_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&cursor_live)
            .unwrap();

        let workbuddy_live = env.write_live_file("parallel/work.log", "delete me\n");
        let workbuddy_path = workbuddy_live.to_string_lossy().to_string();
        let workbuddy_old_hash = env.store_content("parallel/work.log.old", "delete me\n");
        env.delete_live_file("parallel/work.log");

        let cursor_task = process_event_with_objects(
            &env.db,
            &prompt_envelope(
                "cursor",
                cursor_session,
                "create cursor file",
                Some("conv-parallel"),
                Some("gen-cursor"),
            ),
            &env.objects_root,
        )
        .task_id
        .expect("cursor task");

        let workbuddy_task = process_event_with_objects(
            &env.db,
            &prompt_envelope(
                "workbuddy",
                workbuddy_session,
                "delete work file",
                None,
                Some("gen-work"),
            ),
            &env.objects_root,
        )
        .task_id
        .expect("workbuddy task");

        env.seed_pre_tool_hash(workbuddy_session, &workbuddy_path, &workbuddy_old_hash);

        process_event_with_objects(
            &env.db,
            &post_envelope(
                "workbuddy",
                workbuddy_session,
                "remove_file",
                vec![ObservedPathChange {
                    file_path: workbuddy_path.clone(),
                    file_exists_after: false,
                    new_hash: None,
                }],
            ),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                cursor_session,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: cursor_path.clone(),
                    file_exists_after: true,
                    new_hash: Some(cursor_hash.clone()),
                }],
            ),
            &env.objects_root,
        );

        process_event_with_objects(
            &env.db,
            &stop_envelope("workbuddy", workbuddy_session, Some("gen-work")),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", cursor_session, Some("gen-cursor")),
            &env.objects_root,
        );

        let finalized = env.finalize_all_jobs();
        assert_eq!(finalized.len(), 2);
        assert!(finalized.contains(&cursor_task));
        assert!(finalized.contains(&workbuddy_task));

        let cursor_changes = env.db.get_changes_for_task(&cursor_task).unwrap();
        assert_eq!(cursor_changes.len(), 1);
        assert_eq!(cursor_changes[0].file_path, PathBuf::from(&cursor_path));
        assert_eq!(cursor_changes[0].change_type, ChangeType::Created);

        let workbuddy_changes = env.db.get_changes_for_task(&workbuddy_task).unwrap();
        assert_eq!(workbuddy_changes.len(), 1);
        assert_eq!(workbuddy_changes[0].file_path, PathBuf::from(&workbuddy_path));
        assert_eq!(workbuddy_changes[0].change_type, ChangeType::Deleted);
        assert!(env.db.get_active_task_for_session(cursor_session).unwrap().is_none());
        assert!(env.db.get_active_task_for_session(workbuddy_session).unwrap().is_none());
    }

    #[test]
    fn same_tool_multiple_sessions_remain_isolated() {
        let env = HookFlowEnv::new();
        let session_a = "cursor:conv-a";
        let session_b = "cursor:conv-b";

        let path_a_live = env.write_live_file("multi/a.md", "a one\n");
        let path_b_live = env.write_live_file("multi/b.md", "b one\n");
        let path_a = path_a_live.to_string_lossy().to_string();
        let path_b = path_b_live.to_string_lossy().to_string();
        let hash_a = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&path_a_live)
            .unwrap();
        let hash_b = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&path_b_live)
            .unwrap();

        let task_a = process_event_with_objects(
            &env.db,
            &prompt_envelope("cursor", session_a, "task a", Some("conv-a"), Some("gen-a")),
            &env.objects_root,
        )
        .task_id
        .expect("task a");
        let task_b = process_event_with_objects(
            &env.db,
            &prompt_envelope("cursor", session_b, "task b", Some("conv-b"), Some("gen-b")),
            &env.objects_root,
        )
        .task_id
        .expect("task b");

        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                session_b,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: path_b.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_b.clone()),
                }],
            ),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                session_a,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: path_a.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_a.clone()),
                }],
            ),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_b, Some("gen-b")),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_a, Some("gen-a")),
            &env.objects_root,
        );
        env.finalize_all_jobs();

        let changes_a = env.db.get_changes_for_task(&task_a).unwrap();
        let changes_b = env.db.get_changes_for_task(&task_b).unwrap();
        assert_eq!(changes_a.len(), 1);
        assert_eq!(changes_b.len(), 1);
        assert_eq!(changes_a[0].file_path, PathBuf::from(&path_a));
        assert_eq!(changes_b[0].file_path, PathBuf::from(&path_b));
        assert_ne!(task_a, task_b);
    }

    #[test]
    fn same_file_changes_with_hook_stay_on_respective_tasks() {
        let env = HookFlowEnv::new();
        let file_path = env.write_live_file("shared/file.md", "v1\n");
        let file_path_str = file_path.to_string_lossy().to_string();
        let initial_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&file_path)
            .unwrap();
        env.db
            .upsert_live_file_index_entry(
                &file_path_str,
                1,
                &initial_hash,
                Some(&initial_hash),
                "test",
                "seed",
                &Utc::now().to_rfc3339(),
                Some(1),
            )
            .unwrap();

        let session_a = "codebuddy:shared-a";
        let session_b = "workbuddy:shared-b";
        let task_a = process_event_with_objects(
            &env.db,
            &prompt_envelope("codebuddy", session_a, "edit shared file A", None, Some("gen-a")),
            &env.objects_root,
        )
        .task_id
        .expect("task a");

        std::fs::write(&file_path, "v2 from a\n").unwrap();
        let hash_a = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&file_path)
            .unwrap();
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "codebuddy",
                session_a,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: file_path_str.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_a.clone()),
                }],
            ),
            &env.objects_root,
        );

        let task_b = process_event_with_objects(
            &env.db,
            &prompt_envelope("workbuddy", session_b, "edit shared file B", None, Some("gen-b")),
            &env.objects_root,
        )
        .task_id
        .expect("task b");

        std::fs::write(&file_path, "v3 from b\n").unwrap();
        let hash_b = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&file_path)
            .unwrap();
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "workbuddy",
                session_b,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: file_path_str.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_b.clone()),
                }],
            ),
            &env.objects_root,
        );

        process_event_with_objects(
            &env.db,
            &stop_envelope("codebuddy", session_a, Some("gen-a")),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("workbuddy", session_b, Some("gen-b")),
            &env.objects_root,
        );
        env.finalize_all_jobs();

        let changes_a = env.db.get_changes_for_task(&task_a).unwrap();
        let changes_b = env.db.get_changes_for_task(&task_b).unwrap();
        assert_eq!(changes_a.len(), 1);
        assert_eq!(changes_b.len(), 1);
        assert_eq!(changes_a[0].file_path, PathBuf::from(&file_path_str));
        assert_eq!(changes_b[0].file_path, PathBuf::from(&file_path_str));
        assert_eq!(changes_a[0].change_type, ChangeType::Modified);
        assert_eq!(changes_b[0].change_type, ChangeType::Modified);
        assert_ne!(changes_a[0].task_id, changes_b[0].task_id);
        assert!(changes_a[0].new_hash.is_some());
        assert!(changes_b[0].new_hash.is_some());
    }

    #[test]
    fn multi_tool_same_file_pre_tool_hashes_remain_isolated() {
        let env = HookFlowEnv::new();
        let live_path = env.write_live_file("shared/pretool.md", "base\n");
        let file_path = live_path.to_string_lossy().to_string();
        let base_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();

        let session_a = "cursor:pretool-a";
        let session_b = "workbuddy:pretool-b";
        let task_a = process_event_with_objects(
            &env.db,
            &prompt_envelope("cursor", session_a, "edit shared file A", Some("conv-a"), Some("gen-a")),
            &env.objects_root,
        )
        .task_id
        .expect("task a");
        let task_b = process_event_with_objects(
            &env.db,
            &prompt_envelope("workbuddy", session_b, "edit shared file B", None, Some("gen-b")),
            &env.objects_root,
        )
        .task_id
        .expect("task b");

        env.seed_pre_tool_hash(session_a, &file_path, &base_hash);
        std::fs::write(&live_path, "from a\n").unwrap();
        let hash_a = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                session_a,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: file_path.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_a.clone()),
                }],
            ),
            &env.objects_root,
        );

        env.seed_pre_tool_hash(session_b, &file_path, &hash_a);
        std::fs::write(&live_path, "from b\n").unwrap();
        let hash_b = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "workbuddy",
                session_b,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: file_path.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_b.clone()),
                }],
            ),
            &env.objects_root,
        );

        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_a, Some("gen-a")),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("workbuddy", session_b, Some("gen-b")),
            &env.objects_root,
        );
        env.finalize_all_jobs();

        let changes_a = env.db.get_changes_for_task(&task_a).unwrap();
        let changes_b = env.db.get_changes_for_task(&task_b).unwrap();
        assert_eq!(changes_a[0].old_hash.as_deref(), Some(base_hash.as_str()));
        assert_eq!(changes_b[0].old_hash.as_deref(), Some(hash_a.as_str()));
        assert!(changes_a[0].new_hash.is_some());
        assert!(changes_b[0].new_hash.is_some());
    }

    #[test]
    fn same_tool_same_file_pre_tool_hashes_remain_isolated() {
        let env = HookFlowEnv::new();
        let live_path = env.write_live_file("shared/cursor-pretool.md", "base\n");
        let file_path = live_path.to_string_lossy().to_string();
        let base_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();

        let session_a = "cursor:shared-a";
        let session_b = "cursor:shared-b";
        let task_a = process_event_with_objects(
            &env.db,
            &prompt_envelope("cursor", session_a, "cursor A", Some("conv-a"), Some("gen-a")),
            &env.objects_root,
        )
        .task_id
        .expect("task a");
        let task_b = process_event_with_objects(
            &env.db,
            &prompt_envelope("cursor", session_b, "cursor B", Some("conv-b"), Some("gen-b")),
            &env.objects_root,
        )
        .task_id
        .expect("task b");

        env.seed_pre_tool_hash(session_a, &file_path, &base_hash);
        std::fs::write(&live_path, "cursor a\n").unwrap();
        let hash_a = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                session_a,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: file_path.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_a.clone()),
                }],
            ),
            &env.objects_root,
        );

        env.seed_pre_tool_hash(session_b, &file_path, &hash_a);
        std::fs::write(&live_path, "cursor b\n").unwrap();
        let hash_b = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();
        process_event_with_objects(
            &env.db,
            &post_envelope(
                "cursor",
                session_b,
                "write_to_file",
                vec![ObservedPathChange {
                    file_path: file_path.clone(),
                    file_exists_after: true,
                    new_hash: Some(hash_b.clone()),
                }],
            ),
            &env.objects_root,
        );

        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_a, Some("gen-a")),
            &env.objects_root,
        );
        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_b, Some("gen-b")),
            &env.objects_root,
        );
        env.finalize_all_jobs();

        let changes_a = env.db.get_changes_for_task(&task_a).unwrap();
        let changes_b = env.db.get_changes_for_task(&task_b).unwrap();
        assert_eq!(changes_a[0].old_hash.as_deref(), Some(base_hash.as_str()));
        assert_eq!(changes_b[0].old_hash.as_deref(), Some(hash_a.as_str()));
        assert!(changes_a[0].new_hash.is_some());
        assert!(changes_b[0].new_hash.is_some());
    }

    #[test]
    fn stop_cleans_session_pre_tool_store() {
        let env = HookFlowEnv::new();
        let live_path = env.write_live_file("shared/cleanup.md", "before\n");
        let file_path = live_path.to_string_lossy().to_string();
        let base_hash = ObjectStore::new(env.objects_root.clone())
            .unwrap()
            .store(&live_path)
            .unwrap();
        let session = "cursor:cleanup";
        process_event_with_objects(
            &env.db,
            &prompt_envelope("cursor", session, "cleanup test", Some("conv-clean"), Some("gen-clean")),
            &env.objects_root,
        );
        env.seed_pre_tool_hash(session, &file_path, &base_hash);
        assert_eq!(
            get_pre_tool_hash_in(&env.pre_tool_root(), session, &file_path)
                .unwrap()
                .as_deref(),
            Some(base_hash.as_str())
        );

        process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session, Some("gen-clean")),
            &env.objects_root,
        );

        assert!(
            get_pre_tool_hash_in(&env.pre_tool_root(), session, &file_path)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn stop_guard_mismatch_does_not_finalize_session() {
        let env = HookFlowEnv::new();
        let session_key = "cursor:conv-guard";

        let task_id = process_event_with_objects(
            &env.db,
            &prompt_envelope(
                "cursor",
                session_key,
                "guarded task",
                Some("conv-guard"),
                Some("gen-expected"),
            ),
            &env.objects_root,
        )
        .task_id
        .expect("guard task");

        let wrong_stop = process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_key, Some("gen-wrong")),
            &env.objects_root,
        );
        assert!(wrong_stop.task_id.is_none());
        assert!(env.db.claim_next_task_finalization_job().unwrap().is_none());
        assert_eq!(
            env.db.get_active_task_for_session(session_key).unwrap().as_deref(),
            Some(task_id.as_str())
        );

        let right_stop = process_event_with_objects(
            &env.db,
            &stop_envelope("cursor", session_key, Some("gen-expected")),
            &env.objects_root,
        );
        assert_eq!(right_stop.task_id.as_deref(), Some(task_id.as_str()));
        assert_eq!(env.finalize_next_job(), task_id);
    }

    #[test]
    fn spool_claims_oldest_event_in_timestamp_order() {
        let dir = tempdir().unwrap();
        ensure_hook_spool_dirs_in(dir.path()).unwrap();
        let pending = hook_spool_state_dir_in(dir.path(), "pending");

        let second = prompt_envelope("cursor", "cursor:later", "later", Some("later"), Some("gen2"));
        let first = prompt_envelope("cursor", "cursor:earlier", "earlier", Some("earlier"), Some("gen1"));
        std::fs::write(
            pending.join("00000000000000000002_second.json"),
            serde_json::to_vec(&second).unwrap(),
        )
        .unwrap();
        std::fs::write(
            pending.join("00000000000000000001_first.json"),
            serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();

        let (_, claimed_first) = claim_oldest_hook_event_in(dir.path()).unwrap().unwrap();
        let (_, claimed_second) = claim_oldest_hook_event_in(dir.path()).unwrap().unwrap();
        assert_eq!(claimed_first.idempotency_key, first.idempotency_key);
        assert_eq!(claimed_second.idempotency_key, second.idempotency_key);
    }

    #[test]
    fn spool_requeues_processing_files_after_crash() {
        let dir = tempdir().unwrap();
        ensure_hook_spool_dirs_in(dir.path()).unwrap();
        let processing = hook_spool_state_dir_in(dir.path(), "processing");
        let pending = hook_spool_state_dir_in(dir.path(), "pending");
        let envelope = prompt_envelope("cursor", "cursor:requeue", "requeue", Some("conv"), Some("gen"));
        let processing_file = processing.join("00000000000000000001_event.json");
        std::fs::write(&processing_file, serde_json::to_vec(&envelope).unwrap()).unwrap();

        let moved = requeue_hook_spool_processing_files_in(dir.path()).unwrap();
        assert_eq!(moved, 1);
        assert!(!processing_file.exists());
        assert!(pending.join("00000000000000000001_event.json").exists());
    }

    #[test]
    fn spool_invalid_json_moves_to_failed_with_sidecar() {
        let dir = tempdir().unwrap();
        ensure_hook_spool_dirs_in(dir.path()).unwrap();
        let pending = hook_spool_state_dir_in(dir.path(), "pending");
        let invalid = pending.join("00000000000000000001_invalid.json");
        std::fs::write(&invalid, b"{not valid json").unwrap();

        assert!(claim_oldest_hook_event_in(dir.path()).is_err());
        let failed = hook_spool_state_dir_in(dir.path(), "failed")
            .join("00000000000000000001_invalid.json");
        assert!(failed.exists());
        let sidecar = failed.with_extension("error.txt");
        assert!(sidecar.exists());
        let message = std::fs::read_to_string(sidecar).unwrap();
        assert!(!message.is_empty());
    }

    #[test]
    fn append_hook_event_concurrently_produces_unique_files() {
        let dir = tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(5));
        let mut handles = Vec::new();

        for idx in 0..5 {
            let barrier = Arc::clone(&barrier);
            let rew_home = dir.path().to_path_buf();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let envelope = prompt_envelope(
                    "cursor",
                    &format!("cursor:thread-{idx}"),
                    &format!("prompt-{idx}"),
                    Some("conv"),
                    Some("gen"),
                );
                append_hook_event_in(&rew_home, &envelope).unwrap()
            }));
        }

        let paths: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let unique: HashSet<PathBuf> = paths.iter().cloned().collect();
        assert_eq!(unique.len(), paths.len());
        assert_eq!(
            std::fs::read_dir(hook_spool_state_dir_in(dir.path(), "pending"))
                .unwrap()
                .count(),
            5
        );
    }
}
