//! Core backup engine for file backup operations.

use crate::config::RewConfig;
use crate::error::RewResult;
use crate::types::{FileEvent, FileEventKind};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use super::copy_strategy::FileCopyStrategy;

/// A single backup job specification.
#[derive(Debug, Clone)]
pub struct BackupJob {
    /// Snapshot ID that owns these backups
    pub snapshot_id: Uuid,
    /// Events to backup
    pub events: Vec<FileEvent>,
    /// Root directory for backups (typically ~/.rew/backups)
    pub backup_root: PathBuf,
}

/// Result of a backup operation.
#[derive(Debug, Clone)]
pub struct BackupResult {
    /// Snapshot ID that was backed up
    pub snapshot_id: Uuid,
    /// Number of files successfully backed up
    pub files_backed_up: usize,
    /// Total size of backed up files in bytes
    pub total_size_bytes: u64,
    /// Failed files: (path, error reason)
    pub failed_files: Vec<(PathBuf, String)>,
}

/// Main backup engine that orchestrates file backups.
pub struct BackupEngine {
    copy_strategy: FileCopyStrategy,
}

impl BackupEngine {
    /// Create a new BackupEngine with configuration.
    pub fn new(config: &RewConfig) -> RewResult<Self> {
        let copy_strategy = FileCopyStrategy::new(config.ignore_patterns.clone())?;
        Ok(BackupEngine { copy_strategy })
    }

    /// Execute a backup job, copying relevant files to the backup directory.
    pub fn backup_batch(&self, job: &BackupJob) -> RewResult<BackupResult> {
        // Create the snapshot-specific backup directory
        let snapshot_backup_dir = job.backup_root.join(job.snapshot_id.to_string());
        std::fs::create_dir_all(&snapshot_backup_dir)?;

        let mut files_backed_up = 0;
        let mut total_size = 0u64;
        let mut failed_files = Vec::new();

        // Backup only modified and deleted files (not created ones by default to save space)
        // NOTE: Decisions on whether to backup "Created" can be made at config time
        for event in &job.events {
            match event.kind {
                FileEventKind::Modified | FileEventKind::Deleted => {
                    // For deleted files, we can't read them now, so skip
                    // For modified, we can still read them
                    if event.kind == FileEventKind::Modified && event.path.exists() {
                        match self.copy_strategy.copy_file_with_retry(&event.path, &snapshot_backup_dir, 3) {
                            Ok(size) if size > 0 => {
                                files_backed_up += 1;
                                total_size += size;
                            }
                            Ok(_) => {
                                // size == 0 means file was skipped by ignore patterns
                            }
                            Err(e) => {
                                failed_files.push((event.path.clone(), e));
                            }
                        }
                    } else if event.kind == FileEventKind::Deleted {
                        // Log: file already deleted, can't backup
                        failed_files.push((
                            event.path.clone(),
                            "File already deleted when backup was attempted".to_string(),
                        ));
                    }
                }
                FileEventKind::Created => {
                    // Optionally backup created files (disabled by default to save space)
                    // Uncomment below if you want to backup newly created files:
                    // if event.path.exists() {
                    //     match self.copy_strategy.copy_file_with_retry(&event.path, &snapshot_backup_dir, 3) {
                    //         Ok(size) => {
                    //             files_backed_up += 1;
                    //             total_size += size;
                    //         }
                    //         Err(e) => {
                    //             failed_files.push((event.path.clone(), e));
                    //         }
                    //     }
                    // }
                }
                FileEventKind::Renamed => {
                    // For renamed files, backup both the new name if it exists
                    if event.path.exists() {
                        match self.copy_strategy.copy_file_with_retry(&event.path, &snapshot_backup_dir, 3) {
                            Ok(size) if size > 0 => {
                                files_backed_up += 1;
                                total_size += size;
                            }
                            Ok(_) => {
                                // size == 0 means file was skipped by ignore patterns
                            }
                            Err(e) => {
                                failed_files.push((event.path.clone(), e));
                            }
                        }
                    }
                }
            }
        }

        Ok(BackupResult {
            snapshot_id: job.snapshot_id,
            files_backed_up,
            total_size_bytes: total_size,
            failed_files,
        })
    }

