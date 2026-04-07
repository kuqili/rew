//! Notification system for anomaly alerts.
//!
//! Supports:
//! - Immediate notification for CRITICAL and HIGH severity
//! - Aggregated notification for MEDIUM severity (2-minute collection window)
//! - macOS native notifications via osascript

pub mod macos;

use crate::types::{AnomalySeverity, AnomalySignal};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Configuration for the notification aggregator.
#[derive(Debug, Clone)]
pub struct NotifierConfig {
    /// How long to collect MEDIUM alerts before sending aggregated notification
    pub medium_aggregation_window: Duration,
}

impl Default for NotifierConfig {
    fn default() -> Self {
        Self {
            medium_aggregation_window: Duration::from_secs(120), // 2 minutes
        }
    }
}

/// Aggregates MEDIUM-severity alerts and flushes them periodically.
pub struct MediumAggregator {
    /// Collected MEDIUM alerts pending notification
    pending: Vec<AnomalySignal>,
    /// When the aggregation window started (first MEDIUM alert received)
    window_start: Option<Instant>,
    /// Aggregation window duration
    window_duration: Duration,
}

impl MediumAggregator {
    pub fn new(window_duration: Duration) -> Self {
        Self {
            pending: Vec::new(),
            window_start: None,
            window_duration,
        }
    }

    /// Add a MEDIUM alert to the aggregation buffer.
    pub fn add(&mut self, signal: AnomalySignal) {
        if self.window_start.is_none() {
            self.window_start = Some(Instant::now());
        }
        self.pending.push(signal);
    }

    /// Check if the aggregation window has elapsed and return pending alerts if so.
    pub fn flush_if_ready(&mut self) -> Option<Vec<AnomalySignal>> {
        if let Some(start) = self.window_start {
            if Instant::now().duration_since(start) >= self.window_duration {
                return self.force_flush();
            }
        }
        None
    }

    /// Force flush all pending alerts regardless of window.
    pub fn force_flush(&mut self) -> Option<Vec<AnomalySignal>> {
        if self.pending.is_empty() {
            return None;
        }
        let alerts = std::mem::take(&mut self.pending);
        self.window_start = None;
        Some(alerts)
    }

    /// Number of pending MEDIUM alerts.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Check if there are pending alerts.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// The notification dispatcher routes alerts to the platform notifier.
///
/// CRITICAL and HIGH -> immediate notification
/// MEDIUM -> aggregated, sent after 2-minute window
pub struct NotificationDispatcher<N: PlatformNotifier> {
    notifier: N,
    aggregator: Mutex<MediumAggregator>,
}

/// Platform-specific notification interface.
pub trait PlatformNotifier: Send + Sync {
    /// Send a single anomaly notification.
    fn send_anomaly(&self, signal: &AnomalySignal) -> crate::error::RewResult<()>;

    /// Send an aggregated summary notification for multiple MEDIUM alerts.
    fn send_aggregated(&self, signals: &[AnomalySignal]) -> crate::error::RewResult<()>;

    /// Send a general notification with title and body.
    fn send_general(&self, title: &str, body: &str) -> crate::error::RewResult<()>;
}

impl<N: PlatformNotifier> NotificationDispatcher<N> {
    /// Create a new dispatcher with the given platform notifier.
    pub fn new(notifier: N) -> Self {
        Self {
            notifier,
            aggregator: Mutex::new(MediumAggregator::new(
                NotifierConfig::default().medium_aggregation_window,
            )),
        }
    }

    /// Create a dispatcher with custom aggregation window.
    pub fn with_config(notifier: N, config: NotifierConfig) -> Self {
        Self {
            notifier,
            aggregator: Mutex::new(MediumAggregator::new(config.medium_aggregation_window)),
        }
    }

    /// Dispatch an anomaly signal.
    ///
    /// CRITICAL/HIGH: sent immediately
    /// MEDIUM: queued for aggregation
    pub fn dispatch(&self, signal: &AnomalySignal) -> crate::error::RewResult<()> {
        match signal.severity {
            AnomalySeverity::Critical | AnomalySeverity::High => {
                info!(
                    "Sending immediate notification: [{}] {}",
                    signal.severity,
                    signal.kind_str()
                );
                self.notifier.send_anomaly(signal)?;
            }
            AnomalySeverity::Medium => {
                debug!("Queuing MEDIUM alert for aggregation: {}", signal.kind_str());
                let mut agg = self.aggregator.lock().unwrap();
                agg.add(signal.clone());
            }
        }
        Ok(())
    }

