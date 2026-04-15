//! Restore engine for recovering files from APFS snapshots.
//!
//! Supports both dry-run mode (preview what would change) and actual restoration.
//! Before any actual restore, automatically creates a safety snapshot of the
//! current state as insurance.

use crate::db::Database;
use crate::error::{RewError, RewResult};
use crate::types::{RestoreJob, Snapshot, SnapshotTrigger};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

/// Default timeout for tmutil restore commands (60 seconds).
const TMUTIL_RESTORE_TIMEOUT_SECS: u64 = 60;

/// Maximum retry attempts for tmutil restore on transient failures.
const TMUTIL_RESTORE_MAX_RETRIES: u32 = 2;

/// Result of a dry-run restore preview.
#[derive(Debug, Clone)]
pub struct RestorePreview {
    /// Snapshot being restored from.
    pub snapshot: Snapshot,
    /// Files that would be restored (existed at snapshot time but not now).
    pub files_to_restore: Vec<PathBuf>,
    /// Files that exist now but were created after the snapshot (would be removed in full restore).
    pub files_to_remove: Vec<PathBuf>,
    /// Files that would be overwritten with snapshot-time versions.
    pub files_to_overwrite: Vec<PathBuf>,
    /// Total estimated size of files to restore in bytes.
    pub estimated_size_bytes: u64,
}

/// Abstraction over snapshot capabilities needed by the restore engine.
///
/// This decouples RestoreEngine from any specific snapshot implementation
/// (MacOS, Windows VSS, etc.), making it testable and extensible.
pub trait RestoreSnapshotProvider {
    /// Look up a snapshot by ID.
    fn get_snapshot(&self, id: &uuid::Uuid) -> RewResult<Option<Snapshot>>;

    /// Create a safety snapshot (best-effort, for insurance before restore).
    fn create_safety_snapshot(&self) -> RewResult<Snapshot>;

    /// Restore a file/directory from a snapshot to a destination.
    /// Returns the destination path on success.
    fn restore_path_from_snapshot(
        &self,
        snapshot: &Snapshot,
        source_path: &Path,
        dest_path: &Path,
    ) -> RewResult<PathBuf>;
}

/// Blanket implementation for any Database + SnapshotEngine combo.
/// Concrete adapters are defined below.
pub struct SnapshotProviderAdapter<'a> {
    db: &'a Database,
    /// Closure that creates a snapshot. Returns the created Snapshot on success.
    create_fn: Box<dyn Fn() -> RewResult<Snapshot> + 'a>,
    /// Closure that restores a path from a snapshot.
    restore_fn: Box<dyn Fn(&Snapshot, &Path, &Path) -> RewResult<PathBuf> + 'a>,
}

impl<'a> SnapshotProviderAdapter<'a> {
    /// Create a provider backed by a Database and closures for snapshot creation and restore.
    pub fn new(
        db: &'a Database,
        create_fn: impl Fn() -> RewResult<Snapshot> + 'a,
        restore_fn: impl Fn(&Snapshot, &Path, &Path) -> RewResult<PathBuf> + 'a,
    ) -> Self {
        Self {
            db,
            create_fn: Box::new(create_fn),
            restore_fn: Box::new(restore_fn),
        }
    }
}

impl<'a> RestoreSnapshotProvider for SnapshotProviderAdapter<'a> {
    fn get_snapshot(&self, id: &uuid::Uuid) -> RewResult<Option<Snapshot>> {
        self.db.get_snapshot(id)
    }

    fn create_safety_snapshot(&self) -> RewResult<Snapshot> {
        (self.create_fn)()
    }

    fn restore_path_from_snapshot(
        &self,
        snapshot: &Snapshot,
        source_path: &Path,
        dest_path: &Path,
    ) -> RewResult<PathBuf> {
        (self.restore_fn)(snapshot, source_path, dest_path)
    }
}

/// Restore engine that handles file recovery from APFS snapshots.
///
/// Key safety features:
/// - Pre-restore safety snapshot (insurance against bad restores)
/// - Dry-run mode for previewing changes
/// - Validates snapshot existence before attempting restore
/// - Timeout and retry for tmutil commands
pub struct RestoreEngine<P: RestoreSnapshotProvider> {
    provider: P,
    /// Timeout for tmutil restore operations.
    restore_timeout: Duration,
    /// Max retries for transient tmutil failures.
    max_retries: u32,
}

impl<P: RestoreSnapshotProvider> RestoreEngine<P> {
    /// Create a new RestoreEngine backed by the given snapshot provider.
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            restore_timeout: Duration::from_secs(TMUTIL_RESTORE_TIMEOUT_SECS),
            max_retries: TMUTIL_RESTORE_MAX_RETRIES,
        }
    }

    /// Set a custom timeout for tmutil restore operations.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.restore_timeout = timeout;
        self
    }

    /// Set custom max retries for tmutil restore.
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Execute a restore job.
    ///
    /// If `job.dry_run` is true, returns a preview of what would change.
    /// If `job.dry_run` is false, creates a safety snapshot first, then restores.
    pub fn restore(&self, job: &RestoreJob) -> RewResult<RestorePreview> {
        // Look up the snapshot
        let snapshot = self
            .provider
            .get_snapshot(&job.snapshot_id)?
            .ok_or_else(|| {
                RewError::Snapshot(format!("Snapshot {} not found", job.snapshot_id))
            })?;

        info!(
            "Restore requested: snapshot={}, dry_run={}, target_paths={}",
            snapshot.id,
            job.dry_run,
            job.target_paths.len()
        );

        if job.dry_run {
            self.preview_restore(&snapshot, &job.target_paths)
        } else {
            self.execute_restore(&snapshot, &job.target_paths)
        }
    }

    /// Preview what a restore would do without making any changes.
    ///
    /// Compares the current filesystem state against the snapshot timestamp
    /// to classify each target path into one of three categories:
    /// - `files_to_restore`: paths that don't exist now but likely existed at snapshot time
    /// - `files_to_remove`: files created after the snapshot (would be removed in full restore)
    /// - `files_to_overwrite`: files that exist now and existed at snapshot time (version revert)
    fn preview_restore(
        &self,
        snapshot: &Snapshot,
        target_paths: &[PathBuf],
    ) -> RewResult<RestorePreview> {
        info!("Running dry-run restore preview for snapshot {}", snapshot.id);

        let mut files_to_restore = Vec::new();
        let mut files_to_remove = Vec::new();
        let mut files_to_overwrite = Vec::new();
        let mut estimated_size_bytes = 0u64;

        let snapshot_time = snapshot.timestamp;

        // If specific target paths given, scope the preview to those
        let scan_paths = if target_paths.is_empty() {
            warn!("No target paths specified for dry-run; preview may be incomplete");
            vec![]
        } else {
            target_paths.to_vec()
        };

        // Scan current filesystem state for each target path
        for path in &scan_paths {
            if path.exists() {
                if path.is_file() {
                    self.classify_file(
                        path,
                        &snapshot_time,
                        &mut files_to_restore,
                        &mut files_to_remove,
                        &mut files_to_overwrite,
                        &mut estimated_size_bytes,
                    );
                } else if path.is_dir() {
                    // Walk the directory and classify each file
                    match Self::walk_dir(path) {
                        Ok(entries) => {
                            for entry in entries {
                                self.classify_file(
                                    &entry,
                                    &snapshot_time,
                                    &mut files_to_restore,
                                    &mut files_to_remove,
                                    &mut files_to_overwrite,
                                    &mut estimated_size_bytes,
                                );
                            }
                        }
                        Err(e) => {
                            warn!("Failed to walk directory {:?}: {}", path, e);
                        }
                    }
                }
            } else {
                // Path doesn't exist now but may have existed at snapshot time —
                // restoring would bring it back.
                files_to_restore.push(path.clone());
            }
        }

        Ok(RestorePreview {
            snapshot: snapshot.clone(),
            files_to_restore,
            files_to_remove,
            files_to_overwrite,
            estimated_size_bytes,
        })
    }

    /// Classify a single file relative to the snapshot timestamp.
    ///
    /// Uses the file's modification time to determine:
    /// - Modified after snapshot → it will be overwritten (revert to snapshot version)
    /// - Created after snapshot → it would be removed (didn't exist at snapshot time)
    /// - Modified before snapshot → already matches snapshot state (still counted as overwrite
    ///   since tmutil restore will rewrite it)
    fn classify_file(
        &self,
        path: &Path,
        snapshot_time: &chrono::DateTime<chrono::Utc>,
        _files_to_restore: &mut Vec<PathBuf>,
        files_to_remove: &mut Vec<PathBuf>,
        files_to_overwrite: &mut Vec<PathBuf>,
        estimated_size_bytes: &mut u64,
    ) {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                warn!("Cannot stat {:?}: {}", path, e);
                return;
            }
        };

        let file_size = meta.len();

        // Check if the file was created after the snapshot
        // If creation time is after the snapshot, this file didn't exist at snapshot time
        // and would be removed during a full restore.
        let created_after_snapshot = meta
            .created()
            .ok()
            .and_then(|ct| {
                let created: chrono::DateTime<chrono::Utc> = ct.into();
                Some(created > *snapshot_time)
            })
            .unwrap_or(false);

        if created_after_snapshot {
            // File was created after snapshot — restoring would remove it
            files_to_remove.push(path.to_path_buf());
            // Don't count removed files toward restore size
        } else {
            // File existed at snapshot time — restoring would overwrite with snapshot version
            files_to_overwrite.push(path.to_path_buf());
            *estimated_size_bytes += file_size;
        }
    }

    /// Execute the actual restore.
    ///
    /// 1. Creates a safety snapshot of the current state
    /// 2. Runs `tmutil restore` to restore files from the target snapshot
    fn execute_restore(
        &self,
        snapshot: &Snapshot,
        target_paths: &[PathBuf],
    ) -> RewResult<RestorePreview> {
        info!(
            "Executing restore from snapshot {} (os_ref: {})",
            snapshot.id, snapshot.os_snapshot_ref
        );

        // Step 1: Create a safety snapshot as insurance
        info!("Creating pre-restore safety snapshot...");
        match self.provider.create_safety_snapshot() {
            Ok(safety) => {
                info!(
                    "Safety snapshot created: {} (os_ref: {})",
                    safety.id, safety.os_snapshot_ref
                );
            }
            Err(e) => {
                warn!(
                    "Failed to create safety snapshot (continuing with restore): {}",
                    e
                );
            }
        }

        // Step 2: Build preview first (for return value)
        let preview = self.preview_restore(snapshot, target_paths)?;

        // Step 3: Validate we have target paths
        if target_paths.is_empty() {
            return Err(RewError::Snapshot(
                "Cannot execute full-volume restore without specific target paths. \
                 Please specify which files or directories to restore."
                    .to_string(),
            ));
        }

        // Step 4: Execute the restore via tmutil with retry
        for target in target_paths {
            self.restore_path_with_retry(snapshot, target)?;
        }

        info!("Restore completed successfully from snapshot {}", snapshot.id);
        Ok(preview)
    }

    /// Restore a single path with retry logic for transient failures.
    fn restore_path_with_retry(&self, snapshot: &Snapshot, target: &Path) -> RewResult<()> {
        let mut last_err = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                info!(
                    "Retrying restore for {:?} (attempt {}/{})",
                    target,
                    attempt + 1,
                    self.max_retries + 1
                );
            }

            match self.provider.restore_path_from_snapshot(snapshot, target, target) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    // Only retry on potentially transient errors
                    let is_transient = matches!(&e, RewError::Io(_))
                        || e.to_string().contains("temporarily unavailable")
                        || e.to_string().contains("resource busy");

                    if !is_transient || attempt == self.max_retries {
                        return Err(e);
                    }

                    warn!("Transient error restoring {:?}: {} (will retry)", target, e);
                    last_err = Some(e);
                    // Brief pause before retry
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            RewError::Snapshot(format!(
                "Restore failed for {:?} after {} retries",
                target,
                self.max_retries
            ))
        }))
    }

    /// Recursively walk a directory and return all file paths.
    fn walk_dir(dir: &Path) -> RewResult<Vec<PathBuf>> {
        let mut files = Vec::new();
        Self::walk_dir_recursive(dir, &mut files)?;
        Ok(files)
    }

    fn walk_dir_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> RewResult<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::walk_dir_recursive(&path, files)?;
            } else {
                files.push(path);
            }
        }
        Ok(())
    }
}

