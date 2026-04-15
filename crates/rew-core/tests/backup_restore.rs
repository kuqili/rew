//! Integration tests for backup and restore functionality.

use chrono::Utc;
use rew_core::backup::{BackupEngine, BackupJob};
use rew_core::config::RewConfig;
use rew_core::types::{FileEvent, FileEventKind};
use std::fs;
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn test_backup_and_restore_workflow() {
    let temp_dir = tempdir().unwrap();
    let source_dir = temp_dir.path().join("source");
    let backup_dir = temp_dir.path().join("backups");
    fs::create_dir_all(&source_dir).unwrap();

    let file1 = source_dir.join("document.txt");
    fs::write(&file1, "Important document").unwrap();

    let subdir = source_dir.join("subfolder");
    fs::create_dir(&subdir).unwrap();
    let file2 = subdir.join("data.json");
    fs::write(&file2, r#"{"key": "value"}"#).unwrap();

    let config = RewConfig::default();
    let engine = BackupEngine::new(&config).unwrap();

    let snapshot_id = Uuid::new_v4();
    let events = vec![
        FileEvent {
            path: file1.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(18),
        },
        FileEvent {
            path: file2.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(16),
        },
    ];

    let job = BackupJob {
        snapshot_id,
        events,
        backup_root: backup_dir.clone(),
    };

    let backup_result = engine.backup_batch(&job).unwrap();
    assert_eq!(backup_result.files_backed_up, 2);
    assert!(backup_result.total_size_bytes > 0);

    let backed_up_file1 = backup_dir
        .join(snapshot_id.to_string())
        .join(file1.strip_prefix("/").unwrap_or(&file1));
    assert!(backed_up_file1.exists());

    let backed_up_file2 = backup_dir
        .join(snapshot_id.to_string())
        .join(file2.strip_prefix("/").unwrap_or(&file2));
    assert!(backed_up_file2.exists());

    let original_content1 = fs::read_to_string(&file1).unwrap();
    let backed_up_content1 = fs::read_to_string(&backed_up_file1).unwrap();
    assert_eq!(original_content1, backed_up_content1);

    let original_content2 = fs::read_to_string(&file2).unwrap();
    let backed_up_content2 = fs::read_to_string(&backed_up_file2).unwrap();
    assert_eq!(original_content2, backed_up_content2);
}

#[test]
fn test_backup_ignores_patterns() {
    let temp_dir = tempdir().unwrap();
    let source_dir = temp_dir.path().join("source");
    let backup_dir = temp_dir.path().join("backups");
    fs::create_dir_all(&source_dir).unwrap();

    let normal_file = source_dir.join("main.rs");
    fs::write(&normal_file, "fn main() {}").unwrap();

    let node_modules = source_dir.join("node_modules");
    fs::create_dir(&node_modules).unwrap();
    let dep_file = node_modules.join("package.js");
    fs::write(&dep_file, "exports = {}").unwrap();

    let mut config = RewConfig::default();
    config.ignore_patterns = vec!["**/node_modules/**".to_string()];
    let engine = BackupEngine::new(&config).unwrap();

    let snapshot_id = Uuid::new_v4();
    let events = vec![
        FileEvent {
            path: normal_file.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(12),
        },
        FileEvent {
            path: dep_file.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(10),
        },
    ];

    let job = BackupJob {
        snapshot_id,
        events,
        backup_root: backup_dir.clone(),
    };

    let backup_result = engine.backup_batch(&job).unwrap();
    assert_eq!(backup_result.files_backed_up, 1);

    let backed_up_normal = backup_dir
        .join(snapshot_id.to_string())
        .join(normal_file.strip_prefix("/").unwrap_or(&normal_file));
    assert!(backed_up_normal.exists());
}

#[test]
fn test_backup_handles_deleted_files_gracefully() {
    let temp_dir = tempdir().unwrap();
    let backup_dir = temp_dir.path().join("backups");

    let config = RewConfig::default();
    let engine = BackupEngine::new(&config).unwrap();

    let deleted_file = temp_dir.path().join("already_deleted.txt");

    let snapshot_id = Uuid::new_v4();
    let events = vec![FileEvent {
        path: deleted_file,
        kind: FileEventKind::Deleted,
        timestamp: Utc::now(),
        size_bytes: None,
    }];

    let job = BackupJob {
        snapshot_id,
        events,
        backup_root: backup_dir,
    };

    let backup_result = engine.backup_batch(&job).unwrap();
    assert_eq!(backup_result.files_backed_up, 0);
    assert_eq!(backup_result.failed_files.len(), 1);
}

#[test]
fn test_backup_size_calculation() {
    let temp_dir = tempdir().unwrap();
    let source_dir = temp_dir.path().join("source");
    let backup_dir = temp_dir.path().join("backups");
    fs::create_dir_all(&source_dir).unwrap();

    let file1 = source_dir.join("file1.txt");
    fs::write(&file1, "a".repeat(1000)).unwrap();

    let file2 = source_dir.join("file2.txt");
    fs::write(&file2, "b".repeat(2000)).unwrap();

    let config = RewConfig::default();
    let engine = BackupEngine::new(&config).unwrap();

    let snapshot_id = Uuid::new_v4();
    let events = vec![
        FileEvent {
            path: file1.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(1000),
        },
        FileEvent {
            path: file2.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(2000),
        },
    ];

    let job = BackupJob {
        snapshot_id,
        events,
        backup_root: backup_dir.clone(),
    };

    let backup_result = engine.backup_batch(&job).unwrap();
    assert_eq!(backup_result.files_backed_up, 2);
    assert!(backup_result.total_size_bytes >= 3000);
    assert!(backup_result.total_size_bytes < 4000);
}
