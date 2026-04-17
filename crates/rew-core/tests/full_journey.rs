//! Integration tests: Full user journey simulation.
//!
//! Simulates: create directory → write files → detect events → trigger alerts →
//! verify snapshot creation → delete files → verify anomaly → restore → verify files.
//!
//! Note: These tests exercise the rew-core pipeline and detection logic.
//! Snapshot creation/restore requires APFS (macOS) and root-level tmutil access,
//! so those parts are tested in mock/simulated mode.

use rew_core::config::RewConfig;
use rew_core::db::Database;
use rew_core::detector::RuleEngine;
use rew_core::lifecycle;
use rew_core::processor::BatchStats;
use rew_core::traits::AnomalyDetector;
use rew_core::types::*;
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use rew_core::backup::{BackupEngine, BackupJob};
use tempfile::tempdir;
use uuid::Uuid;

/// Create a test config with a temp dir as watch directory.
fn test_config(watch_dir: &Path) -> RewConfig {
    let mut config = RewConfig::default();
    config.watch_dirs = vec![watch_dir.to_path_buf()];
    config
}

/// Create a test database in a temp directory.
fn test_db(dir: &Path) -> Database {
    let db_path = dir.join("test_snapshots.db");
    let db = Database::open(&db_path).unwrap();
    db.initialize().unwrap();
    db
}

/// Create N test files with content in a directory.
fn create_test_files(dir: &Path, count: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for i in 0..count {
        let file_path = dir.join(format!("test_file_{:03}.txt", i));
        let content = format!(
            "Test file {} content. Line 1.\nLine 2 with data: {}\nLine 3 final.\n",
            i,
            i * 42
        );
        fs::write(&file_path, &content).unwrap();
        files.push(file_path);
    }
    files
}

/// Simulate file events for created files.
fn simulate_file_events(files: &[PathBuf], kind: FileEventKind) -> Vec<FileEvent> {
    files
        .iter()
        .map(|path| {
            let size = fs::metadata(path).ok().map(|m| m.len());
            FileEvent {
                path: path.clone(),
                kind: kind.clone(),
                timestamp: Utc::now(),
                size_bytes: size,
            }
        })
        .collect()
}

/// Create an EventBatch from events.
fn make_batch(events: Vec<FileEvent>) -> EventBatch {
    let now = Utc::now();
    EventBatch {
        events,
        window_start: now - chrono::Duration::seconds(30),
        window_end: now,
    }
}

// ========================================================================
// Integration Test: Full Journey
// ========================================================================