    /// Check and flush aggregated MEDIUM alerts if the window has elapsed.
    /// Should be called periodically (e.g. every 30 seconds).
    pub fn flush_aggregated(&self) -> crate::error::RewResult<()> {
        let signals = {
            let mut agg = self.aggregator.lock().unwrap();
            agg.flush_if_ready()
        };

        if let Some(signals) = signals {
            info!(
                "Sending aggregated notification for {} MEDIUM alerts",
                signals.len()
            );
            self.notifier.send_aggregated(&signals)?;
        }
        Ok(())
    }

    /// Force flush all pending MEDIUM alerts (e.g. on shutdown).
    pub fn force_flush(&self) -> crate::error::RewResult<()> {
        let signals = {
            let mut agg = self.aggregator.lock().unwrap();
            agg.force_flush()
        };

        if let Some(signals) = signals {
            info!(
                "Force-flushing {} MEDIUM alerts",
                signals.len()
            );
            self.notifier.send_aggregated(&signals)?;
        }
        Ok(())
    }

    /// Number of pending aggregated alerts.
    pub fn pending_count(&self) -> usize {
        self.aggregator.lock().unwrap().pending_count()
    }
}

/// Implement the Notifier trait for NotificationDispatcher.
impl<N: PlatformNotifier> crate::traits::Notifier for NotificationDispatcher<N> {
    fn notify_anomaly(&self, signal: &AnomalySignal) -> crate::error::RewResult<()> {
        self.dispatch(signal)
    }

