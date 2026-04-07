//! Anomaly detection engine.
//!
//! The `RuleEngine` evaluates EventBatches against 7 anomaly detection rules
//! (RULE-01 through RULE-07), with priority merging and alert deduplication.
//!
//! Processing flow:
//!   EventBatch → evaluate all rules → priority merge → dedup filter → AnomalySignal[]
//!
//! Priority: CRITICAL > HIGH > MEDIUM. If multiple rules fire for the same anomaly
//! kind, only the highest severity is kept. CRITICAL interrupts immediately.

pub mod dedup;
pub mod rules;

use crate::config::AnomalyRulesConfig;
use crate::traits::AnomalyDetector;
use crate::types::{AnomalySeverity, AnomalySignal, EventBatch};
use dedup::AlertDeduplicator;
use rules::all_rules;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing::{debug, info, warn};

/// The rule-based anomaly detection engine.
///
/// Evaluates event batches against all 7 rules, merges by priority,
/// and deduplicates to prevent notification spam.
pub struct RuleEngine {
    /// Anomaly rule thresholds from config
    config: AnomalyRulesConfig,
    /// Directories being watched (for RULE-05)
    watch_dirs: Vec<PathBuf>,
    /// Alert deduplicator (mutable, behind mutex for Send+Sync)
    deduplicator: Mutex<AlertDeduplicator>,
}

impl RuleEngine {
    /// Create a new rule engine with the given configuration.
    pub fn new(config: AnomalyRulesConfig, watch_dirs: Vec<PathBuf>) -> Self {
        Self {
            config,
            watch_dirs,
            deduplicator: Mutex::new(AlertDeduplicator::new()),
        }
    }

    /// Create a rule engine with custom dedup cooldown.
    pub fn with_dedup_cooldown(
        config: AnomalyRulesConfig,
        watch_dirs: Vec<PathBuf>,
        cooldown: std::time::Duration,
    ) -> Self {
        Self {
            config,
            watch_dirs,
            deduplicator: Mutex::new(AlertDeduplicator::with_cooldown(cooldown)),
        }
    }

    /// Evaluate all rules against a batch, returning deduplicated signals.
    ///
    /// Rules are evaluated in priority order (CRITICAL first), and the results
    /// are deduped so the same (directory, rule) pair only alerts once per cooldown.
    fn evaluate_and_dedup(&self, batch: &EventBatch) -> Vec<AnomalySignal> {
        let rules = all_rules();
        let mut signals = Vec::new();

        // Evaluate all rules
        for rule in &rules {
            if let Some(signal) = rule.evaluate(batch, &self.config, &self.watch_dirs) {
                debug!(
                    "Rule {} fired: {} (severity: {})",
                    rule.id(),
                    signal.description,
                    signal.severity
                );
                signals.push(signal);
            }
        }

        if signals.is_empty() {
            return signals;
        }

        // Sort by severity descending (Critical > High > Medium)
        signals.sort_by(|a, b| b.severity.cmp(&a.severity));

        // If CRITICAL detected, log it prominently
        if signals
            .first()
            .map(|s| s.severity == AnomalySeverity::Critical)
            .unwrap_or(false)
        {
            warn!(
                "CRITICAL anomaly detected: {}",
                signals[0].description
            );
        }

        // Dedup: filter out suppressed signals
        let mut dedup = self.deduplicator.lock().unwrap();
        let mut result = Vec::new();
        for signal in signals {
            if dedup.should_alert(&signal) {
                info!(
                    "Anomaly alert: [{}] {} — {}",
                    signal.severity, signal.kind_str(), signal.description
                );
                result.push(signal);
            } else {
                debug!(
                    "Anomaly suppressed by dedup: [{}] {}",
                    signal.severity, signal.kind_str()
                );
            }
        }

        // Periodic cleanup of expired entries
        dedup.purge_expired();

        result
    }
}

impl AnomalyDetector for RuleEngine {
    fn analyze(&self, batch: &EventBatch) -> Vec<AnomalySignal> {
        self.evaluate_and_dedup(batch)
    }
}