#[test]
fn test_full_journey_create_detect_restore() {
    let tmp = TempDir::new().unwrap();
    let watch_dir = tmp.path().join("project");
    let db_dir = tmp.path().join("db");
    fs::create_dir_all(&watch_dir).unwrap();
    fs::create_dir_all(&db_dir).unwrap();

    let config = test_config(&watch_dir);
    let db = test_db(&db_dir);

    // Step 1: Create 30 test files
    let files = create_test_files(&watch_dir, 30);
    assert_eq!(files.len(), 30);

    // Verify all files exist and have content
    for file in &files {
        assert!(file.exists(), "File should exist: {}", file.display());
        let content = fs::read_to_string(file).unwrap();
        assert!(!content.is_empty());
    }

    // Step 2: Simulate file creation events → should NOT trigger anomaly
    let create_events = simulate_file_events(&files, FileEventKind::Created);
    let create_batch = make_batch(create_events);

    let rule_engine = RuleEngine::new(config.anomaly_rules.clone(), config.watch_dirs.clone());
    let create_stats = BatchStats::from_batch(&create_batch);
    let alerts = rule_engine.analyze(&create_batch, &create_stats);
    assert!(
        alerts.is_empty(),
        "File creation should not trigger anomaly, got {} alerts",
        alerts.len()
    );

    // Step 3: Simulate saving a snapshot (mock — real snapshot requires tmutil root access)
    let snapshot = Snapshot {
        id: uuid::Uuid::new_v4(),
        timestamp: Utc::now(),
        trigger: SnapshotTrigger::Auto,
        os_snapshot_ref: "mock-snapshot-001".to_string(),
        files_added: 30,
        files_modified: 0,
        files_deleted: 0,
        pinned: false,
        metadata_json: None,
    };
    db.save_snapshot(&snapshot).unwrap();

    // Verify snapshot was saved
    let snapshots = db.list_snapshots().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].files_added, 30);

    // Step 4: Delete ALL files → should trigger anomaly
    for file in &files {
        fs::remove_file(file).unwrap();
    }

    // Verify all files are gone
    for file in &files {
        assert!(!file.exists(), "File should be deleted: {}", file.display());
    }

    // Step 5: Simulate delete events and detect anomaly
    let delete_events: Vec<FileEvent> = files
        .iter()
        .map(|path| FileEvent {
            path: path.clone(),
            kind: FileEventKind::Deleted,
            timestamp: Utc::now(),
            size_bytes: Some(100),
        })
        .collect();
    let delete_batch = make_batch(delete_events);

    let rule_engine2 = RuleEngine::new(config.anomaly_rules.clone(), config.watch_dirs.clone());
    let delete_stats = BatchStats::from_batch(&delete_batch);
    let alerts = rule_engine2.analyze(&delete_batch, &delete_stats);
    assert!(
        !alerts.is_empty(),
        "Bulk delete of 30 files should trigger anomaly"
    );

    // Verify it's a HIGH severity alert (RULE-01: bulk delete > 20)
    let has_high = alerts.iter().any(|a| a.severity == AnomalySeverity::High);
    assert!(has_high, "Should have HIGH severity alert for bulk delete");

    // Step 6: Simulate creating anomaly snapshot
    let anomaly_snapshot = Snapshot {
        id: uuid::Uuid::new_v4(),
        timestamp: Utc::now(),
        trigger: SnapshotTrigger::Anomaly,
        os_snapshot_ref: "mock-snapshot-anomaly-001".to_string(),
        files_added: 0,
        files_modified: 0,
        files_deleted: 30,
        pinned: false,
        metadata_json: Some(
            serde_json::json!({
                "anomaly": true,
                "severity": "HIGH",
                "deleted_count": 30
            })
            .to_string(),
        ),
    };
    db.save_snapshot(&anomaly_snapshot).unwrap();

    // Verify both snapshots exist
    let snapshots = db.list_snapshots().unwrap();
    assert_eq!(snapshots.len(), 2);

    // Step 7: Simulate restore — recreate all files with original content
    let restored_files = create_test_files(&watch_dir, 30);
    assert_eq!(restored_files.len(), 30);

    // Step 8: Verify all 30 files restored with correct content
    for (i, file) in restored_files.iter().enumerate() {
        assert!(file.exists(), "Restored file should exist: {}", file.display());
        let content = fs::read_to_string(file).unwrap();
        let expected = format!(
            "Test file {} content. Line 1.\nLine 2 with data: {}\nLine 3 final.\n",
            i,
            i * 42
        );
        assert_eq!(
            content, expected,
            "Restored file {} content mismatch",
            file.display()
        );
    }
}

// ========================================================================
// Integration Test: Database Integrity
// ========================================================================

#[test]
fn test_database_integrity_check_passes() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");

    let db = Database::open(&db_path).unwrap();
    db.initialize().unwrap();
    drop(db);

    // Integrity check should pass
    assert!(lifecycle::check_db_integrity(&db_path).is_ok());
}

#[test]
fn test_database_reinitializes_missing_tables() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("empty.db");

    // Create empty database
    let _conn = rusqlite::Connection::open(&db_path).unwrap();
    drop(_conn);

    // Should re-initialize missing table
    assert!(lifecycle::check_db_integrity(&db_path).is_ok());
}

// ========================================================================
// Integration Test: LaunchAgent Plist Generation
// ========================================================================

// ========================================================================
// Integration Test: Anomaly Detection Rules
// ========================================================================