    fn notify(&self, title: &str, body: &str) -> crate::error::RewResult<()> {
        self.notifier.send_general(title, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AnomalyKind, AnomalySeverity, AnomalySignal};
    use chrono::Utc;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Mock notifier for testing
    struct MockNotifier {
        immediate_count: Arc<AtomicUsize>,
        aggregated_count: Arc<AtomicUsize>,
        general_count: Arc<AtomicUsize>,
    }

    impl MockNotifier {
        fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
            let imm = Arc::new(AtomicUsize::new(0));
            let agg = Arc::new(AtomicUsize::new(0));
            let gen = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    immediate_count: imm.clone(),
                    aggregated_count: agg.clone(),
                    general_count: gen.clone(),
                },
                imm,
                agg,
                gen,
            )
        }
    }

    impl PlatformNotifier for MockNotifier {
        fn send_anomaly(&self, _signal: &AnomalySignal) -> crate::error::RewResult<()> {
            self.immediate_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn send_aggregated(&self, _signals: &[AnomalySignal]) -> crate::error::RewResult<()> {
            self.aggregated_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn send_general(&self, _title: &str, _body: &str) -> crate::error::RewResult<()> {
            self.general_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn make_signal(severity: AnomalySeverity, kind: AnomalyKind) -> AnomalySignal {
        AnomalySignal {
            kind,
            severity,
            affected_files: vec![PathBuf::from("/test/file.txt")],
            detected_at: Utc::now(),
            description: "test".to_string(),
        }
    }

    #[test]
    fn test_critical_sent_immediately() {
        let (mock, imm, _agg, _gen) = MockNotifier::new();
        let dispatcher = NotificationDispatcher::new(mock);

        let signal = make_signal(AnomalySeverity::Critical, AnomalyKind::RootDirDeleted);
        dispatcher.dispatch(&signal).unwrap();

        assert_eq!(imm.load(Ordering::SeqCst), 1);
        assert_eq!(dispatcher.pending_count(), 0);
    }

    #[test]
    fn test_high_sent_immediately() {
        let (mock, imm, _agg, _gen) = MockNotifier::new();
        let dispatcher = NotificationDispatcher::new(mock);

        let signal = make_signal(AnomalySeverity::High, AnomalyKind::BulkDelete);
        dispatcher.dispatch(&signal).unwrap();

        assert_eq!(imm.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_medium_queued_for_aggregation() {
        let (mock, imm, _agg, _gen) = MockNotifier::new();
        let dispatcher = NotificationDispatcher::new(mock);

        let signal = make_signal(AnomalySeverity::Medium, AnomalyKind::BulkModify);
        dispatcher.dispatch(&signal).unwrap();

        assert_eq!(imm.load(Ordering::SeqCst), 0); // NOT sent immediately
        assert_eq!(dispatcher.pending_count(), 1);
    }

    #[test]
    fn test_medium_aggregation_flush() {
        let (mock, _imm, agg, _gen) = MockNotifier::new();
        let config = NotifierConfig {
            medium_aggregation_window: Duration::from_millis(50),
        };
        let dispatcher = NotificationDispatcher::with_config(mock, config);

        // Queue 3 MEDIUM alerts
        for _ in 0..3 {
            let signal = make_signal(AnomalySeverity::Medium, AnomalyKind::BulkModify);
            dispatcher.dispatch(&signal).unwrap();
        }
        assert_eq!(dispatcher.pending_count(), 3);

        // Wait for window to elapse
        std::thread::sleep(Duration::from_millis(100));

        // Flush should send aggregated notification
        dispatcher.flush_aggregated().unwrap();
        assert_eq!(agg.load(Ordering::SeqCst), 1); // One aggregated call
        assert_eq!(dispatcher.pending_count(), 0);
    }

    #[test]
    fn test_force_flush() {
        let (mock, _imm, agg, _gen) = MockNotifier::new();
        let dispatcher = NotificationDispatcher::new(mock); // default 2-minute window

        let signal = make_signal(AnomalySeverity::Medium, AnomalyKind::BulkModify);
        dispatcher.dispatch(&signal).unwrap();

        // Force flush even though window hasn't elapsed
        dispatcher.force_flush().unwrap();
        assert_eq!(agg.load(Ordering::SeqCst), 1);
        assert_eq!(dispatcher.pending_count(), 0);
    }

    #[test]
    fn test_flush_no_pending_is_noop() {
        let (mock, _imm, agg, _gen) = MockNotifier::new();
        let dispatcher = NotificationDispatcher::new(mock);

        dispatcher.force_flush().unwrap();
        assert_eq!(agg.load(Ordering::SeqCst), 0); // No call made
    }

    #[test]
    fn test_mixed_severities() {
        let (mock, imm, _agg, _gen) = MockNotifier::new();
        let dispatcher = NotificationDispatcher::new(mock);

        dispatcher
            .dispatch(&make_signal(
                AnomalySeverity::Critical,
                AnomalyKind::RootDirDeleted,
            ))
            .unwrap();
        dispatcher
            .dispatch(&make_signal(AnomalySeverity::High, AnomalyKind::BulkDelete))
            .unwrap();
        dispatcher
            .dispatch(&make_signal(
                AnomalySeverity::Medium,
                AnomalyKind::BulkModify,
            ))
            .unwrap();

        assert_eq!(imm.load(Ordering::SeqCst), 2); // CRITICAL + HIGH
        assert_eq!(dispatcher.pending_count(), 1); // MEDIUM queued
    }

    #[test]
    fn test_general_notification() {
        let (mock, _imm, _agg, gen) = MockNotifier::new();
        let dispatcher = NotificationDispatcher::new(mock);
        use crate::traits::Notifier;
        dispatcher.notify("Test", "Body").unwrap();
        assert_eq!(gen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_aggregator_window_not_ready() {
        let mut agg = MediumAggregator::new(Duration::from_secs(120));
        let signal = make_signal(AnomalySeverity::Medium, AnomalyKind::BulkModify);
        agg.add(signal);
        assert!(agg.flush_if_ready().is_none()); // Not enough time passed
        assert_eq!(agg.pending_count(), 1);
    }

    #[test]
    fn test_aggregator_window_ready() {
        let mut agg = MediumAggregator::new(Duration::from_millis(50));
        let signal = make_signal(AnomalySeverity::Medium, AnomalyKind::BulkModify);
        agg.add(signal);

        std::thread::sleep(Duration::from_millis(100));
        let result = agg.flush_if_ready();
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 1);
        assert_eq!(agg.pending_count(), 0);
    }
}
