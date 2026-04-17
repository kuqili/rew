//! Anomaly detection rules (RULE-01 through RULE-07).
//!
//! Each rule evaluates an EventBatch and optionally returns an AnomalySignal.

use crate::config::AnomalyRulesConfig;
use crate::processor::BatchStats;
use crate::types::{AnomalyKind, AnomalySeverity, AnomalySignal, EventBatch, FileEventKind};
use chrono::Utc;
use std::path::PathBuf;

/// A single anomaly detection rule.
pub trait Rule: Send + Sync {
    /// Rule identifier (e.g. "RULE-01").
    fn id(&self) -> &str;

    /// Evaluate the batch. Returns Some(signal) if the rule fires.
    ///
    /// `stats` contains pre-aggregated counts (O(1) lookups) that can be used
    /// for early short-circuit checks before iterating over individual events.
    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        config: &AnomalyRulesConfig,
        watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal>;
}

/// RULE-01: Bulk delete > 20 files in 30s window -> HIGH
pub struct Rule01BulkDeleteHigh;

impl Rule for Rule01BulkDeleteHigh {
    fn id(&self) -> &str {
        "RULE-01"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        config: &AnomalyRulesConfig,
        _watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        // O(1) early exit: if deleted count doesn't exceed threshold, skip iteration
        if (stats.files_deleted as u32) <= config.bulk_delete_high {
            return None;
        }
        let deleted: Vec<PathBuf> = batch
            .events
            .iter()
            .filter(|e| e.kind == FileEventKind::Deleted)
            .map(|e| e.path.clone())
            .collect();

        if deleted.len() as u32 > config.bulk_delete_high {
            let dirs = extract_directories(&deleted);
            Some(AnomalySignal {
                kind: AnomalyKind::BulkDelete,
                severity: AnomalySeverity::High,
                affected_files: deleted.clone(),
                detected_at: Utc::now(),
                description: format!(
                    "RULE-01: {} files deleted in 30s (threshold: {}). Directories: {}",
                    deleted.len(),
                    config.bulk_delete_high,
                    dirs.join(", ")
                ),
            })
        } else {
            None
        }
    }
}

/// RULE-02: Bulk delete > 5 files in 30s window -> MEDIUM
pub struct Rule02BulkDeleteMedium;

impl Rule for Rule02BulkDeleteMedium {
    fn id(&self) -> &str {
        "RULE-02"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        config: &AnomalyRulesConfig,
        _watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        // O(1) early exit: RULE-02 only fires in the medium range
        let quick_count = stats.files_deleted as u32;
        if quick_count <= config.bulk_delete_medium || quick_count > config.bulk_delete_high {
            return None;
        }
        let deleted: Vec<PathBuf> = batch
            .events
            .iter()
            .filter(|e| e.kind == FileEventKind::Deleted)
            .map(|e| e.path.clone())
            .collect();

        let count = deleted.len() as u32;
        // Only fire if below RULE-01 threshold (RULE-01 takes priority)
        if count > config.bulk_delete_medium && count <= config.bulk_delete_high {
            let dirs = extract_directories(&deleted);
            Some(AnomalySignal {
                kind: AnomalyKind::BulkDelete,
                severity: AnomalySeverity::Medium,
                affected_files: deleted.clone(),
                detected_at: Utc::now(),
                description: format!(
                    "RULE-02: {} files deleted in 30s (threshold: {}). Directories: {}",
                    deleted.len(),
                    config.bulk_delete_medium,
                    dirs.join(", ")
                ),
            })
        } else {
            None
        }
    }
}

/// RULE-03: Total deletion size > 100MB in 30s window -> HIGH
pub struct Rule03LargeDeletion;

impl Rule for Rule03LargeDeletion {
    fn id(&self) -> &str {
        "RULE-03"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        config: &AnomalyRulesConfig,
        _watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        // Use pre-aggregated total from stats (O(1)), fallback to batch method
        let total_size = stats.total_deleted_size;
        if total_size > config.delete_size_high_bytes {
            let deleted: Vec<PathBuf> = batch
                .events
                .iter()
                .filter(|e| e.kind == FileEventKind::Deleted)
                .map(|e| e.path.clone())
                .collect();
            let dirs = extract_directories(&deleted);
            Some(AnomalySignal {
                kind: AnomalyKind::LargeDeletion,
                severity: AnomalySeverity::High,
                affected_files: deleted,
                detected_at: Utc::now(),
                description: format!(
                    "RULE-03: {:.1}MB deleted in 30s (threshold: {:.1}MB). Directories: {}",
                    total_size as f64 / 1_048_576.0,
                    config.delete_size_high_bytes as f64 / 1_048_576.0,
                    dirs.join(", ")
                ),
            })
        } else {
            None
        }
    }
}

