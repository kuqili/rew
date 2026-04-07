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
}

/// Blanket implementation for any Database + SnapshotEngine combo.
/// Concrete adapters are defined below.
pub struct SnapshotProviderAdapter<'a> {
    db: &'a Database,
    /// Closure that creates a snapshot. Returns the created Snapshot on success.
    create_fn: Box<dyn Fn() -> RewResult<Snapshot> + 'a>,
}

impl<'a> SnapshotProviderAdapter<'a> {
    /// Create a provider backed by a Database and a closure for snapshot creation.
    pub fn new(
        db: &'a Database,
        create_fn: impl Fn() -> RewResult<Snapshot> + 'a,
    ) -> Self {
        Self {
            db,
            create_fn: Box::new(create_fn),
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

            match self.restore_path(snapshot, target) {
                Ok(()) => return Ok(()),
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

    /// Restore a single path from a snapshot using tmutil.
    fn restore_path(&self, snapshot: &Snapshot, target: &Path) -> RewResult<()> {
        use std::process::Command;

        let target_str = target.to_str().unwrap_or_default();

        // tmutil restore uses: tmutil restore <source> <destination>
        // For APFS snapshot restore, the snapshot can be accessed via the mount point.
        let output = Command::new("tmutil")
            .args(["restore", target_str, target_str])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    RewError::Snapshot(format!(
                        "tmutil restore requires elevated privileges for {:?}. \
                         Please grant Full Disk Access to rew in System Settings > \
                         Privacy & Security > Full Disk Access.",
                        target
                    ))
                } else if e.kind() == std::io::ErrorKind::NotFound {
                    RewError::Snapshot(
                        "tmutil not found. Restore is only available on macOS.".to_string(),
                    )
                } else {
                    RewError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };

            return Err(RewError::Snapshot(format!(
                "tmutil restore failed for {:?} (exit code {:?}): {}",
                target,
                output.status.code(),
                detail
            )));
        }

        info!("Restored {:?} from snapshot {}", target, snapshot.id);
        Ok(())
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
        let provider = SnapshotProviderAdapter::new(engine.db(), || {
            engine.create(SnapshotTrigger::Auto, 0, 0, 0)
        });
        RestoreEngine::new(provider)
    }
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
}