/// Extension trait to add kind_str() to AnomalySignal.
impl AnomalySignal {
    /// Human-readable name for the anomaly kind.
    pub fn kind_str(&self) -> &str {
        use crate::types::AnomalyKind;
        match &self.kind {
            AnomalyKind::BulkDelete => "BulkDelete",
            AnomalyKind::LargeDeletion => "LargeDeletion",
            AnomalyKind::BulkModify => "BulkModify",
            AnomalyKind::RootDirDeleted => "RootDirDeleted",
            AnomalyKind::SensitiveConfigModified => "SensitiveConfigModified",
            AnomalyKind::LargeNonPackageModify => "LargeNonPackageModify",
        }
    }

    /// The primary affected directory (first file's parent).
    pub fn primary_directory(&self) -> Option<PathBuf> {
        self.affected_files
            .first()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AnomalyKind, FileEvent, FileEventKind};
    use chrono::Utc;
    use std::time::Duration;

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

    fn default_engine() -> RuleEngine {
        let config = AnomalyRulesConfig::default();
        let watch_dirs = vec![PathBuf::from("/home/user/Documents")];
        RuleEngine::new(config, watch_dirs)
    }

    #[test]
    fn test_no_anomaly_on_normal_batch() {
        let engine = default_engine();
        let events = vec![
            make_event("/home/user/Documents/readme.md", FileEventKind::Modified, Some(1000)),
            make_event("/home/user/Documents/main.rs", FileEventKind::Modified, Some(2000)),
        ];
        let batch = make_batch(events);
        let signals = engine.analyze(&batch);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_bulk_delete_25_files_high() {
        let engine = default_engine();
        let events: Vec<FileEvent> = (0..25)
            .map(|i| {
                make_event(
                    &format!("/home/user/Documents/file_{}.txt", i),
                    FileEventKind::Deleted,
                    Some(100),
                )
            })
            .collect();
        let batch = make_batch(events);
        let signals = engine.analyze(&batch);

        // Should have at least one HIGH signal
        assert!(!signals.is_empty());
        let high_signals: Vec<_> = signals
            .iter()
            .filter(|s| s.severity == AnomalySeverity::High)
            .collect();
        assert!(!high_signals.is_empty());

        // Check that the signal contains file count and directory
        let s = &high_signals[0];
        assert!(s.description.contains("25"));
        assert!(s.description.contains("/home/user/Documents"));
    }

    #[test]
    fn test_root_dir_deleted_critical() {
        let engine = default_engine();
        let events = vec![make_event(
            "/home/user/Documents",
            FileEventKind::Deleted,
            None,
        )];
        let batch = make_batch(events);
        let signals = engine.analyze(&batch);

        assert!(!signals.is_empty());
        // CRITICAL should be first (highest priority)
        assert_eq!(signals[0].severity, AnomalySeverity::Critical);
        assert_eq!(signals[0].kind, AnomalyKind::RootDirDeleted);
    }

    #[test]
    fn test_sensitive_file_modified() {
        let engine = default_engine();
        let events = vec![make_event(
            "/home/user/Documents/.env",
            FileEventKind::Modified,
            Some(256),
        )];
        let batch = make_batch(events);
        let signals = engine.analyze(&batch);

        assert!(!signals.is_empty());
        let env_signal = signals
            .iter()
            .find(|s| s.kind == AnomalyKind::SensitiveConfigModified);
        assert!(env_signal.is_some());
        let s = env_signal.unwrap();
        assert_eq!(s.severity, AnomalySeverity::High);
        assert!(s.description.contains(".env"));
    }

    #[test]
    fn test_dedup_suppresses_repeat() {
        // Short cooldown for testing
        let config = AnomalyRulesConfig::default();
        let watch_dirs = vec![PathBuf::from("/home/user/Documents")];
        let engine = RuleEngine::with_dedup_cooldown(
            config,
            watch_dirs,
            Duration::from_secs(300),
        );

        let events: Vec<FileEvent> = (0..25)
            .map(|i| {
                make_event(
                    &format!("/home/user/Documents/file_{}.txt", i),
                    FileEventKind::Deleted,
                    Some(100),
                )
            })
            .collect();

        // First batch: should fire
        let batch1 = make_batch(events.clone());
        let signals1 = engine.analyze(&batch1);
        assert!(!signals1.is_empty());

        // Second identical batch: should be suppressed by dedup
        let batch2 = make_batch(events);
        let signals2 = engine.analyze(&batch2);
        // All signals from the same dir+rule should be suppressed
        let bulk_delete_signals: Vec<_> = signals2
            .iter()
            .filter(|s| s.kind == AnomalyKind::BulkDelete)
            .collect();
        assert!(
            bulk_delete_signals.is_empty(),
            "Duplicate BulkDelete should be suppressed, got {} signals",
            bulk_delete_signals.len()
        );
    }

    #[test]
    fn test_dedup_expires_after_cooldown() {
        let config = AnomalyRulesConfig::default();
        let watch_dirs = vec![PathBuf::from("/home/user/Documents")];
        let engine = RuleEngine::with_dedup_cooldown(
            config,
            watch_dirs,
            Duration::from_millis(100), // very short cooldown for test
        );

        let events: Vec<FileEvent> = (0..25)
            .map(|i| {
                make_event(
                    &format!("/home/user/Documents/file_{}.txt", i),
                    FileEventKind::Deleted,
                    Some(100),
                )
            })
            .collect();

        let batch = make_batch(events.clone());
        let signals1 = engine.analyze(&batch);
        assert!(!signals1.is_empty());

        // Wait for cooldown
        std::thread::sleep(Duration::from_millis(150));

        // Should fire again after cooldown
        let batch2 = make_batch(events);
        let signals2 = engine.analyze(&batch2);
        assert!(!signals2.is_empty());
    }

    #[test]
    fn test_priority_merge_critical_first() {
        let config = AnomalyRulesConfig::default();
        let watch_dirs = vec![PathBuf::from("/home/user/Documents")];
        let engine = RuleEngine::new(config, watch_dirs);

        // Mix: root dir deleted (CRITICAL) + bulk delete (HIGH)
        let mut events: Vec<FileEvent> = (0..25)
            .map(|i| {
                make_event(
                    &format!("/home/user/Documents/file_{}.txt", i),
                    FileEventKind::Deleted,
                    Some(100),
                )
            })
            .collect();
        events.push(make_event(
            "/home/user/Documents",
            FileEventKind::Deleted,
            None,
        ));

        let batch = make_batch(events);
        let signals = engine.analyze(&batch);
        assert!(!signals.is_empty());
        // CRITICAL should come first
        assert_eq!(signals[0].severity, AnomalySeverity::Critical);
    }

    #[test]
    fn test_node_modules_events_no_alert() {
        let engine = default_engine();
        // Even many deletions inside node_modules should not fire alerts
        // (these events should be filtered by PathFilter BEFORE reaching the detector,
        //  but even if they somehow arrive, RULE-07 explicitly excludes package paths)
        let events: Vec<FileEvent> = (0..50)
            .map(|i| {
                make_event(
                    &format!("/home/user/Documents/proj/node_modules/pkg/file_{}.js", i),
                    FileEventKind::Modified,
                    Some(100),
                )
            })
            .collect();
        let batch = make_batch(events);
        let signals = engine.analyze(&batch);
        // BulkModify may fire (50 > threshold), but LargeNonPackageModify should not
        let npm_signals: Vec<_> = signals
            .iter()
            .filter(|s| s.kind == AnomalyKind::LargeNonPackageModify)
            .collect();
        assert!(npm_signals.is_empty(), "RULE-07 should exclude node_modules");
    }

    #[test]
    fn test_kind_str() {
        use crate::types::AnomalyKind;
        let signal = AnomalySignal {
            kind: AnomalyKind::BulkDelete,
            severity: AnomalySeverity::High,
            affected_files: vec![],
            detected_at: Utc::now(),
            description: String::new(),
        };
        assert_eq!(signal.kind_str(), "BulkDelete");
    }

    #[test]
    fn test_primary_directory() {
        let signal = AnomalySignal {
            kind: AnomalyKind::BulkDelete,
            severity: AnomalySeverity::High,
            affected_files: vec![PathBuf::from("/home/user/Documents/file.txt")],
            detected_at: Utc::now(),
            description: String::new(),
        };
        assert_eq!(
            signal.primary_directory(),
            Some(PathBuf::from("/home/user/Documents"))
        );
    }
}