    /// Get the size of a backup directory.
    pub fn get_backup_size(&self, snapshot_id: &Uuid, backup_root: &PathBuf) -> RewResult<u64> {
        let snapshot_dir = backup_root.join(snapshot_id.to_string());
        let mut total = 0u64;

        if snapshot_dir.exists() {
            for entry in walkdir::WalkDir::new(&snapshot_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
            {
                if let Ok(metadata) = entry.metadata() {
                    total += metadata.len();
                }
            }
        }

        Ok(total)
    }

    /// Delete a backup directory for a snapshot.
    pub fn delete_backup(&self, snapshot_id: &Uuid, backup_root: &PathBuf) -> RewResult<()> {
        delete_backup_dir(snapshot_id, backup_root)
    }
}

/// Return the backup directory path for a snapshot.
pub fn snapshot_backup_dir(snapshot_id: &Uuid, backup_root: &Path) -> PathBuf {
    backup_root.join(snapshot_id.to_string())
}

/// Delete a backup directory for a snapshot.
pub fn delete_backup_dir(snapshot_id: &Uuid, backup_root: &Path) -> RewResult<()> {
    let snapshot_dir = snapshot_backup_dir(snapshot_id, backup_root);
    if snapshot_dir.exists() {
        std::fs::remove_dir_all(snapshot_dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RewConfig;
    use tempfile::tempdir;

    #[test]
    fn test_backup_engine_creation() {
        let config = RewConfig::default();
        let engine = BackupEngine::new(&config).unwrap();
        // If we got here, engine was created successfully
        assert!(true);
    }

    #[test]
    fn test_backup_modified_files() {
        let config = RewConfig::default();
        let engine = BackupEngine::new(&config).unwrap();

        let temp_dir = tempdir().unwrap();
        let backup_root = temp_dir.path().join("backups");
        let snapshot_id = Uuid::new_v4();

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "original content").unwrap();

        // Create an event for modification
        let event = FileEvent {
            path: test_file.clone(),
            kind: FileEventKind::Modified,
            timestamp: chrono::Utc::now(),
            size_bytes: Some(16),
        };

        let job = BackupJob {
            snapshot_id,
            events: vec![event],
            backup_root: backup_root.clone(),
        };

        let result = engine.backup_batch(&job).unwrap();
        assert_eq!(result.files_backed_up, 1);
        assert!(result.total_size_bytes > 0);
    }

    #[test]
    fn test_backup_ignores_patterns() {
        // Create a custom config that only ignores .swp files
        let mut config = RewConfig::default();
        config.ignore_patterns = vec!["**/*.swp".to_string()];
        let engine = BackupEngine::new(&config).unwrap();

        let temp_dir = tempdir().unwrap();
        let backup_root = temp_dir.path().join("backups");
        let snapshot_id = Uuid::new_v4();

        // Create a regular .txt file - should be backed up
        let txt_file = temp_dir.path().join("test.txt");
        std::fs::write(&txt_file, "content").unwrap();

        let txt_event = FileEvent {
            path: txt_file,
            kind: FileEventKind::Modified,
            timestamp: chrono::Utc::now(),
            size_bytes: Some(7),
        };

        let job = BackupJob {
            snapshot_id,
            events: vec![txt_event],
            backup_root,
        };

        let result = engine.backup_batch(&job).unwrap();
        // .txt file should be backed up since only .swp is ignored
        assert_eq!(result.files_backed_up, 1);
    }

    #[test]
    fn test_backup_deleted_files_recorded_as_failed() {
        let config = RewConfig::default();
        let engine = BackupEngine::new(&config).unwrap();

        let temp_dir = tempdir().unwrap();
        let backup_root = temp_dir.path().join("backups");
        let snapshot_id = Uuid::new_v4();

        // File that doesn't exist (simulating already-deleted)
        let deleted_file = temp_dir.path().join("deleted.txt");

        let event = FileEvent {
            path: deleted_file,
            kind: FileEventKind::Deleted,
            timestamp: chrono::Utc::now(),
            size_bytes: None,
        };

        let job = BackupJob {
            snapshot_id,
            events: vec![event],
            backup_root,
        };

        let result = engine.backup_batch(&job).unwrap();
        // Should be marked as failed (can't backup deleted file)
        assert_eq!(result.files_backed_up, 0);
        assert_eq!(result.failed_files.len(), 1);
    }
}
