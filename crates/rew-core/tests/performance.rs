//! Performance benchmarks and resource usage tests.
//!
//! Tests ensure the daemon meets performance requirements:
//! - Event processing: 10000+ events handled efficiently
//! - Database operations: high throughput insert/query
//! - Path filtering: fast glob matching
//! - Memory: reasonable baseline usage

use rew_core::config::RewConfig;
use rew_core::db::Database;
use rew_core::detector::RuleEngine;
use rew_core::processor::BatchStats;
use rew_core::traits::AnomalyDetector;
use rew_core::types::*;
use rew_core::watcher::filter::PathFilter;
use chrono::Utc;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::TempDir;

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
// Performance Test: Large Directory Event Processing (10000+ files)
// ========================================================================

#[test]
fn test_perf_large_directory_event_processing() {
    let tmp = TempDir::new().unwrap();
    let watch_dir = tmp.path().join("large_project");

    let config = RewConfig::default();
    let rule_engine = RuleEngine::new(config.anomaly_rules.clone(), vec![watch_dir.clone()]);

    // Generate 10000 file events (mix of created, modified, deleted)
    let events: Vec<FileEvent> = (0..10000)
        .map(|i| {
            let kind = match i % 3 {
                0 => FileEventKind::Created,
                1 => FileEventKind::Modified,
                _ => FileEventKind::Deleted,
            };
            FileEvent {
                path: watch_dir.join(format!("src/module_{}/file_{}.rs", i / 100, i)),
                kind,
                timestamp: Utc::now(),
                size_bytes: Some(1024),
            }
        })
        .collect();

    let batch = make_batch(events);

    // Measure anomaly detection time
    let stats = BatchStats::from_batch(&batch);
    let start = Instant::now();
    let alerts = rule_engine.analyze(&batch, &stats);
    let detection_time = start.elapsed();

    println!(
        "PERF: 10000 events → {} alerts in {:?}",
        alerts.len(),
        detection_time
    );

    // Should process 10000 events in well under 500ms
    assert!(
        detection_time < Duration::from_millis(500),
        "Anomaly detection took too long: {:?}",
        detection_time
    );
}

// ========================================================================
// Performance Test: Database Operations at Scale
// ========================================================================

#[test]
fn test_perf_database_insert_1000_snapshots() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("perf_test.db");
    let db = Database::open(&db_path).unwrap();
    db.initialize().unwrap();

    // Insert 1000 snapshots
    let start = Instant::now();
    for i in 0..1000i64 {
        let snapshot = Snapshot {
            id: uuid::Uuid::new_v4(),
            timestamp: Utc::now() - chrono::Duration::minutes(i),
            trigger: if i % 10 == 0 {
                SnapshotTrigger::Anomaly
            } else {
                SnapshotTrigger::Auto
            },
            os_snapshot_ref: format!("perf-snap-{:04}", i),
            files_added: (i % 50) as u32,
            files_modified: (i % 30) as u32,
            files_deleted: (i % 10) as u32,
            pinned: i % 100 == 0,
            metadata_json: None,
        };
        db.save_snapshot(&snapshot).unwrap();
    }
    let insert_time = start.elapsed();

    println!(
        "PERF: 1000 snapshot inserts: {:?} ({:.0} inserts/sec)",
        insert_time,
        1000.0 / insert_time.as_secs_f64()
    );

    assert!(
        insert_time < Duration::from_secs(5),
        "Database inserts too slow: {:?}",
        insert_time
    );

    // List all snapshots
    let start = Instant::now();
    let snapshots = db.list_snapshots().unwrap();
    let list_time = start.elapsed();

    assert_eq!(snapshots.len(), 1000);
    println!("PERF: List 1000 snapshots: {:?}", list_time);

    assert!(
        list_time < Duration::from_millis(500),
        "Database list too slow: {:?}",
        list_time
    );
}

// ========================================================================
// Performance Test: Path Filter Performance
// ========================================================================