/// RULE-04: Bulk modify > 50 files in 30s window -> MEDIUM
pub struct Rule04BulkModify;

impl Rule for Rule04BulkModify {
    fn id(&self) -> &str {
        "RULE-04"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        config: &AnomalyRulesConfig,
        _watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        // O(1) early exit
        if (stats.files_modified as u32) <= config.bulk_modify_medium {
            return None;
        }
        let modified: Vec<PathBuf> = batch
            .events
            .iter()
            .filter(|e| e.kind == FileEventKind::Modified)
            .map(|e| e.path.clone())
            .collect();

        if modified.len() as u32 > config.bulk_modify_medium {
            let dirs = extract_directories(&modified);
            Some(AnomalySignal {
                kind: AnomalyKind::BulkModify,
                severity: AnomalySeverity::Medium,
                affected_files: modified.clone(),
                detected_at: Utc::now(),
                description: format!(
                    "RULE-04: {} files modified in 30s (threshold: {}). Directories: {}",
                    modified.len(),
                    config.bulk_modify_medium,
                    dirs.join(", ")
                ),
            })
        } else {
            None
        }
    }
}

/// RULE-05: Watch root directory deleted -> CRITICAL
pub struct Rule05RootDirDeleted;

impl Rule for Rule05RootDirDeleted {
    fn id(&self) -> &str {
        "RULE-05"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        _config: &AnomalyRulesConfig,
        watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        // O(1) early exit: no deleted events means root dir can't be deleted
        if stats.files_deleted == 0 {
            return None;
        }
        for event in &batch.events {
            if event.kind == FileEventKind::Deleted {
                for watch_dir in watch_dirs {
                    if &event.path == watch_dir {
                        return Some(AnomalySignal {
                            kind: AnomalyKind::RootDirDeleted,
                            severity: AnomalySeverity::Critical,
                            affected_files: vec![event.path.clone()],
                            detected_at: Utc::now(),
                            description: format!(
                                "RULE-05: CRITICAL - Watch root directory deleted: {}",
                                event.path.display()
                            ),
                        });
                    }
                }
            }
        }
        None
    }
}

/// RULE-06: Sensitive config file modified -> HIGH
pub struct Rule06SensitiveConfig;

impl Rule for Rule06SensitiveConfig {
    fn id(&self) -> &str {
        "RULE-06"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        config: &AnomalyRulesConfig,
        _watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        // O(1) early exit: only Modified and Deleted events can match sensitive files
        if stats.files_modified == 0 && stats.files_deleted == 0 {
            return None;
        }
        let mut affected = Vec::new();

        for event in &batch.events {
            if event.kind == FileEventKind::Modified || event.kind == FileEventKind::Deleted {
                let path_str = event.path.to_string_lossy();
                let file_name = event
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                for sensitive in &config.sensitive_files {
                    // Match by file name or path suffix
                    if file_name == *sensitive || path_str.ends_with(sensitive) {
                        affected.push(event.path.clone());
                        break;
                    }
                }
            }
        }

        if !affected.is_empty() {
            let file_list: Vec<String> = affected
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            Some(AnomalySignal {
                kind: AnomalyKind::SensitiveConfigModified,
                severity: AnomalySeverity::High,
                affected_files: affected,
                detected_at: Utc::now(),
                description: format!(
                    "RULE-06: Sensitive config file(s) modified: {}",
                    file_list.join(", ")
                ),
            })
        } else {
            None
        }
    }
}

/// RULE-07: Non-package modifications > 30 in 30s -> MEDIUM
/// Excludes node_modules, target, and other package manager directories.
pub struct Rule07NonPackageModify;

impl Rule for Rule07NonPackageModify {
    fn id(&self) -> &str {
        "RULE-07"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        stats: &BatchStats,
        config: &AnomalyRulesConfig,
        _watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        // O(1) early exit: if total modified count doesn't exceed threshold, skip
        if (stats.files_modified as u32) <= config.non_package_modify_medium {
            return None;
        }
        let non_package_modified: Vec<PathBuf> = batch
            .events
            .iter()
            .filter(|e| e.kind == FileEventKind::Modified)
            .filter(|e| !is_package_path(&e.path))
            .map(|e| e.path.clone())
            .collect();

        if non_package_modified.len() as u32 > config.non_package_modify_medium {
            let dirs = extract_directories(&non_package_modified);
            Some(AnomalySignal {
                kind: AnomalyKind::LargeNonPackageModify,
                severity: AnomalySeverity::Medium,
                affected_files: non_package_modified.clone(),
                detected_at: Utc::now(),
                description: format!(
                    "RULE-07: {} non-package files modified in 30s (threshold: {}). Directories: {}",
                    non_package_modified.len(),
                    config.non_package_modify_medium,
                    dirs.join(", ")
                ),
            })
        } else {
            None
        }
    }
}