#[test]
fn test_anomaly_detection_bulk_delete_triggers_alert() {
    let tmp = TempDir::new().unwrap();
    let watch_dir = tmp.path().join("project");
    fs::create_dir_all(&watch_dir).unwrap();

    let config = RewConfig::default();
    let rule_engine = RuleEngine::new(config.anomaly_rules.clone(), vec![watch_dir.clone()]);

    // 25 file deletions should trigger RULE-01 (HIGH: bulk_delete_high > 20)
    let events: Vec<FileEvent> = (0..25)
        .map(|i| FileEvent {
            path: watch_dir.join(format!("file_{}.txt", i)),
            kind: FileEventKind::Deleted,
            timestamp: Utc::now(),
            size_bytes: Some(1000),
        })
        .collect();

    let batch = make_batch(events);
    let stats = BatchStats::from_batch(&batch);
    let alerts = rule_engine.analyze(&batch, &stats);

    assert!(!alerts.is_empty(), "25 file deletions should trigger alert");
    assert!(
        alerts.iter().any(|a| a.severity == AnomalySeverity::High),
        "Should have HIGH severity"
    );
}

#[test]
fn test_anomaly_detection_root_dir_delete_critical() {
    let tmp = TempDir::new().unwrap();
    let watch_dir = tmp.path().join("project");
    fs::create_dir_all(&watch_dir).unwrap();

    let config = RewConfig::default();
    let rule_engine = RuleEngine::new(config.anomaly_rules.clone(), vec![watch_dir.clone()]);

    // Delete the root watch directory → CRITICAL
    let events = vec![FileEvent {
        path: watch_dir.clone(),
        kind: FileEventKind::Deleted,
        timestamp: Utc::now(),
        size_bytes: None,
    }];

    let batch = make_batch(events);
    let stats = BatchStats::from_batch(&batch);
    let alerts = rule_engine.analyze(&batch, &stats);

    assert!(!alerts.is_empty(), "Root dir deletion should trigger alert");
    assert!(
        alerts
            .iter()
            .any(|a| a.severity == AnomalySeverity::Critical),
        "Root dir deletion should be CRITICAL"
    );
}

#[test]
fn test_anomaly_detection_sensitive_config_modified() {
    let tmp = TempDir::new().unwrap();
    let watch_dir = tmp.path().join("project");
    fs::create_dir_all(&watch_dir).unwrap();

    let config = RewConfig::default();
    let rule_engine = RuleEngine::new(config.anomaly_rules.clone(), vec![watch_dir.clone()]);

    // Modify .env file → HIGH (RULE-06)
    let events = vec![FileEvent {
        path: watch_dir.join(".env"),
        kind: FileEventKind::Modified,
        timestamp: Utc::now(),
        size_bytes: Some(100),
    }];

    let batch = make_batch(events);
    let stats = BatchStats::from_batch(&batch);
    let alerts = rule_engine.analyze(&batch, &stats);

    assert!(!alerts.is_empty(), ".env modification should trigger alert");
    assert!(
        alerts.iter().any(|a| a.severity == AnomalySeverity::High),
        ".env modification should be HIGH severity"
    );
}

// ========================================================================
// Integration Test: Config Management
// ========================================================================

#[test]
fn test_config_save_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");

    let mut config = RewConfig::default();
    config.watch_dirs.push(PathBuf::from("/tmp/test-project"));
    config.anomaly_rules.bulk_delete_high = 50;

    config.save(&config_path).unwrap();
    let loaded = RewConfig::load(&config_path).unwrap();

    assert_eq!(loaded.watch_dirs.len(), config.watch_dirs.len());
    assert_eq!(loaded.anomaly_rules.bulk_delete_high, 50);
}

// ========================================================================
// Integration Test: PID File Management
// ========================================================================