/// Convenience constructor for RestoreEngine using MacOSSnapshotEngine.
///
/// This preserves backward compatibility with the existing API while using
/// the trait-based decoupled design internally.
pub mod compat {
    use super::*;
    use crate::snapshot::macos::MacOSSnapshotEngine;

    /// Create a RestoreEngine from a MacOSSnapshotEngine reference.
    pub fn restore_engine_from_macos(
        engine: &MacOSSnapshotEngine,
    ) -> RestoreEngine<SnapshotProviderAdapter<'_>> {
        let provider = SnapshotProviderAdapter::new(
            engine.db(),
            || engine.create(SnapshotTrigger::Auto, 0, 0, 0),
            |snapshot, source, dest| {
                engine
                    .tmutil()
                    .restore_from_snapshot(&snapshot.os_snapshot_ref, source, dest)
            },
        );
        RestoreEngine::new(provider)
    }
}

// ================================================================
// Task-level restore engine (V2)
// ================================================================

/// Preview of what an undo operation would do.
#[derive(Debug, Clone)]
pub struct UndoPreview {
    /// Task being undone
    pub task_id: String,
    /// Files that would be restored to previous version
    pub files_to_restore: Vec<PathBuf>,
    /// Files that would be deleted (were created by this task)
    pub files_to_delete: Vec<PathBuf>,
    /// Files that would be renamed back
    pub files_to_rename: Vec<(PathBuf, PathBuf)>,
    /// Total number of changes
    pub total_changes: usize,
}

/// Result of an undo operation.
#[derive(Debug, Clone)]
pub struct UndoResult {
    /// How many files were successfully restored
    pub files_restored: usize,
    /// How many files were deleted (created files removed)
    pub files_deleted: usize,
    /// Failures: (path, error)
    pub failures: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone)]
pub struct PreparedDirectoryUndoPlan {
    pub task_id: String,
    pub total_task_changes: usize,
    pub scoped_change_paths: Vec<PathBuf>,
    operations: Vec<PreparedUndoChange>,
}

#[derive(Debug, Clone)]
pub struct PreparedDirectoryUndoOutcome {
    pub result: UndoResult,
    pub restored_change_paths: Vec<PathBuf>,
    pub suppression_entries: Vec<PreparedSuppressionEntry>,
}

#[derive(Debug, Clone)]
pub struct PreparedDirectoryUndoProgress {
    pub total_files: usize,
    pub processed_files: usize,
    pub restored_files: usize,
    pub deleted_files: usize,
    pub failed_files: usize,
    pub current_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct PreparedUndoChange {
    change_file_path: PathBuf,
    action: PreparedUndoAction,
}

#[derive(Debug, Clone)]
pub struct PreparedSuppressionEntry {
    pub path: PathBuf,
    pub expected_content_hash: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone)]
struct ResolvedRestoreHashes {
    candidates: Vec<String>,
    expected_content_hash: Option<String>,
}

#[derive(Debug, Clone)]
enum PreparedUndoAction {
    DeleteCurrent {
        target_path: PathBuf,
    },
    RestoreObject {
        restore_hashes: Vec<String>,
        restore_target: PathBuf,
        delete_after_restore: Option<PathBuf>,
        expected_content_hash: Option<String>,
    },
    Fail {
        error: String,
    },
}

/// Task-level restore engine.
///
/// Works with `ObjectStore` (content-addressable backup) and `Database`
/// to undo changes at the task granularity.
///
/// Restore strategy per change type:
/// - **Created** → delete the file (it didn't exist before the task)
/// - **Modified** → restore from `.rew/objects/{old_hash}`
/// - **Deleted** → restore from `.rew/objects/{old_hash}`
/// - **Renamed** → rename back (not yet implemented, treated as modified)
pub struct TaskRestoreEngine {
    objects_root: PathBuf,
    cleanup_boundaries: Vec<PathBuf>,
}

impl TaskRestoreEngine {
    /// Create a new TaskRestoreEngine.
    /// `objects_root` is typically `.rew/objects/`.
    pub fn new(objects_root: PathBuf) -> Self {
        Self {
            objects_root,
            cleanup_boundaries: Vec::new(),
        }
    }

    /// Register ancestor directories that must never be deleted by empty-dir cleanup.
    pub fn with_cleanup_boundaries(mut self, cleanup_boundaries: Vec<PathBuf>) -> Self {
        self.cleanup_boundaries = cleanup_boundaries;
        self
    }