/// Check if a path is inside a package manager directory (node_modules, target, etc.)
fn is_package_path(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.contains("/node_modules/")
        || path_str.contains("/target/")
        || path_str.contains("/__pycache__/")
        || path_str.contains("/.venv/")
        || path_str.contains("/vendor/")
}

/// Extract unique parent directories from a list of paths.
fn extract_directories(paths: &[PathBuf]) -> Vec<String> {
    let mut dirs: Vec<String> = paths
        .iter()
        .filter_map(|p| p.parent().map(|d| d.display().to_string()))
        .collect();
    dirs.sort();
    dirs.dedup();
    // Limit to first 5 directories for readability
    dirs.truncate(5);
    dirs
}

/// RULE-08: AI operated on files outside the project directory -> HIGH
/// Detects when file operations affect paths outside any watched directory.
pub struct Rule08OutOfScopeOperation;

impl Rule for Rule08OutOfScopeOperation {
    fn id(&self) -> &str {
        "RULE-08"
    }

    fn evaluate(
        &self,
        batch: &EventBatch,
        _stats: &BatchStats,
        _config: &AnomalyRulesConfig,
        watch_dirs: &[PathBuf],
    ) -> Option<AnomalySignal> {
        if watch_dirs.is_empty() {
            return None;
        }

        let out_of_scope: Vec<PathBuf> = batch
            .events
            .iter()
            .filter(|e| {
                // Check if the event path is outside ALL watched directories
                !watch_dirs.iter().any(|wd| e.path.starts_with(wd))
            })
            .map(|e| e.path.clone())
            .collect();

        if !out_of_scope.is_empty() {
            let dirs = extract_directories(&out_of_scope);
            Some(AnomalySignal {
                kind: AnomalyKind::OutOfScope,
                severity: AnomalySeverity::High,
                affected_files: out_of_scope.clone(),
                detected_at: Utc::now(),
                description: format!(
                    "RULE-08: {} file(s) operated outside project scope. Directories: {}",
                    out_of_scope.len(),
                    dirs.join(", ")
                ),
            })
        } else {
            None
        }
    }
}