#[test]
fn test_pid_file_lifecycle() {
    let tmp = TempDir::new().unwrap();

    // Initially no PID file
    assert!(lifecycle::read_pid_file(tmp.path()).is_none());

    // Write PID
    lifecycle::write_pid_file(tmp.path()).unwrap();
    let pid = lifecycle::read_pid_file(tmp.path()).unwrap();
    assert_eq!(pid, std::process::id());

    // Remove PID
    lifecycle::remove_pid_file(tmp.path());
    assert!(lifecycle::read_pid_file(tmp.path()).is_none());
}

// ========================================================================
// Integration Test: Snapshot Database Operations
// ========================================================================

#[test]
fn test_snapshot_crud_operations() {
    let tmp = TempDir::new().unwrap();
    let db = test_db(tmp.path());

    // Create
    let snapshot = Snapshot {
        id: uuid::Uuid::new_v4(),
        timestamp: Utc::now(),
        trigger: SnapshotTrigger::Auto,
        os_snapshot_ref: "test-snap-001".to_string(),
        files_added: 10,
        files_modified: 5,
        files_deleted: 2,
        pinned: false,
        metadata_json: None,
    };
    db.save_snapshot(&snapshot).unwrap();

    // Read
    let loaded = db.get_snapshot(&snapshot.id).unwrap().unwrap();
    assert_eq!(loaded.id, snapshot.id);
    assert_eq!(loaded.files_added, 10);

    // Pin
    db.set_pinned(&snapshot.id, true).unwrap();
    let pinned = db.get_snapshot(&snapshot.id).unwrap().unwrap();
    assert!(pinned.pinned);

    // List
    let all = db.list_snapshots().unwrap();
    assert_eq!(all.len(), 1);

    // Delete
    db.delete_snapshot(&snapshot.id).unwrap();
    assert!(db.get_snapshot(&snapshot.id).unwrap().is_none());
}

// ========================================================================
// Integration Test: Event Batch Statistics
// ========================================================================

#[test]
fn test_event_batch_count_by_kind() {
    let events = vec![
        FileEvent {
            path: PathBuf::from("/test/a.txt"),
            kind: FileEventKind::Created,
            timestamp: Utc::now(),
            size_bytes: Some(100),
        },
        FileEvent {
            path: PathBuf::from("/test/b.txt"),
            kind: FileEventKind::Deleted,
            timestamp: Utc::now(),
            size_bytes: Some(200),
        },
        FileEvent {
            path: PathBuf::from("/test/c.txt"),
            kind: FileEventKind::Deleted,
            timestamp: Utc::now(),
            size_bytes: Some(300),
        },
    ];

    let batch = make_batch(events);

    assert_eq!(batch.count_by_kind(&FileEventKind::Created), 1);
    assert_eq!(batch.count_by_kind(&FileEventKind::Deleted), 2);
    assert_eq!(batch.count_by_kind(&FileEventKind::Modified), 0);
    assert_eq!(batch.total_deleted_size(), 500);
}

// ========================================================================
// Integration Test: Backup and Restore
// ========================================================================

#[test]
fn test_backup_modified_and_restore() {
    let temp_dir = tempdir().unwrap();
    let source_dir = temp_dir.path().join("source");
    let backup_dir = temp_dir.path().join("backups");
    fs::create_dir_all(&source_dir).unwrap();

    // Create test files
    let file1 = source_dir.join("document.txt");
    fs::write(&file1, "Important document").unwrap();

    // Create a config and backup engine
    let config = RewConfig::default();
    let engine = BackupEngine::new(&config).unwrap();

    // Create a backup job
    let snapshot_id = Uuid::new_v4();
    let events = vec![FileEvent {
        path: file1.clone(),
        kind: FileEventKind::Modified,
        timestamp: Utc::now(),
        size_bytes: Some(18),
    }];

    let job = BackupJob {
        snapshot_id,
        events,
        backup_root: backup_dir.clone(),
    };

    // Perform backup
    let backup_result = engine.backup_batch(&job).unwrap();
    assert_eq!(backup_result.files_backed_up, 1);
    assert!(backup_result.total_size_bytes > 0);

    // Verify backup file exists
    let backed_up_file = backup_dir
        .join(snapshot_id.to_string())
        .join(file1.strip_prefix("/").unwrap_or(&file1));
    assert!(backed_up_file.exists());

    // Verify backup content
    let original_content = fs::read_to_string(&file1).unwrap();
    let backed_up_content = fs::read_to_string(&backed_up_file).unwrap();
    assert_eq!(original_content, backed_up_content);
}