    /// Preview what an undo would do without making changes.
    pub fn preview_undo(
        &self,
        db: &crate::db::Database,
        task_id: &str,
    ) -> RewResult<UndoPreview> {
        let _task = db
            .get_task(task_id)?
            .ok_or_else(|| RewError::Snapshot(format!("Task '{}' not found", task_id)))?;

        let changes: Vec<_> = db.get_changes_for_task(task_id)?
            .into_iter()
            .filter(|c| !is_temp_file(&c.file_path))
            .collect();

        let mut files_to_restore = Vec::new();
        let mut files_to_delete = Vec::new();
        let files_to_rename = Vec::new();

        for change in &changes {
            match change.change_type {
                crate::types::ChangeType::Created => {
                    files_to_delete.push(change.file_path.clone());
                }
                crate::types::ChangeType::Modified | crate::types::ChangeType::Deleted => {
                    if change.old_hash.is_some() {
                        files_to_restore.push(change.file_path.clone());
                    }
                }
                crate::types::ChangeType::Renamed => {
                    if let Some(ref old_path) = change.old_file_path {
                        files_to_restore.push(old_path.clone());
                        files_to_delete.push(change.file_path.clone());
                    } else if change.old_hash.is_some() {
                        files_to_restore.push(change.file_path.clone());
                    }
                }
            }
        }

        Ok(UndoPreview {
            task_id: task_id.to_string(),
            total_changes: changes.len(),
            files_to_restore,
            files_to_delete,
            files_to_rename,
        })
    }

    /// Execute undo for an entire task.
    ///
    /// Processes changes in reverse order (last change first).
    /// On success, marks the task as `RolledBack`.
    pub fn undo_task(
        &self,
        db: &crate::db::Database,
        task_id: &str,
    ) -> RewResult<UndoResult> {
        let _task = db
            .get_task(task_id)?
            .ok_or_else(|| RewError::Snapshot(format!("Task '{}' not found", task_id)))?;

        let mut changes = db.get_changes_for_task(task_id)?;

        // Strip temp/system files that should never be restored.
        // These can appear in old DB records created before the filter was added.
        changes.retain(|c| !is_temp_file(&c.file_path));

        // Deduplicate: for the same file with multiple changes in one batch,
        // keep only the one that matters most for undo.
        // Priority: Deleted (with old_hash) > Modified (with old_hash) > Created > others
        let mut seen: std::collections::HashMap<PathBuf, crate::types::Change> = std::collections::HashMap::new();
        for change in &changes {
            if let Some(existing) = seen.get(&change.file_path) {
                // Decide which change to keep
                let dominated = match (&change.change_type, &existing.change_type) {
                    // Deleted/Renamed with old_hash always wins — we can restore the file
                    (crate::types::ChangeType::Deleted, _) if change.old_hash.is_some() => true,
                    (crate::types::ChangeType::Renamed, _) if change.old_hash.is_some() => true,
                    // Modified with old_hash beats Created
                    (crate::types::ChangeType::Modified, crate::types::ChangeType::Created) if change.old_hash.is_some() => true,
                    // Keep existing otherwise
                    _ => false,
                };
                if dominated {
                    seen.insert(change.file_path.clone(), change.clone());
                }
            } else {
                seen.insert(change.file_path.clone(), change.clone());
            }
        }

        changes = seen.into_values().collect();
        changes.sort_by_key(|change| std::cmp::Reverse(path_depth(&change.file_path)));

        let mut files_restored = 0;
        let mut files_deleted = 0;
        let mut failures = Vec::new();

        for change in &changes {
            match self.undo_single_change(db, change) {
                Ok((UndoAction::Restored, suppression_entries)) => {
                    files_restored += 1;
                    apply_file_index_updates_after_restore(db, &suppression_entries);
                }
                Ok((UndoAction::Deleted, suppression_entries)) => {
                    files_deleted += 1;
                    apply_file_index_updates_after_restore(db, &suppression_entries);
                }
                Err(e) => {
                    failures.push((change.file_path.clone(), e));
                }
            }
        }

        // Update task status — keep completed_at unchanged so the archive's
        // original timestamp is preserved in the timeline (the rollback time
        // would otherwise overwrite it and show a confusing display time).
        if failures.is_empty() {
            db.update_task_status(
                task_id,
                &crate::types::TaskStatus::RolledBack,
                None,
            )?;
        } else if files_restored > 0 || files_deleted > 0 {
            db.update_task_status(
                task_id,
                &crate::types::TaskStatus::PartialRolledBack,
                None,
            )?;
        }

        info!(
            "Undo task '{}': restored={}, deleted={}, failures={}",
            task_id, files_restored, files_deleted, failures.len()
        );

        Ok(UndoResult {
            files_restored,
            files_deleted,
            failures,
        })
    }

    /// Execute undo for all changes under a specific directory within a task.
    pub fn undo_directory(
        &self,
        db: &crate::db::Database,
        task_id: &str,
        dir_path: &Path,
    ) -> RewResult<UndoResult> {
        let plan = self.prepare_directory_undo(db, task_id, dir_path)?;
        let outcome = self.execute_prepared_directory_undo(&plan);

        if outcome.result.failures.is_empty() {
            let new_status = if plan.scoped_change_paths.len() == plan.total_task_changes {
                crate::types::TaskStatus::RolledBack
            } else {
                crate::types::TaskStatus::PartialRolledBack
            };
            db.update_task_status(task_id, &new_status, None)?;
        } else if outcome.result.files_restored > 0 || outcome.result.files_deleted > 0 {
            db.update_task_status(
                task_id,
                &crate::types::TaskStatus::PartialRolledBack,
                None,
            )?;
        }

        Ok(outcome.result)
    }

    pub fn prepare_directory_undo(
        &self,
        db: &crate::db::Database,
        task_id: &str,
        dir_path: &Path,
    ) -> RewResult<PreparedDirectoryUndoPlan> {
        let _task = db
            .get_task(task_id)?
            .ok_or_else(|| RewError::Snapshot(format!("Task '{}' not found", task_id)))?;

        let mut scoped_changes = db.get_changes_for_task_in_dir_for_restore(
            task_id,
            &dir_path.to_string_lossy(),
        )?;
        scoped_changes.retain(|c| !is_temp_file(&c.file_path));

        if scoped_changes.is_empty() {
            return Err(RewError::Snapshot(format!(
                "Task '{}' has no changes under '{}'",
                task_id,
                dir_path.display()
            )));
        }

        scoped_changes.sort_by_key(|change| std::cmp::Reverse(path_depth(&change.file_path)));

        let operations = scoped_changes
            .iter()
            .map(|change| self.prepare_undo_change(db, change))
            .collect::<Vec<_>>();
        let scoped_change_paths = scoped_changes
            .iter()
            .map(|change| change.file_path.clone())
            .collect::<Vec<_>>();
        let total_task_changes = db.count_changes_for_task(task_id)?;

        Ok(PreparedDirectoryUndoPlan {
            task_id: task_id.to_string(),
            total_task_changes,
            scoped_change_paths,
            operations,
        })
    }

    pub fn execute_prepared_directory_undo(
        &self,
        plan: &PreparedDirectoryUndoPlan,
    ) -> PreparedDirectoryUndoOutcome {
        self.execute_prepared_directory_undo_with_progress(plan, |_progress| {})
    }

    pub fn execute_prepared_directory_undo_with_progress<F>(
        &self,
        plan: &PreparedDirectoryUndoPlan,
        mut on_progress: F,
    ) -> PreparedDirectoryUndoOutcome
    where
        F: FnMut(PreparedDirectoryUndoProgress),
    {
        let mut files_restored = 0;
        let mut files_deleted = 0;
        let mut failures = Vec::new();
        let mut restored_change_paths = Vec::new();
        let mut suppression_entries = Vec::new();
        let total_files = plan.operations.len();

        on_progress(PreparedDirectoryUndoProgress {
            total_files,
            processed_files: 0,
            restored_files: 0,
            deleted_files: 0,
            failed_files: 0,
            current_path: None,
        });

        for (idx, prepared) in plan.operations.iter().enumerate() {
            match self.execute_prepared_undo_change(prepared) {
                Ok(UndoAction::Restored) => {
                    files_restored += 1;
                    restored_change_paths.push(prepared.change_file_path.clone());
                    suppression_entries.extend(prepared.suppression_entries());
                }
                Ok(UndoAction::Deleted) => {
                    files_deleted += 1;
                    restored_change_paths.push(prepared.change_file_path.clone());
                    suppression_entries.extend(prepared.suppression_entries());
                }
                Err(e) => failures.push((prepared.change_file_path.clone(), e)),
            }

            on_progress(PreparedDirectoryUndoProgress {
                total_files,
                processed_files: idx + 1,
                restored_files: files_restored,
                deleted_files: files_deleted,
                failed_files: failures.len(),
                current_path: Some(prepared.change_file_path.clone()),
            });
        }

        info!(
            "Execute prepared directory undo for task '{}': restored={}, deleted={}, failures={}",
            plan.task_id,
            files_restored,
            files_deleted,
            failures.len()
        );

        PreparedDirectoryUndoOutcome {
            result: UndoResult {
                files_restored,
                files_deleted,
                failures,
            },
            restored_change_paths,
            suppression_entries,
        }
    }

