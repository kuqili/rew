//! Alert deduplication for the anomaly detector.
//!
//! Suppresses duplicate alerts for the same (directory, rule) combination
//! within a configurable time window (default: 5 minutes).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::types::AnomalySignal;

/// Key for deduplication: directory path + rule kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DedupKey {
    /// The primary directory affected (first parent of affected files)
    directory: PathBuf,
    /// The rule kind as a string identifier
    rule_id: String,
}

/// Deduplicates anomaly alerts to prevent notification spam.
///
/// For the same directory + same rule, only one alert is allowed within
/// the configured cooldown period.
pub struct AlertDeduplicator {
    /// Cooldown duration (default: 5 minutes)
    cooldown: Duration,
    /// Map of (directory, rule) -> last alert time
    last_alerts: HashMap<DedupKey, Instant>,
}

impl AlertDeduplicator {
    /// Create a new deduplicator with default 5-minute cooldown.
    pub fn new() -> Self {
        Self {
            cooldown: Duration::from_secs(300), // 5 minutes
            last_alerts: HashMap::new(),
        }
    }

    /// Create a deduplicator with a custom cooldown duration.
    pub fn with_cooldown(cooldown: Duration) -> Self {
        Self {
            cooldown,
            last_alerts: HashMap::new(),
        }
    }

    /// Check if an alert should be sent (not suppressed by dedup).
    ///
    /// Returns `true` if the alert should proceed, `false` if it's a duplicate.
    /// If the alert should proceed, records this alert time for future dedup.
    pub fn should_alert(&mut self, signal: &AnomalySignal) -> bool {
        let directory = signal
            .affected_files
            .first()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        let key = DedupKey {
            directory,
            rule_id: format!("{:?}", signal.kind),
        };

        let now = Instant::now();

        if let Some(last) = self.last_alerts.get(&key) {
            if now.duration_since(*last) < self.cooldown {
                return false; // Suppressed — too recent
            }
        }

        // Record this alert
        self.last_alerts.insert(key, now);
        true
    }

    /// Purge expired entries to prevent unbounded growth.
    /// Should be called periodically (e.g. every cleanup cycle).
    pub fn purge_expired(&mut self) {
        let now = Instant::now();
        self.last_alerts
            .retain(|_, last| now.duration_since(*last) < self.cooldown);
    }

    /// Number of active dedup entries (for testing/monitoring).
    pub fn active_entries(&self) -> usize {
        self.last_alerts.len()
    }
}

impl Default for AlertDeduplicator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AnomalyKind, AnomalySeverity, AnomalySignal};
    use chrono::Utc;
    use std::thread;

    fn make_signal(dir: &str, kind: AnomalyKind) -> AnomalySignal {
        AnomalySignal {
            kind,
            severity: AnomalySeverity::High,
            affected_files: vec![PathBuf::from(format!("{}/file.txt", dir))],
            detected_at: Utc::now(),
            description: "test signal".to_string(),
        }
    }

    #[test]
    fn test_first_alert_passes() {
        let mut dedup = AlertDeduplicator::new();
        let signal = make_signal("/home/user/Documents", AnomalyKind::BulkDelete);
        assert!(dedup.should_alert(&signal));
    }

    #[test]
    fn test_duplicate_suppressed() {
        let mut dedup = AlertDeduplicator::new();
        let signal = make_signal("/home/user/Documents", AnomalyKind::BulkDelete);
        assert!(dedup.should_alert(&signal)); // first: pass
        assert!(!dedup.should_alert(&signal)); // second: suppressed
        assert!(!dedup.should_alert(&signal)); // third: still suppressed
    }

    #[test]
    fn test_different_directory_not_suppressed() {
        let mut dedup = AlertDeduplicator::new();
        let signal1 = make_signal("/home/user/Documents", AnomalyKind::BulkDelete);
        let signal2 = make_signal("/home/user/Downloads", AnomalyKind::BulkDelete);
        assert!(dedup.should_alert(&signal1));
        assert!(dedup.should_alert(&signal2)); // different dir -> passes
    }

    #[test]
    fn test_different_rule_not_suppressed() {
        let mut dedup = AlertDeduplicator::new();
        let signal1 = make_signal("/home/user/Documents", AnomalyKind::BulkDelete);
        let signal2 = make_signal("/home/user/Documents", AnomalyKind::SensitiveConfigModified);
        assert!(dedup.should_alert(&signal1));
        assert!(dedup.should_alert(&signal2)); // different rule -> passes
    }

    #[test]
    fn test_cooldown_expiry() {
        let mut dedup = AlertDeduplicator::with_cooldown(Duration::from_millis(100));
        let signal = make_signal("/home/user/Documents", AnomalyKind::BulkDelete);
        assert!(dedup.should_alert(&signal));
        assert!(!dedup.should_alert(&signal)); // suppressed

        // Wait for cooldown to expire
        thread::sleep(Duration::from_millis(150));
        assert!(dedup.should_alert(&signal)); // passes after cooldown
    }

    #[test]
    fn test_purge_expired() {
        let mut dedup = AlertDeduplicator::with_cooldown(Duration::from_millis(50));
        let signal1 = make_signal("/dir1", AnomalyKind::BulkDelete);
        let signal2 = make_signal("/dir2", AnomalyKind::BulkDelete);
        dedup.should_alert(&signal1);
        dedup.should_alert(&signal2);
        assert_eq!(dedup.active_entries(), 2);

        thread::sleep(Duration::from_millis(100));
        dedup.purge_expired();
        assert_eq!(dedup.active_entries(), 0);
    }

    #[test]
    fn test_active_entries_count() {
        let mut dedup = AlertDeduplicator::new();
        assert_eq!(dedup.active_entries(), 0);

        dedup.should_alert(&make_signal("/dir1", AnomalyKind::BulkDelete));
        assert_eq!(dedup.active_entries(), 1);

        dedup.should_alert(&make_signal("/dir2", AnomalyKind::LargeDeletion));
        assert_eq!(dedup.active_entries(), 2);

        // Same key shouldn't increase count
        dedup.should_alert(&make_signal("/dir1", AnomalyKind::BulkDelete));
        assert_eq!(dedup.active_entries(), 2);
    }
}