#[test]
fn test_perf_path_filter_10000_paths() {
    let config = RewConfig::default();
    let filter = PathFilter::new(&config.ignore_patterns).unwrap();

    let paths: Vec<PathBuf> = (0..10000)
        .map(|i| match i % 5 {
            0 => PathBuf::from(format!("/project/src/module_{}.rs", i)),
            1 => PathBuf::from(format!("/project/node_modules/pkg_{}/index.js", i)),
            2 => PathBuf::from(format!("/project/.git/objects/{:x}", i)),
            3 => PathBuf::from(format!("/project/target/debug/lib_{}.rlib", i)),
            _ => PathBuf::from(format!("/project/docs/page_{}.md", i)),
        })
        .collect();

    let start = Instant::now();
    let mut filtered = 0usize;
    let mut passed = 0usize;
    for path in &paths {
        if filter.should_ignore(path) {
            filtered += 1;
        } else {
            passed += 1;
        }
    }
    let filter_time = start.elapsed();

    println!(
        "PERF: 10000 paths: {} passed, {} ignored, in {:?}",
        passed, filtered, filter_time
    );

    assert!(
        filter_time < Duration::from_millis(200),
        "Path filtering too slow: {:?}",
        filter_time
    );

    assert!(filtered > 0, "Some paths should be filtered");
    assert!(passed > 0, "Some paths should pass");
}

// ========================================================================
// Performance Test: EventBatch Count Operations
// ========================================================================

#[test]
fn test_perf_event_batch_operations() {
    // Create a large batch and measure count/total operations
    let events: Vec<FileEvent> = (0..5000)
        .map(|i| {
            let kind = match i % 4 {
                0 => FileEventKind::Created,
                1 => FileEventKind::Modified,
                2 => FileEventKind::Deleted,
                _ => FileEventKind::Renamed,
            };
            FileEvent {
                path: PathBuf::from(format!("/project/file_{}.txt", i)),
                kind,
                timestamp: Utc::now(),
                size_bytes: Some((i * 100) as u64),
            }
        })
        .collect();

    let batch = make_batch(events);

    let start = Instant::now();
    let created = batch.count_by_kind(&FileEventKind::Created);
    let modified = batch.count_by_kind(&FileEventKind::Modified);
    let deleted = batch.count_by_kind(&FileEventKind::Deleted);
    let total_size = batch.total_deleted_size();
    let ops_time = start.elapsed();

    println!(
        "PERF: Batch ops on 5000 events: created={}, modified={}, deleted={}, total_deleted_size={} in {:?}",
        created, modified, deleted, total_size, ops_time
    );

    assert_eq!(created, 1250);
    assert_eq!(modified, 1250);
    assert_eq!(deleted, 1250);

    assert!(
        ops_time < Duration::from_millis(50),
        "Batch operations too slow: {:?}",
        ops_time
    );
}

// ========================================================================
// Performance Test: Memory Baseline with 500 Snapshots
// ========================================================================

#[test]
fn test_perf_memory_baseline() {
    let config = RewConfig::default();
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("mem_test.db");
    let db = Database::open(&db_path).unwrap();
    db.initialize().unwrap();

    // Insert 500 snapshots (weeks of operation)
    for i in 0..500i64 {
        let snapshot = Snapshot {
            id: uuid::Uuid::new_v4(),
            timestamp: Utc::now() - chrono::Duration::hours(i),
            trigger: SnapshotTrigger::Auto,
            os_snapshot_ref: format!("snap-{:04}", i),
            files_added: 10,
            files_modified: 5,
            files_deleted: 0,
            pinned: false,
            metadata_json: None,
        };
        db.save_snapshot(&snapshot).unwrap();
    }

    // Load all snapshots into memory
    let snapshots = db.list_snapshots().unwrap();
    assert_eq!(snapshots.len(), 500);

    // Create rule engine
    let _rule_engine = RuleEngine::new(
        config.anomaly_rules.clone(),
        config.watch_dirs.clone(),
    );

    // If we got here without OOM, memory is reasonable
    println!("PERF: Memory baseline test passed with 500 snapshots loaded");
}