    /// Undo a single file within a task.
    pub fn undo_file(
        &self,
        db: &crate::db::Database,
        task_id: &str,
        file_path: &Path,
    ) -> RewResult<UndoResult> {
        let changes = db.get_changes_for_task(task_id)?;

        let change = changes
            .iter()
            .find(|c| c.file_path == file_path)
            .ok_or_else(|| {
                RewError::Snapshot(format!(
                    "No change for '{}' in task '{}'",
                    file_path.display(),
                    task_id
                ))
            })?;

        let mut failures = Vec::new();
        let (mut files_restored, mut files_deleted) = (0, 0);

        match self.undo_single_change(db, change) {
            Ok((UndoAction::Restored, suppression_entries)) => {
                files_restored = 1;
                apply_file_index_updates_after_restore(db, &suppression_entries);
            }
            Ok((UndoAction::Deleted, suppression_entries)) => {
                files_deleted = 1;
                apply_file_index_updates_after_restore(db, &suppression_entries);
            }
            Err(e) => failures.push((file_path.to_path_buf(), e)),
        }

        Ok(UndoResult {
            files_restored,
            files_deleted,
            failures,
        })
    }

    /// Undo a single change, returning what action was taken.
    fn undo_single_change(
        &self,
        db: &crate::db::Database,
        change: &crate::types::Change,
    ) -> Result<(UndoAction, Vec<PreparedSuppressionEntry>), String> {
        let prepared = self.prepare_undo_change(db, change);
        let action = self.execute_prepared_undo_change(&prepared)?;
        Ok((action, prepared.suppression_entries()))
    }

    fn prepare_undo_change(
        &self,
        db: &crate::db::Database,
        change: &crate::types::Change,
    ) -> PreparedUndoChange {
        use crate::types::ChangeType;

        let action = match change.change_type {
            ChangeType::Created => PreparedUndoAction::DeleteCurrent {
                target_path: change.file_path.clone(),
            },
            ChangeType::Modified | ChangeType::Deleted => match self.resolve_restore_hash(
                db,
                &change.file_path,
                change.old_hash.as_deref(),
            ) {
                Ok(resolved) => PreparedUndoAction::RestoreObject {
                    restore_hashes: resolved.candidates,
                    restore_target: change.file_path.clone(),
                    delete_after_restore: None,
                    expected_content_hash: resolved.expected_content_hash,
                },
                Err(error) => PreparedUndoAction::Fail { error },
            },
            ChangeType::Renamed => {
                if let Some(ref old_path) = change.old_file_path {
                    match self.resolve_restore_hash(db, old_path, change.old_hash.as_deref()) {
                        Ok(resolved) => PreparedUndoAction::RestoreObject {
                            restore_hashes: resolved.candidates,
                            restore_target: old_path.clone(),
                            delete_after_restore: if change.file_path != *old_path {
                                Some(change.file_path.clone())
                            } else {
                                None
                            },
                            expected_content_hash: resolved.expected_content_hash,
                        },
                        Err(error) => PreparedUndoAction::Fail { error },
                    }
                } else {
                    match self.resolve_restore_hash(
                        db,
                        &change.file_path,
                        change.old_hash.as_deref(),
                    ) {
                        Ok(resolved) => PreparedUndoAction::RestoreObject {
                            restore_hashes: resolved.candidates,
                            restore_target: change.file_path.clone(),
                            delete_after_restore: None,
                            expected_content_hash: resolved.expected_content_hash,
                        },
                        Err(error) => PreparedUndoAction::Fail { error },
                    }
                }
            }
        };

        PreparedUndoChange {
            change_file_path: change.file_path.clone(),
            action,
        }
    }

    fn execute_prepared_undo_change(&self, prepared: &PreparedUndoChange) -> Result<UndoAction, String> {
        match &prepared.action {
            PreparedUndoAction::DeleteCurrent { target_path } => {
                if target_path.exists() {
                    remove_path_recursively(target_path)?;
                    self.cleanup_empty_ancestor_dirs(target_path);
                }
                Ok(UndoAction::Deleted)
            }
            PreparedUndoAction::RestoreObject {
                restore_hashes,
                restore_target,
                delete_after_restore,
                expected_content_hash: _,
            } => {
                let mut last_error = None;
                let mut restored = false;
                for restore_hash in restore_hashes {
                    match self.restore_from_object(restore_hash, restore_target) {
                        Ok(()) => {
                            restored = true;
                            break;
                        }
                        Err(err) => last_error = Some(err),
                    }
                }
                if !restored {
                    return Err(last_error.unwrap_or_else(|| {
                        format!("Failed to restore {}", restore_target.display())
                    }));
                }
                if let Some(delete_path) = delete_after_restore {
                    if delete_path.exists() && delete_path != restore_target {
                        remove_path_recursively(delete_path)?;
                        self.cleanup_empty_ancestor_dirs(delete_path);
                    }
                }
                Ok(UndoAction::Restored)
            }
            PreparedUndoAction::Fail { error } => Err(error.clone()),
        }
    }

    fn resolve_restore_hash(
        &self,
        db: &crate::db::Database,
        restore_path: &Path,
        old_hash: Option<&str>,
    ) -> Result<ResolvedRestoreHashes, String> {
        let mut candidates = Vec::new();
        let mut expected_content_hash = None;
        if let Some(hash) = old_hash {
            candidates.push(hash.to_string());
            if !hash.starts_with("fast-") {
                expected_content_hash = Some(hash.to_string());
            }
        }

        let path_key = restore_path.to_string_lossy().to_string();
        if let Ok(Some(entry)) = db.get_file_index_entry(&path_key) {
            if let Some(content_hash) = entry.content_hash {
                if expected_content_hash.is_none() {
                    expected_content_hash = Some(content_hash.clone());
                }
                if !candidates.iter().any(|hash| hash == &content_hash) {
                    candidates.push(content_hash);
                }
            }
            if !candidates.iter().any(|hash| hash == &entry.fast_hash) {
                candidates.push(entry.fast_hash);
            }
        }

        if !candidates.is_empty() {
            return Ok(ResolvedRestoreHashes {
                candidates,
                expected_content_hash,
            });
        }

        Err(
            "该文件没有可用备份记录（old_hash 为空，file_index 也未命中）。\n\n\
             可能原因：文件在 rew 建立基线前从未被扫描或备份。\n\
             建议：检查废纸篓，或重新扫描目录后再继续使用。"
                .to_string(),
        )
    }

    fn cleanup_empty_ancestor_dirs(&self, path: &Path) {
        cleanup_empty_ancestor_dirs(path, &self.cleanup_boundaries);
    }

    /// Restore a file from the object store.
    fn restore_from_object(
        &self,
        hash: &str,
        target: &Path,
    ) -> Result<(), String> {
        let obj_path = self.object_path(hash);

        if !obj_path.exists() {
            return Err(format!(
                "Object {} not found for {}",
                &hash[..12.min(hash.len())],
                target.display()
            ));
        }

        // Create parent dir if needed
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir {}: {}", parent.display(), e))?;
        }

        // Copy object to target
        std::fs::copy(&obj_path, target)
            .map_err(|e| format!("Failed to restore {}: {}", target.display(), e))?;

        Ok(())
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        if hash.len() < 4 {
            return self.objects_root.join(hash);
        }
        self.objects_root.join(&hash[..2]).join(&hash[2..])
    }
}