/// Returns all built-in rules.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(Rule05RootDirDeleted),    // CRITICAL first
        Box::new(Rule01BulkDeleteHigh),    // HIGH
        Box::new(Rule03LargeDeletion),     // HIGH
        Box::new(Rule06SensitiveConfig),   // HIGH
        Box::new(Rule08OutOfScopeOperation), // HIGH — out of project scope
        Box::new(Rule02BulkDeleteMedium),  // MEDIUM
        Box::new(Rule04BulkModify),        // MEDIUM
        Box::new(Rule07NonPackageModify),  // MEDIUM
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processor::BatchStats;
    use crate::types::FileEvent;
    use chrono::Utc;

    fn make_event(path: &str, kind: FileEventKind, size: Option<u64>) -> FileEvent {
        FileEvent {
            path: PathBuf::from(path),
            kind,
            timestamp: Utc::now(),
            size_bytes: size,
        }
    }

    fn make_batch(events: Vec<FileEvent>) -> EventBatch {
        EventBatch {
            events,
            window_start: Utc::now(),
            window_end: Utc::now(),
        }
    }

    fn eval<R: Rule>(rule: &R, batch: &EventBatch, config: &AnomalyRulesConfig, watch_dirs: &[PathBuf]) -> Option<AnomalySignal> {
        let stats = BatchStats::from_batch(batch);
        rule.evaluate(batch, &stats, config, watch_dirs)
    }

    fn default_config() -> AnomalyRulesConfig {
        AnomalyRulesConfig::default()
    }

    #[test]
    fn test_rule01_bulk_delete_high() {
        let rule = Rule01BulkDeleteHigh;
        let config = default_config();

        // 25 deletes -> should fire (threshold is 20)
        let events: Vec<FileEvent> = (0..25)
            .map(|i| make_event(&format!("/tmp/file_{}.txt", i), FileEventKind::Deleted, Some(100)))
            .collect();
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &[]);
        assert!(signal.is_some());
        let s = signal.unwrap();
        assert_eq!(s.severity, AnomalySeverity::High);
        assert_eq!(s.kind, AnomalyKind::BulkDelete);
        assert_eq!(s.affected_files.len(), 25);
        assert!(s.description.contains("25"));
        assert!(s.description.contains("/tmp"));
    }

    #[test]
    fn test_rule01_below_threshold() {
        let rule = Rule01BulkDeleteHigh;
        let config = default_config();

        // 15 deletes -> below threshold of 20
        let events: Vec<FileEvent> = (0..15)
            .map(|i| make_event(&format!("/tmp/file_{}.txt", i), FileEventKind::Deleted, Some(100)))
            .collect();
        let batch = make_batch(events);
        assert!(eval(&rule, &batch, &config, &[]).is_none());
    }

    #[test]
    fn test_rule02_medium_delete() {
        let rule = Rule02BulkDeleteMedium;
        let config = default_config();

        // 10 deletes -> between 5 and 20 -> MEDIUM
        let events: Vec<FileEvent> = (0..10)
            .map(|i| make_event(&format!("/tmp/file_{}.txt", i), FileEventKind::Deleted, Some(100)))
            .collect();
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &[]);
        assert!(signal.is_some());
        assert_eq!(signal.unwrap().severity, AnomalySeverity::Medium);
    }

    #[test]
    fn test_rule02_not_when_high() {
        let rule = Rule02BulkDeleteMedium;
        let config = default_config();

        // 25 deletes -> above HIGH threshold, RULE-02 should NOT fire
        let events: Vec<FileEvent> = (0..25)
            .map(|i| make_event(&format!("/tmp/file_{}.txt", i), FileEventKind::Deleted, Some(100)))
            .collect();
        let batch = make_batch(events);
        assert!(eval(&rule, &batch, &config, &[]).is_none());
    }

    #[test]
    fn test_rule03_large_deletion() {
        let rule = Rule03LargeDeletion;
        let config = default_config();

        // 5 files of 30MB each = 150MB > 100MB threshold
        let events: Vec<FileEvent> = (0..5)
            .map(|i| {
                make_event(
                    &format!("/tmp/big_{}.bin", i),
                    FileEventKind::Deleted,
                    Some(30 * 1024 * 1024),
                )
            })
            .collect();
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &[]);
        assert!(signal.is_some());
        let s = signal.unwrap();
        assert_eq!(s.severity, AnomalySeverity::High);
        assert_eq!(s.kind, AnomalyKind::LargeDeletion);
    }

    #[test]
    fn test_rule04_bulk_modify() {
        let rule = Rule04BulkModify;
        let config = default_config();

        // 55 modified files > 50 threshold
        let events: Vec<FileEvent> = (0..55)
            .map(|i| make_event(&format!("/proj/src/mod_{}.rs", i), FileEventKind::Modified, Some(1000)))
            .collect();
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &[]);
        assert!(signal.is_some());
        assert_eq!(signal.unwrap().severity, AnomalySeverity::Medium);
    }

    #[test]
    fn test_rule05_root_dir_deleted() {
        let rule = Rule05RootDirDeleted;
        let config = default_config();
        let watch_dirs = vec![PathBuf::from("/home/user/Documents")];

        let events = vec![make_event(
            "/home/user/Documents",
            FileEventKind::Deleted,
            None,
        )];
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &watch_dirs);
        assert!(signal.is_some());
        let s = signal.unwrap();
        assert_eq!(s.severity, AnomalySeverity::Critical);
        assert_eq!(s.kind, AnomalyKind::RootDirDeleted);
        assert!(s.description.contains("/home/user/Documents"));
    }

    #[test]
    fn test_rule05_non_root_delete_no_fire() {
        let rule = Rule05RootDirDeleted;
        let config = default_config();
        let watch_dirs = vec![PathBuf::from("/home/user/Documents")];

        let events = vec![make_event(
            "/home/user/Documents/subdir",
            FileEventKind::Deleted,
            None,
        )];
        let batch = make_batch(events);
        assert!(eval(&rule, &batch, &config, &watch_dirs).is_none());
    }

    #[test]
    fn test_rule06_sensitive_config() {
        let rule = Rule06SensitiveConfig;
        let config = default_config();

        let events = vec![make_event("/project/.env", FileEventKind::Modified, Some(256))];
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &[]);
        assert!(signal.is_some());
        let s = signal.unwrap();
        assert_eq!(s.severity, AnomalySeverity::High);
        assert_eq!(s.kind, AnomalyKind::SensitiveConfigModified);
        assert!(s.description.contains(".env"));
    }

    #[test]
    fn test_rule06_env_local() {
        let rule = Rule06SensitiveConfig;
        let config = default_config();

        let events = vec![make_event(
            "/project/.env.local",
            FileEventKind::Modified,
            Some(128),
        )];
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &[]);
        assert!(signal.is_some());
        assert!(signal.unwrap().description.contains(".env.local"));
    }

    #[test]
    fn test_rule06_normal_file_no_fire() {
        let rule = Rule06SensitiveConfig;
        let config = default_config();

        let events = vec![make_event(
            "/project/src/main.rs",
            FileEventKind::Modified,
            Some(5000),
        )];
        let batch = make_batch(events);
        assert!(eval(&rule, &batch, &config, &[]).is_none());
    }

    #[test]
    fn test_rule07_non_package_modify() {
        let rule = Rule07NonPackageModify;
        let config = default_config();

        // 35 non-package modified files > 30 threshold
        let events: Vec<FileEvent> = (0..35)
            .map(|i| make_event(&format!("/proj/src/file_{}.rs", i), FileEventKind::Modified, Some(500)))
            .collect();
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &[]);
        assert!(signal.is_some());
        assert_eq!(signal.unwrap().severity, AnomalySeverity::Medium);
    }

    #[test]
    fn test_rule07_package_files_excluded() {
        let rule = Rule07NonPackageModify;
        let config = default_config();

        // 40 files, but 35 are in node_modules -> only 5 non-package files -> below threshold
        let mut events: Vec<FileEvent> = (0..35)
            .map(|i| {
                make_event(
                    &format!("/proj/node_modules/pkg/file_{}.js", i),
                    FileEventKind::Modified,
                    Some(100),
                )
            })
            .collect();
        for i in 0..5 {
            events.push(make_event(
                &format!("/proj/src/file_{}.rs", i),
                FileEventKind::Modified,
                Some(500),
            ));
        }
        let batch = make_batch(events);
        assert!(eval(&rule, &batch, &config, &[]).is_none());
    }

    #[test]
    fn test_is_package_path() {
        assert!(is_package_path(&PathBuf::from("/proj/node_modules/foo/index.js")));
        assert!(is_package_path(&PathBuf::from("/proj/target/debug/build/x")));
        assert!(is_package_path(&PathBuf::from("/proj/__pycache__/mod.pyc")));
        assert!(!is_package_path(&PathBuf::from("/proj/src/main.rs")));
        assert!(!is_package_path(&PathBuf::from("/proj/README.md")));
    }

    #[test]
    fn test_all_rules_count() {
        let rules = all_rules();
        assert_eq!(rules.len(), 8);
    }

    #[test]
    fn test_rule08_out_of_scope() {
        let rule = Rule08OutOfScopeOperation;
        let config = default_config();
        let watch_dirs = vec![PathBuf::from("/Users/alice/project")];

        // File outside project scope
        let events = vec![make_event(
            "/Users/alice/Desktop/important.docx",
            FileEventKind::Deleted,
            Some(1024),
        )];
        let batch = make_batch(events);
        let signal = eval(&rule, &batch, &config, &watch_dirs);
        assert!(signal.is_some());
        let s = signal.unwrap();
        assert_eq!(s.severity, AnomalySeverity::High);
        assert_eq!(s.kind, AnomalyKind::OutOfScope);
    }

    #[test]
    fn test_rule08_in_scope_no_fire() {
        let rule = Rule08OutOfScopeOperation;
        let config = default_config();
        let watch_dirs = vec![PathBuf::from("/Users/alice/project")];

        // File inside project scope
        let events = vec![make_event(
            "/Users/alice/project/src/main.rs",
            FileEventKind::Modified,
            Some(500),
        )];
        let batch = make_batch(events);
        assert!(eval(&rule, &batch, &config, &watch_dirs).is_none());
    }

    #[test]
    fn test_extract_directories_dedup() {
        let paths = vec![
            PathBuf::from("/proj/src/a.rs"),
            PathBuf::from("/proj/src/b.rs"),
            PathBuf::from("/proj/tests/c.rs"),
        ];
        let dirs = extract_directories(&paths);
        assert_eq!(dirs.len(), 2);
        assert!(dirs.contains(&"/proj/src".to_string()));
        assert!(dirs.contains(&"/proj/tests".to_string()));
    }
}