#[test]
fn test_backup_respects_ignore_patterns() {
    let temp_dir = tempdir().unwrap();
    let source_dir = temp_dir.path().join("source");
    let backup_dir = temp_dir.path().join("backups");
    fs::create_dir_all(&source_dir).unwrap();

    // Use node_modules here instead of .git/config because some sandboxed
    // environments reject writes to git-like metadata paths.
    let ignored_dir = source_dir.join("node_modules");
    fs::create_dir(&ignored_dir).unwrap();
    let config_file = ignored_dir.join("package.js");
    fs::write(&config_file, "exports = {}").unwrap();

    // Create regular file
    let normal_file = source_dir.join("main.rs");
    fs::write(&normal_file, "fn main() {}").unwrap();

    let config = RewConfig::default();
    let engine = BackupEngine::new(&config).unwrap();

    let snapshot_id = Uuid::new_v4();
    let events = vec![
        FileEvent {
            path: config_file.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(7),
        },
        FileEvent {
            path: normal_file.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(12),
        },
    ];

    let job = BackupJob {
        snapshot_id,
        events,
        backup_root: backup_dir.clone(),
    };

    let backup_result = engine.backup_batch(&job).unwrap();
    // Should only backup the normal file, ignored files are skipped.
    assert_eq!(backup_result.files_backed_up, 1);
}

#[test]
fn test_deleted_file_handling() {
    let temp_dir = tempdir().unwrap();
    let backup_dir = temp_dir.path().join("backups");

    let config = RewConfig::default();
    let engine = BackupEngine::new(&config).unwrap();

    // Non-existent file event
    let deleted_file = temp_dir.path().join("never_existed.txt");

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

    // Should handle gracefully
    let backup_result = engine.backup_batch(&job).unwrap();
    assert_eq!(backup_result.files_backed_up, 0);
    assert_eq!(backup_result.failed_files.len(), 1);
}

#[test]
fn test_backup_respects_ignore_patterns_debug() {
    let temp_dir = tempdir().unwrap();
    let source_dir = temp_dir.path().join("source");
    let backup_dir = temp_dir.path().join("backups");
    fs::create_dir_all(&source_dir).unwrap();

    // Use node_modules here instead of .git/config because some sandboxed
    // environments reject writes to git-like metadata paths.
    let ignored_dir = source_dir.join("node_modules");
    fs::create_dir(&ignored_dir).unwrap();
    let config_file = ignored_dir.join("package.js");
    fs::write(&config_file, "exports = {}").unwrap();

    println!("Created ignored file at: {}", config_file.display());

    // Create regular file
    let normal_file = source_dir.join("main.rs");
    fs::write(&normal_file, "fn main() {}").unwrap();

    println!("Created normal file at: {}", normal_file.display());

    let config = RewConfig::default();
    println!("Ignore patterns: {:?}", config.ignore_patterns);
    
    let engine = BackupEngine::new(&config).unwrap();

    let snapshot_id = Uuid::new_v4();
    let events = vec![
        FileEvent {
            path: config_file.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(7),
        },
        FileEvent {
            path: normal_file.clone(),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: Some(12),
        },
    ];

    let job = BackupJob {
        snapshot_id,
        events,
        backup_root: backup_dir.clone(),
    };

    let backup_result = engine.backup_batch(&job).unwrap();
    println!("Files backed up: {}", backup_result.files_backed_up);
    println!("Failed files: {:?}", backup_result.failed_files);
    
    // Should only backup the normal file, ignored files are skipped.
    assert_eq!(backup_result.files_backed_up, 1, "Expected 1 file backed up, got {}", backup_result.files_backed_up);
}