impl PreparedUndoChange {
    fn suppression_entries(&self) -> Vec<PreparedSuppressionEntry> {
        match &self.action {
            PreparedUndoAction::DeleteCurrent { target_path } => vec![PreparedSuppressionEntry {
                path: target_path.clone(),
                expected_content_hash: None,
                deleted: true,
            }],
            PreparedUndoAction::RestoreObject {
                restore_target,
                delete_after_restore,
                expected_content_hash,
                ..
            } => {
                let mut entries = vec![PreparedSuppressionEntry {
                    path: restore_target.clone(),
                    expected_content_hash: expected_content_hash.clone(),
                    deleted: false,
                }];
                if let Some(delete_path) = delete_after_restore {
                    entries.push(PreparedSuppressionEntry {
                        path: delete_path.clone(),
                        expected_content_hash: None,
                        deleted: true,
                    });
                }
                entries
            }
            PreparedUndoAction::Fail { .. } => Vec::new(),
        }
    }
}

fn remove_path_recursively(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect {}: {}", path.display(), e))?;

    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
            .map_err(|e| format!("Failed to delete directory {}: {}", path.display(), e))
    } else {
        std::fs::remove_file(path)
            .map_err(|e| format!("Failed to delete {}: {}", path.display(), e))
    }
}

fn cleanup_empty_ancestor_dirs(path: &Path, stop_paths: &[PathBuf]) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if stop_paths.iter().any(|stop| dir == stop) {
            break;
        }

        let Ok(mut entries) = std::fs::read_dir(dir) else {
            break;
        };

        if entries.next().is_some() {
            break;
        }

        if std::fs::remove_dir(dir).is_err() {
            break;
        }

        current = dir.parent();
    }
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

enum UndoAction {
    Restored,
    Deleted,
}

fn apply_file_index_updates_after_restore(
    db: &crate::db::Database,
    entries: &[PreparedSuppressionEntry],
) {
    let seen_at = chrono::Utc::now().to_rfc3339();

    for entry in entries {
        let path_key = entry.path.to_string_lossy().to_string();

        if entry.deleted {
            let restore_hash = db
                .get_file_index_entry(&path_key)
                .ok()
                .flatten()
                .and_then(|existing| existing.content_hash.or(Some(existing.fast_hash)));
            if let Some(hash) = restore_hash.as_deref() {
                let _ = db.mark_file_index_deleted(
                    &path_key,
                    Some(hash),
                    "restore",
                    "restored_delete",
                    &seen_at,
                );
            }
            continue;
        }

        if !entry.path.exists() {
            continue;
        }

        let mtime_secs = std::fs::metadata(&entry.path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        let existing = db.get_file_index_entry(&path_key).ok().flatten();
        let mut content_hash = entry
            .expected_content_hash
            .clone()
            .or_else(|| existing.as_ref().and_then(|row| row.content_hash.clone()));
        let mut fast_hash = existing.as_ref().map(|row| row.fast_hash.clone());

        if content_hash.is_none() {
            content_hash = crate::objects::sha256_file(&entry.path).ok();
        }

        if fast_hash.is_none() {
            fast_hash = content_hash.clone();
        }

        if let Some(fast_hash) = fast_hash {
            let _ = db.upsert_live_file_index_entry(
                &path_key,
                mtime_secs,
                &fast_hash,
                content_hash.as_deref(),
                "restore",
                "restored_live",
                &seen_at,
                None,
            );
        }
    }
}

/// Restore a file from the most recent APFS snapshot.
///
/// NOTE: On macOS, `mount_apfs -s` on the root volume always fails with
/// "Resource busy" because the volume is in use. This L0 fallback is
/// unreliable. The primary restore path is via ObjectStore (shadow + full scan).
///
/// This function is kept as a last-resort attempt but users should not
/// depend on it.
fn restore_from_apfs_snapshot(_file_path: &Path) -> Result<(), String> {
    // Inform the user clearly about why this file can't be restored
    Err(format!(
        "该文件没有备份记录（old_hash 为空）。\n\n\
         可能原因：文件在 rew 启动前从未被修改过，因此没有被备份。\n\
         下次启动 rew 后，后台全量扫描会自动备份所有文件，\n\
         之后删除的文件都可以恢复。\n\n\
         建议：检查废纸篓是否还有该文件。"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::types::SnapshotTrigger;
    use chrono::Utc;
    use tempfile::tempdir;
    use uuid::Uuid;

    /// A test-only snapshot provider that uses an in-memory database
    /// and does not require tmutil.
    struct TestSnapshotProvider {
        db: Database,
    }

    impl TestSnapshotProvider {
        fn new(db: Database) -> Self {
            Self { db }
        }
    }

    impl RestoreSnapshotProvider for TestSnapshotProvider {
        fn get_snapshot(&self, id: &Uuid) -> RewResult<Option<Snapshot>> {
            self.db.get_snapshot(id)
        }

        fn create_safety_snapshot(&self) -> RewResult<Snapshot> {
            let snapshot = Snapshot {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                trigger: SnapshotTrigger::Auto,
                os_snapshot_ref: format!(
                    "com.apple.TimeMachine.safety-{}.local",
                    Utc::now().format("%Y-%m-%d-%H%M%S")
                ),
                files_added: 0,
                files_modified: 0,
                files_deleted: 0,
                pinned: false,
                metadata_json: None,
            };
            self.db.save_snapshot(&snapshot)?;
            Ok(snapshot)
        }

        fn restore_path_from_snapshot(
            &self,
            _snapshot: &Snapshot,
            _source_path: &Path,
            _dest_path: &Path,
        ) -> RewResult<PathBuf> {
            // In tests, we simulate a successful restore without calling tmutil
            Ok(_dest_path.to_path_buf())
        }
    }

    fn setup_test_provider(
        dir: &Path,
    ) -> TestSnapshotProvider {
        let db = Database::open(&dir.join("test.db")).unwrap();
        db.initialize().unwrap();
        TestSnapshotProvider::new(db)
    }

    /// Helper: insert a fake snapshot into the database with a specific timestamp.
    fn insert_test_snapshot(
        provider: &TestSnapshotProvider,
        timestamp: chrono::DateTime<Utc>,
    ) -> Snapshot {
        let snapshot = Snapshot {
            id: Uuid::new_v4(),
            timestamp,
            trigger: SnapshotTrigger::Manual,
            os_snapshot_ref: format!(
                "com.apple.TimeMachine.{}.local",
                timestamp.format("%Y-%m-%d-%H%M%S")
            ),
            files_added: 3,
            files_modified: 2,
            files_deleted: 1,
            pinned: false,
            metadata_json: None,
        };
        provider.db.save_snapshot(&snapshot).unwrap();
        snapshot
    }

    fn insert_snapshot_now(provider: &TestSnapshotProvider) -> Snapshot {
        insert_test_snapshot(provider, Utc::now())
    }

    #[test]
    fn test_dry_run_with_existing_files() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());
        let snapshot = insert_snapshot_now(&provider);

        // Create some files in the work dir to simulate "current state"
        let test_file = dir.path().join("test.txt");
        std::fs::write(&test_file, "current content").unwrap();

        let restore = RestoreEngine::new(provider);
        let job = RestoreJob {
            snapshot_id: snapshot.id,
            target_paths: vec![test_file.clone()],
            dry_run: true,
        };

        let preview = restore.restore(&job).unwrap();
        assert_eq!(preview.snapshot.id, snapshot.id);
        // File existed before snapshot (or at same time) → overwrite
        assert!(
            preview.files_to_overwrite.len() == 1
                || preview.files_to_remove.len() == 1,
            "File should be classified as overwrite or remove, got overwrite={} remove={}",
            preview.files_to_overwrite.len(),
            preview.files_to_remove.len()
        );
        assert!(preview.estimated_size_bytes > 0 || !preview.files_to_remove.is_empty());
    }

    #[test]
    fn test_dry_run_with_missing_files() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());
        let snapshot = insert_snapshot_now(&provider);

        let missing_file = dir.path().join("nonexistent.txt");

        let restore = RestoreEngine::new(provider);
        let job = RestoreJob {
            snapshot_id: snapshot.id,
            target_paths: vec![missing_file.clone()],
            dry_run: true,
        };

        let preview = restore.restore(&job).unwrap();
        assert_eq!(preview.files_to_restore.len(), 1);
        assert_eq!(preview.files_to_restore[0], missing_file);
        assert!(preview.files_to_overwrite.is_empty());
        assert!(preview.files_to_remove.is_empty());
    }

    #[test]
    fn test_dry_run_with_directory() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());
        let snapshot = insert_snapshot_now(&provider);

        // Create a directory with files
        let sub_dir = dir.path().join("subdir");
        std::fs::create_dir(&sub_dir).unwrap();
        std::fs::write(sub_dir.join("a.txt"), "aaa").unwrap();
        std::fs::write(sub_dir.join("b.txt"), "bbb").unwrap();

        let restore = RestoreEngine::new(provider);
        let job = RestoreJob {
            snapshot_id: snapshot.id,
            target_paths: vec![sub_dir],
            dry_run: true,
        };

        let preview = restore.restore(&job).unwrap();
        // All files accounted for in overwrite or remove
        let total = preview.files_to_overwrite.len() + preview.files_to_remove.len();
        assert_eq!(total, 2, "Both files should be classified");
    }

    #[test]
    fn test_dry_run_files_created_after_snapshot_are_marked_for_removal() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());

        // Create snapshot with timestamp in the past
        let past = Utc::now() - chrono::Duration::hours(2);
        let snapshot = insert_test_snapshot(&provider, past);

        // Create a file NOW — it was created after the snapshot
        let new_file = dir.path().join("created_after.txt");
        std::fs::write(&new_file, "I'm new!").unwrap();

        let restore = RestoreEngine::new(provider);
        let job = RestoreJob {
            snapshot_id: snapshot.id,
            target_paths: vec![new_file.clone()],
            dry_run: true,
        };

        let preview = restore.restore(&job).unwrap();
        // The file was created after the snapshot, so it should be in files_to_remove
        assert_eq!(
            preview.files_to_remove.len(),
            1,
            "File created after snapshot should be marked for removal"
        );
        assert_eq!(preview.files_to_remove[0], new_file);
        assert!(
            preview.files_to_overwrite.is_empty(),
            "File created after snapshot should NOT be in overwrite list"
        );
    }

    #[test]
    fn test_dry_run_mixed_files_before_and_after_snapshot() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());

        // Create a file BEFORE the snapshot
        let old_file = dir.path().join("old_file.txt");
        std::fs::write(&old_file, "old content").unwrap();

        // Wait a moment to ensure filesystem timestamp ordering
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Create snapshot with timestamp NOW
        let snapshot = insert_snapshot_now(&provider);

        // Wait a moment, then create a file AFTER the snapshot
        std::thread::sleep(std::time::Duration::from_millis(50));
        let new_file = dir.path().join("new_file.txt");
        std::fs::write(&new_file, "new content").unwrap();

        // Also add a missing file that doesn't exist
        let missing_file = dir.path().join("missing.txt");

        let restore = RestoreEngine::new(provider);
        let job = RestoreJob {
            snapshot_id: snapshot.id,
            target_paths: vec![old_file.clone(), new_file.clone(), missing_file.clone()],
            dry_run: true,
        };

        let preview = restore.restore(&job).unwrap();

        // old_file: created before snapshot → overwrite
        assert!(
            preview.files_to_overwrite.contains(&old_file),
            "old_file should be in overwrite list"
        );
        // new_file: created after snapshot → remove
        assert!(
            preview.files_to_remove.contains(&new_file),
            "new_file should be in remove list"
        );
        // missing_file: doesn't exist → restore
        assert!(
            preview.files_to_restore.contains(&missing_file),
            "missing_file should be in restore list"
        );

        // Total = 3 files accounted for
        let total = preview.files_to_restore.len()
            + preview.files_to_remove.len()
            + preview.files_to_overwrite.len();
        assert_eq!(total, 3, "All 3 files should be classified");
    }

    #[test]
    fn test_restore_nonexistent_snapshot() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());
        let restore = RestoreEngine::new(provider);

        let job = RestoreJob {
            snapshot_id: Uuid::new_v4(),
            target_paths: vec![],
            dry_run: true,
        };

        let result = restore.restore(&job);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"));
    }

    #[test]
    fn test_execute_restore_requires_target_paths() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());
        let snapshot = insert_snapshot_now(&provider);

        let restore = RestoreEngine::new(provider);
        let job = RestoreJob {
            snapshot_id: snapshot.id,
            target_paths: vec![],
            dry_run: false,
        };

        let result = restore.restore(&job);
        // Should fail because tmutil is not available in test env,
        // but would also fail due to safety snapshot creation failure (no tmutil).
        // The important thing is we get a meaningful error.
        assert!(result.is_err());
    }

    #[test]
    fn test_dry_run_empty_preview_on_no_targets() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());
        let snapshot = insert_snapshot_now(&provider);

        let restore = RestoreEngine::new(provider);
        let job = RestoreJob {
            snapshot_id: snapshot.id,
            target_paths: vec![],
            dry_run: true,
        };

        let preview = restore.restore(&job).unwrap();
        assert!(preview.files_to_restore.is_empty());
        assert!(preview.files_to_remove.is_empty());
        assert!(preview.files_to_overwrite.is_empty());
        assert_eq!(preview.estimated_size_bytes, 0);
    }

    #[test]
    fn test_custom_timeout_and_retries() {
        let dir = tempdir().unwrap();
        let provider = setup_test_provider(dir.path());

        let restore = RestoreEngine::new(provider)
            .with_timeout(Duration::from_secs(30))
            .with_max_retries(5);

        assert_eq!(restore.restore_timeout, Duration::from_secs(30));
        assert_eq!(restore.max_retries, 5);
    }

    // ================================================================
    // TaskRestoreEngine tests
    // ================================================================

    use crate::objects::ObjectStore;
    use crate::types::{Change, ChangeType, Task, TaskStatus};

    fn setup_task_restore(
        dir: &std::path::Path,
    ) -> (Database, ObjectStore, TaskRestoreEngine) {
        let db = Database::open(&dir.join("test.db")).unwrap();
        db.initialize().unwrap();

        let objects_dir = dir.join("objects");
        let obj_store = ObjectStore::new(objects_dir.clone()).unwrap();
        let engine = TaskRestoreEngine::new(objects_dir);

        (db, obj_store, engine)
    }

    fn create_test_task(db: &Database, task_id: &str) {
        let task = Task {
            id: task_id.to_string(),
            prompt: Some("test prompt".to_string()),
            tool: Some("test-tool".to_string()),
            started_at: Utc::now(),
            completed_at: None,
            status: TaskStatus::Active,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();
    }

    #[test]
    fn test_undo_created_file() {
        let dir = tempdir().unwrap();
        let (db, _obj, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t001");

        // Simulate: AI created a file
        let created_file = dir.path().join("new_file.rs");
        std::fs::write(&created_file, "fn main() {}").unwrap();

        let change = Change {
            id: None,
            task_id: "t001".to_string(),
            file_path: created_file.clone(),
            change_type: ChangeType::Created,
            old_hash: None,
            new_hash: Some("abc".to_string()),
            diff_text: None,
            lines_added: 1,
            lines_removed: 0,
            attribution: None,
            old_file_path: None,
        };
        db.insert_change(&change).unwrap();

        // Undo → should delete the file
        let result = engine.undo_task(&db, "t001").unwrap();
        assert_eq!(result.files_deleted, 1);
        assert_eq!(result.failures.len(), 0);
        assert!(!created_file.exists());

        // Task should be rolled back
        let task = db.get_task("t001").unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::RolledBack);
    }

    #[test]
    fn test_undo_created_nested_file_cleans_empty_directories() {
        let dir = tempdir().unwrap();
        let (db, _obj, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t001_nested");

        let created_file = dir.path().join("generated/deep/file.rs");
        std::fs::create_dir_all(created_file.parent().unwrap()).unwrap();
        std::fs::write(&created_file, "fn generated() {}").unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "t001_nested".to_string(),
            file_path: created_file.clone(),
            change_type: ChangeType::Created,
            old_hash: None,
            new_hash: Some("nested-hash".to_string()),
            diff_text: None,
            lines_added: 1,
            lines_removed: 0,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();

        let result = engine.undo_task(&db, "t001_nested").unwrap();
        assert_eq!(result.files_deleted, 1);
        assert!(!created_file.exists());
        assert!(!dir.path().join("generated/deep").exists());
        assert!(!dir.path().join("generated").exists());
    }

    #[test]
    fn test_undo_created_nested_file_does_not_delete_cleanup_boundary() {
        let dir = tempdir().unwrap();
        let (db, _obj, _engine) = setup_task_restore(dir.path());
        let engine = TaskRestoreEngine::new(dir.path().join("objects"))
            .with_cleanup_boundaries(vec![dir.path().to_path_buf()]);

        create_test_task(&db, "t001_boundary");

        let created_file = dir.path().join("generated/deep/file.rs");
        std::fs::create_dir_all(created_file.parent().unwrap()).unwrap();
        std::fs::write(&created_file, "fn generated() {}").unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "t001_boundary".to_string(),
            file_path: created_file.clone(),
            change_type: ChangeType::Created,
            old_hash: None,
            new_hash: Some("boundary-hash".to_string()),
            diff_text: None,
            lines_added: 1,
            lines_removed: 0,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();

        let result = engine.undo_task(&db, "t001_boundary").unwrap();
        assert_eq!(result.files_deleted, 1);
        assert!(!created_file.exists());
        assert!(!dir.path().join("generated").exists());
        assert!(dir.path().exists());
    }

    #[test]
    fn test_undo_modified_file() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t002");

        // Create original file and store it in objects
        let target_file = dir.path().join("app.rs");
        std::fs::write(&target_file, "original content").unwrap();
        let old_hash = obj_store.store(&target_file).unwrap();

        // Simulate: AI modified the file
        std::fs::write(&target_file, "AI modified content").unwrap();

        let change = Change {
            id: None,
            task_id: "t002".to_string(),
            file_path: target_file.clone(),
            change_type: ChangeType::Modified,
            old_hash: Some(old_hash),
            new_hash: Some("newhash".to_string()),
            diff_text: None,
            lines_added: 1,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        };
        db.insert_change(&change).unwrap();

        // Undo → should restore original content
        let result = engine.undo_task(&db, "t002").unwrap();
        assert_eq!(result.files_restored, 1);
        assert_eq!(result.failures.len(), 0);
        assert_eq!(
            std::fs::read_to_string(&target_file).unwrap(),
            "original content"
        );
    }

    #[test]
    fn test_undo_deleted_file() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t003");

        // Create original file and store it
        let target_file = dir.path().join("important.txt");
        std::fs::write(&target_file, "important data").unwrap();
        let old_hash = obj_store.store(&target_file).unwrap();

        // Simulate: AI deleted the file
        std::fs::remove_file(&target_file).unwrap();

        let change = Change {
            id: None,
            task_id: "t003".to_string(),
            file_path: target_file.clone(),
            change_type: ChangeType::Deleted,
            old_hash: Some(old_hash),
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 5,
            attribution: None,
            old_file_path: None,
        };
        db.insert_change(&change).unwrap();

        // Undo → should restore the file
        let result = engine.undo_task(&db, "t003").unwrap();
        assert_eq!(result.files_restored, 1);
        assert!(target_file.exists());
        assert_eq!(
            std::fs::read_to_string(&target_file).unwrap(),
            "important data"
        );
    }

    #[test]
    fn test_restore_from_tombstoned_file_index_when_old_hash_missing() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t003_tombstone");

        let target_file = dir.path().join("deleted_via_index.txt");
        std::fs::write(&target_file, "recover me").unwrap();
        let stored_hash = obj_store.store(&target_file).unwrap();
        std::fs::remove_file(&target_file).unwrap();

        db.mark_file_index_deleted(
            &target_file.to_string_lossy(),
            Some(&stored_hash),
            "test",
            "deleted",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "t003_tombstone".to_string(),
            file_path: target_file.clone(),
            change_type: ChangeType::Deleted,
            old_hash: None,
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();

        let result = engine.undo_task(&db, "t003_tombstone").unwrap();
        assert_eq!(result.files_restored, 1);
        assert!(target_file.exists());
        assert_eq!(std::fs::read_to_string(&target_file).unwrap(), "recover me");
    }

    #[test]
    fn test_undo_deleted_file_should_rehydrate_file_index_live_state() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t003_file_index_live");

        let target_file = dir.path().join("aaa/README.md");
        std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
        std::fs::write(&target_file, "restore me").unwrap();
        let old_hash = obj_store.store(&target_file).unwrap();

        db.upsert_live_file_index_entry(
            &target_file.to_string_lossy(),
            1,
            &old_hash,
            Some(&old_hash),
            "test",
            "scan_seen",
            &Utc::now().to_rfc3339(),
            Some(1),
        )
        .unwrap();

        std::fs::remove_file(&target_file).unwrap();
        db.mark_file_index_deleted(
            &target_file.to_string_lossy(),
            Some(&old_hash),
            "test",
            "deleted",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "t003_file_index_live".to_string(),
            file_path: target_file.clone(),
            change_type: ChangeType::Deleted,
            old_hash: Some(old_hash.clone()),
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        })
        .unwrap();

        let result = engine.undo_task(&db, "t003_file_index_live").unwrap();
        assert_eq!(result.files_restored, 1);
        assert!(target_file.exists(), "file should be restored on disk");

        // Regression guard: restore must also flip file_index back to live state.
        let entry = db
            .get_file_index_entry(&target_file.to_string_lossy())
            .unwrap()
            .expect("file_index row should still exist");
        assert!(
            entry.exists_now,
            "file_index should be marked live after restore (exists_now=1)"
        );
    }

    #[test]
    fn test_undo_mixed_changes() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t004");

        // File 1: created by AI (will be deleted on undo)
        let f1 = dir.path().join("new.rs");
        std::fs::write(&f1, "new file").unwrap();

        // File 2: modified by AI (will be restored on undo)
        let f2 = dir.path().join("existing.rs");
        std::fs::write(&f2, "original").unwrap();
        let f2_hash = obj_store.store(&f2).unwrap();
        std::fs::write(&f2, "modified by AI").unwrap();

        // File 3: deleted by AI (will be restored on undo)
        let f3 = dir.path().join("deleted.rs");
        std::fs::write(&f3, "will be deleted").unwrap();
        let f3_hash = obj_store.store(&f3).unwrap();
        std::fs::remove_file(&f3).unwrap();

        db.insert_change(&Change {
            id: None, task_id: "t004".to_string(),
            file_path: f1.clone(), change_type: ChangeType::Created,
            old_hash: None, new_hash: Some("x".into()),
            diff_text: None, lines_added: 1, lines_removed: 0,
            attribution: None,
            old_file_path: None,
        }).unwrap();
        db.insert_change(&Change {
            id: None, task_id: "t004".to_string(),
            file_path: f2.clone(), change_type: ChangeType::Modified,
            old_hash: Some(f2_hash), new_hash: Some("y".into()),
            diff_text: None, lines_added: 1, lines_removed: 1,
            attribution: None,
            old_file_path: None,
        }).unwrap();
        db.insert_change(&Change {
            id: None, task_id: "t004".to_string(),
            file_path: f3.clone(), change_type: ChangeType::Deleted,
            old_hash: Some(f3_hash), new_hash: None,
            diff_text: None, lines_added: 0, lines_removed: 3,
            attribution: None,
            old_file_path: None,
        }).unwrap();

        let result = engine.undo_task(&db, "t004").unwrap();
        assert_eq!(result.files_deleted, 1);  // f1 deleted
        assert_eq!(result.files_restored, 2);  // f2 + f3 restored
        assert_eq!(result.failures.len(), 0);

        assert!(!f1.exists());
        assert_eq!(std::fs::read_to_string(&f2).unwrap(), "original");
        assert_eq!(std::fs::read_to_string(&f3).unwrap(), "will be deleted");
    }

    #[test]
    fn test_undo_single_file() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t005");

        let f1 = dir.path().join("a.rs");
        let f2 = dir.path().join("b.rs");
        std::fs::write(&f1, "orig a").unwrap();
        let f1_hash = obj_store.store(&f1).unwrap();
        std::fs::write(&f1, "modified a").unwrap();
        std::fs::write(&f2, "orig b").unwrap();
        let f2_hash = obj_store.store(&f2).unwrap();
        std::fs::write(&f2, "modified b").unwrap();

        db.insert_change(&Change {
            id: None, task_id: "t005".to_string(),
            file_path: f1.clone(), change_type: ChangeType::Modified,
            old_hash: Some(f1_hash), new_hash: Some("x".into()),
            diff_text: None, lines_added: 1, lines_removed: 1,
            attribution: None,
            old_file_path: None,
        }).unwrap();
        db.insert_change(&Change {
            id: None, task_id: "t005".to_string(),
            file_path: f2.clone(), change_type: ChangeType::Modified,
            old_hash: Some(f2_hash), new_hash: Some("y".into()),
            diff_text: None, lines_added: 1, lines_removed: 1,
            attribution: None,
            old_file_path: None,
        }).unwrap();

        // Only undo f1
        let result = engine.undo_file(&db, "t005", &f1).unwrap();
        assert_eq!(result.files_restored, 1);

        assert_eq!(std::fs::read_to_string(&f1).unwrap(), "orig a");
        assert_eq!(std::fs::read_to_string(&f2).unwrap(), "modified b"); // unchanged
    }

    #[test]
    fn test_undo_directory_only_restores_scoped_changes() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t005_dir");

        let dir_file = dir.path().join("go/pkg/mod/example.txt");
        std::fs::create_dir_all(dir_file.parent().unwrap()).unwrap();
        std::fs::write(&dir_file, "dir original").unwrap();
        let dir_hash = obj_store.store(&dir_file).unwrap();
        std::fs::remove_file(&dir_file).unwrap();

        let other_file = dir.path().join("notes/todo.txt");
        std::fs::create_dir_all(other_file.parent().unwrap()).unwrap();
        std::fs::write(&other_file, "other original").unwrap();
        let other_hash = obj_store.store(&other_file).unwrap();
        std::fs::write(&other_file, "other modified").unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "t005_dir".to_string(),
            file_path: dir_file.clone(),
            change_type: ChangeType::Deleted,
            old_hash: Some(dir_hash),
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        }).unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "t005_dir".to_string(),
            file_path: other_file.clone(),
            change_type: ChangeType::Modified,
            old_hash: Some(other_hash),
            new_hash: Some("other-new".into()),
            diff_text: None,
            lines_added: 1,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        }).unwrap();

        let result = engine
            .undo_directory(&db, "t005_dir", &dir.path().join("go"))
            .unwrap();

        assert_eq!(result.files_restored, 1);
        assert_eq!(result.files_deleted, 0);
        assert!(dir_file.exists());
        assert_eq!(std::fs::read_to_string(&dir_file).unwrap(), "dir original");
        assert_eq!(std::fs::read_to_string(&other_file).unwrap(), "other modified");

        let task = db.get_task("t005_dir").unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::PartialRolledBack);
    }

    #[test]
    fn test_undo_directory_falls_back_to_fast_hash_object_when_old_hash_object_missing() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t005_dir_fast");

        let dir_file = dir.path().join("go/pkg/mod/example.txt");
        std::fs::create_dir_all(dir_file.parent().unwrap()).unwrap();
        std::fs::write(&dir_file, "dir original from fast backup").unwrap();

        let fast_hash = obj_store.store_fast(&dir_file).unwrap();
        let content_hash = crate::objects::sha256_file(&dir_file).unwrap();

        db.upsert_live_file_index_entry(
            &dir_file.to_string_lossy(),
            1,
            &fast_hash,
            Some(&content_hash),
            "test",
            "scan_seen",
            &Utc::now().to_rfc3339(),
            Some(1),
        ).unwrap();

        std::fs::remove_file(&dir_file).unwrap();
        db.mark_file_index_deleted(
            &dir_file.to_string_lossy(),
            Some(&content_hash),
            "test",
            "deleted",
            &Utc::now().to_rfc3339(),
        ).unwrap();

        db.insert_change(&Change {
            id: None,
            task_id: "t005_dir_fast".to_string(),
            file_path: dir_file.clone(),
            change_type: ChangeType::Deleted,
            old_hash: Some(content_hash),
            new_hash: None,
            diff_text: None,
            lines_added: 0,
            lines_removed: 1,
            attribution: None,
            old_file_path: None,
        }).unwrap();

        let result = engine
            .undo_directory(&db, "t005_dir_fast", &dir.path().join("go"))
            .unwrap();

        assert_eq!(result.files_restored, 1);
        assert_eq!(result.failures.len(), 0);
        assert!(dir_file.exists());
        assert_eq!(
            std::fs::read_to_string(&dir_file).unwrap(),
            "dir original from fast backup"
        );
    }

    #[test]
    fn test_execute_prepared_directory_undo_restores_deleted_directory_and_emits_suppressions() {
        let dir = tempdir().unwrap();
        let (db, obj_store, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t005_dir_full");

        let file_a = dir.path().join("go/pkg/a.txt");
        let file_b = dir.path().join("go/pkg/sub/b.txt");
        std::fs::create_dir_all(file_a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(file_b.parent().unwrap()).unwrap();
        std::fs::write(&file_a, "alpha original").unwrap();
        std::fs::write(&file_b, "beta original").unwrap();
        let hash_a = obj_store.store(&file_a).unwrap();
        let hash_b = obj_store.store(&file_b).unwrap();

        std::fs::remove_file(&file_a).unwrap();
        std::fs::remove_file(&file_b).unwrap();
        let _ = std::fs::remove_dir_all(dir.path().join("go"));

        for (path, hash) in [(&file_a, &hash_a), (&file_b, &hash_b)] {
            db.insert_change(&Change {
                id: None,
                task_id: "t005_dir_full".to_string(),
                file_path: path.clone(),
                change_type: ChangeType::Deleted,
                old_hash: Some(hash.clone()),
                new_hash: None,
                diff_text: None,
                lines_added: 0,
                lines_removed: 1,
                attribution: None,
                old_file_path: None,
            })
            .unwrap();
        }

        let plan = engine
            .prepare_directory_undo(&db, "t005_dir_full", &dir.path().join("go"))
            .unwrap();
        let outcome = engine.execute_prepared_directory_undo(&plan);

        assert_eq!(outcome.result.files_restored, 2);
        assert_eq!(outcome.result.files_deleted, 0);
        assert_eq!(outcome.result.failures.len(), 0);
        assert_eq!(outcome.restored_change_paths.len(), 2);
        assert_eq!(outcome.suppression_entries.len(), 2);
        assert!(file_a.exists());
        assert!(file_b.exists());
        assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "alpha original");
        assert_eq!(std::fs::read_to_string(&file_b).unwrap(), "beta original");
        assert!(outcome
            .suppression_entries
            .iter()
            .all(|entry| !entry.deleted && entry.expected_content_hash.is_some()));
    }

    #[test]
    fn test_preview_undo() {
        let dir = tempdir().unwrap();
        let (db, _obj, engine) = setup_task_restore(dir.path());

        create_test_task(&db, "t006");

        db.insert_change(&Change {
            id: None, task_id: "t006".to_string(),
            file_path: PathBuf::from("/tmp/created.rs"), change_type: ChangeType::Created,
            old_hash: None, new_hash: Some("x".into()),
            diff_text: None, lines_added: 10, lines_removed: 0,
            attribution: None,
            old_file_path: None,
        }).unwrap();
        db.insert_change(&Change {
            id: None, task_id: "t006".to_string(),
            file_path: PathBuf::from("/tmp/modified.rs"), change_type: ChangeType::Modified,
            old_hash: Some("oldhash".into()), new_hash: Some("y".into()),
            diff_text: None, lines_added: 5, lines_removed: 3,
            attribution: None,
            old_file_path: None,
        }).unwrap();

        let preview = engine.preview_undo(&db, "t006").unwrap();
        assert_eq!(preview.total_changes, 2);
        assert_eq!(preview.files_to_delete.len(), 1);
        assert_eq!(preview.files_to_restore.len(), 1);
    }

    #[test]
    fn test_undo_already_rolled_back_is_allowed() {
        let dir = tempdir().unwrap();
        let (db, _obj, engine) = setup_task_restore(dir.path());

        let task = Task {
            id: "t007".to_string(),
            prompt: None,
            tool: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TaskStatus::RolledBack,
            risk_level: None,
            summary: None,
            cwd: None,
        };
        db.create_task(&task).unwrap();

        // Should succeed (no changes to restore, but no error either)
        let result = engine.undo_task(&db, "t007");
        assert!(result.is_ok());
    }

    #[test]
    fn test_undo_nonexistent_task() {
        let dir = tempdir().unwrap();
        let (db, _obj, engine) = setup_task_restore(dir.path());

        let result = engine.undo_task(&db, "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}

// ================================================================
// Temp-file helpers
// ================================================================

/// Returns true if this path represents a temporary/system file that should
/// never be recorded or restored (e.g. macOS safe-save intermediates).
fn is_temp_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // macOS safe-save temp files: "original.sb-XXXXXXXX-YYYYYY"
    if let Some(idx) = name.find(".sb-") {
        // ".sb-" must not be the very start (would be a dotfile with that name)
        if idx > 0 {
            return true;
        }
    }

    // Common editor/OS temp patterns
    if name.ends_with(".tmp") || name.ends_with(".temp") {
        return true;
    }
    // Emacs lock files
    if name.starts_with(".#") {
        return true;
    }

    false
}
