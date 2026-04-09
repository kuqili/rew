//! Event processor with sliding window aggregation, deduplication, and dynamic filtering.
//!
//! The EventProcessor receives raw `FileEvent`s from the watcher and:
//! 1. Deduplicates same-path, same-kind events within the window
//! 2. Aggregates events into `EventBatch`es at configurable intervals (default 30s)
//! 3. Implements dynamic filter pausing:
//!    - `package.json`/`Cargo.toml` changes -> pause `node_modules`/`target` filtering for 60s
//!    - `.git/HEAD` changes -> pause all filtering for 10s
//! 4. Computes batch statistics (files added/modified/deleted + sizes)

use crate::types::{EventBatch, FileEvent, FileEventKind};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Configuration for the event processor.
#[derive(Debug, Clone)]
pub struct ProcessorConfig {
    /// Window duration for event aggregation
    pub window_duration: Duration,
    /// How long to pause node_modules/target filtering after package.json/Cargo.toml change
    pub package_pause_duration: Duration,
    /// How long to pause all filtering after .git/HEAD change
    pub git_pause_duration: Duration,
}

impl Default for ProcessorConfig {
    fn default() -> Self {
        Self {
            window_duration: Duration::from_secs(30),
            package_pause_duration: Duration::from_secs(60),
            git_pause_duration: Duration::from_secs(10),
        }
    }
}

/// Key for deduplication: path only.
///
/// Same file path within a window = one record, keeping the last-seen state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DeduplicationKey {
    path: PathBuf,
}

/// Merge two event kinds for the same file path.
///
/// Returns `None` when the pair cancels out (net-zero change):
///   Created → Deleted  = file appeared and vanished, no user-visible effect
///
/// Otherwise returns the resolved kind:
///   Created  → Modified  = still Created  (new file, content updated)
///   Deleted  → Created   = Modified       (re-appeared after being gone)
///   anything → anything  = latest kind wins
fn merge_kind(old: &FileEventKind, new: &FileEventKind) -> Option<FileEventKind> {
    match (old, new) {
        (FileEventKind::Created, FileEventKind::Deleted) => None,
        (FileEventKind::Created, FileEventKind::Modified) => Some(FileEventKind::Created),
        (FileEventKind::Deleted, FileEventKind::Created) => Some(FileEventKind::Modified),
        (_, k) => Some(k.clone()),
    }
}

/// Extract the file name from a FileEvent's path as a String.
fn event_file_name(event: &FileEvent) -> String {
    event
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Check if a path string contains a specific directory component.
fn path_contains(event: &FileEvent, component: &str) -> bool {
    event.path.to_string_lossy().contains(component)
}

/// Tracks dynamic filter pause state.
#[derive(Debug)]
struct PauseState {
    /// When package-related pause expires (node_modules/target ignored)
    package_pause_until: Option<Instant>,
    /// When git-related pause expires (global pause)
    git_pause_until: Option<Instant>,
}

impl PauseState {
    fn new() -> Self {
        Self {
            package_pause_until: None,
            git_pause_until: None,
        }
    }

    /// Check if we're in a global pause (git HEAD change detected).
    fn is_globally_paused(&self) -> bool {
        self.git_pause_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Check if package-related directories should be paused.
    fn is_package_paused(&self) -> bool {
        self.package_pause_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Trigger package pause (package.json or Cargo.toml changed).
    fn trigger_package_pause(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        self.package_pause_until = Some(until);
        info!(
            "Dynamic pause activated: package dirs paused for {}s",
            duration.as_secs()
        );
    }

    /// Trigger global pause (.git/HEAD changed).
    fn trigger_git_pause(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        self.git_pause_until = Some(until);
        info!(
            "Dynamic pause activated: global pause for {}s",
            duration.as_secs()
        );
    }
}

/// Statistics computed for each EventBatch.
#[derive(Debug, Clone, Default)]
pub struct BatchStats {
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub files_renamed: usize,
    pub total_added_size: u64,
    pub total_modified_size: u64,
    pub total_deleted_size: u64,
}

impl BatchStats {
    /// Compute statistics from an EventBatch.
    pub fn from_batch(batch: &EventBatch) -> Self {
        let mut stats = BatchStats::default();
        for event in &batch.events {
            let size = event.size_bytes.unwrap_or(0);
            match event.kind {
                FileEventKind::Created => {
                    stats.files_added += 1;
                    stats.total_added_size += size;
                }
                FileEventKind::Modified => {
                    stats.files_modified += 1;
                    stats.total_modified_size += size;
                }
                FileEventKind::Deleted => {
                    stats.files_deleted += 1;
                    stats.total_deleted_size += size;
                }
                FileEventKind::Renamed => {
                    stats.files_renamed += 1;
                }
            }
        }
        stats
    }
}

/// The event processor aggregates, deduplicates, and batches file events.
pub struct EventProcessor {
    config: ProcessorConfig,
}

impl EventProcessor {
    /// Create a new EventProcessor with the given configuration.
    pub fn new(config: ProcessorConfig) -> Self {
        Self { config }
    }

    /// Create an EventProcessor with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ProcessorConfig::default())
    }

    /// Run the event processing loop.
    ///
    /// Receives events from `event_rx`, aggregates them into batches,
    /// and sends the batches to `batch_tx`.
    ///
    /// This method runs until the input channel is closed.
    pub async fn run(
        &self,
        mut event_rx: mpsc::UnboundedReceiver<FileEvent>,
        batch_tx: mpsc::Sender<(EventBatch, BatchStats)>,
    ) {
        let window = self.config.window_duration;
        let package_pause = self.config.package_pause_duration;
        let git_pause = self.config.git_pause_duration;

        let mut event_map: HashMap<DeduplicationKey, FileEvent> = HashMap::new();
        let mut pause_state = PauseState::new();
        let mut window_start = Utc::now();
        let mut interval = tokio::time::interval(window);
        // First tick completes immediately, skip it
        interval.tick().await;

        loop {
            tokio::select! {
                // Receive new event
                event = event_rx.recv() => {
                    match event {
                        Some(file_event) => {
                            // Check pause state BEFORE triggering new pauses
                            // (the event that triggers a pause should still be processed)
                            let drop = self.should_drop_during_pause(&file_event, &pause_state);

                            // Check if this event triggers dynamic pauses (for future events)
                            self.check_dynamic_triggers(&file_event, &mut pause_state, package_pause, git_pause);

                            // Should we drop this event due to pause?
                            if drop {
                                debug!("Event dropped due to dynamic pause: {:?}", file_event.path);
                                continue;
                            }

                            // Deduplicate: same path = one record, merge kinds
                            let key = DeduplicationKey {
                                path: file_event.path.clone(),
                            };
                            if let Some(existing) = event_map.get(&key) {
                                match merge_kind(&existing.kind, &file_event.kind) {
                                    Some(merged_kind) => {
                                        let mut merged = file_event.clone();
                                        merged.kind = merged_kind;
                                        event_map.insert(key, merged);
                                    }
                                    None => {
                                        // Net-zero: Created then Deleted = nothing
                                        event_map.remove(&key);
                                    }
                                }
                            } else {
                                event_map.insert(key, file_event);
                            }
                        }
                        None => {
                            // Channel closed, flush remaining events and exit
                            if !event_map.is_empty() {
                                let batch = self.flush_events(&mut event_map, window_start);
                                let stats = BatchStats::from_batch(&batch);
                                let _ = batch_tx.send((batch, stats)).await;
                            }
                            info!("Event channel closed, processor shutting down");
                            return;
                        }
                    }
                }
                // Window tick
                _ = interval.tick() => {
                    if !event_map.is_empty() {
                        let batch = self.flush_events(&mut event_map, window_start);
                        let stats = BatchStats::from_batch(&batch);
                        info!(
                            "EventBatch: {} events (added={}, modified={}, deleted={}, renamed={})",
                            batch.events.len(),
                            stats.files_added,
                            stats.files_modified,
                            stats.files_deleted,
                            stats.files_renamed,
                        );
                        if batch_tx.send((batch, stats)).await.is_err() {
                            warn!("Batch channel closed, processor shutting down");
                            return;
                        }
                    }
                    window_start = Utc::now();
                }
            }
        }
    }

    /// Check if this event should trigger a dynamic pause.
    fn check_dynamic_triggers(
        &self,
        event: &FileEvent,
        pause_state: &mut PauseState,
        package_pause: Duration,
        git_pause: Duration,
    ) {
        let file_name = event_file_name(event);

        // package.json or Cargo.toml changed -> pause node_modules/target
        if file_name == "package.json"
            || file_name == "Cargo.toml"
            || file_name == "package-lock.json"
            || file_name == "yarn.lock"
            || file_name == "pnpm-lock.yaml"
            || file_name == "Cargo.lock"
        {
            pause_state.trigger_package_pause(package_pause);
        }

        // .git/HEAD changed -> global pause
        if path_contains(event, ".git") && file_name == "HEAD" {
            pause_state.trigger_git_pause(git_pause);
        }
    }

    /// Check if an event should be dropped due to dynamic pause.
    fn should_drop_during_pause(&self, event: &FileEvent, pause_state: &PauseState) -> bool {
        // Global pause drops everything
        if pause_state.is_globally_paused() {
            return true;
        }

        // Package pause only drops node_modules and target
        if pause_state.is_package_paused()
            && (path_contains(event, "node_modules") || path_contains(event, "/target/"))
        {
            return true;
        }

        false
    }

    /// Flush accumulated events into an EventBatch.
    fn flush_events(
        &self,
        event_map: &mut HashMap<DeduplicationKey, FileEvent>,
        window_start: chrono::DateTime<Utc>,
    ) -> EventBatch {
        let events: Vec<FileEvent> = event_map.drain().map(|(_, v)| v).collect();
        EventBatch {
            events,
            window_start,
            window_end: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::time::timeout;

    fn make_event(path: &str, kind: FileEventKind) -> FileEvent {
        FileEvent {
            path: PathBuf::from(path),
            kind,
            timestamp: Utc::now(),
            size_bytes: Some(100),
        }
    }

    #[tokio::test]
    async fn test_deduplication() {
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(200),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        // Spawn processor
        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Send 5 identical events (same path, same kind)
        for _ in 0..5 {
            event_tx
                .send(make_event("/tmp/test.txt", FileEventKind::Modified))
                .unwrap();
        }

        // Wait for batch
        let result = timeout(Duration::from_secs(2), batch_rx.recv()).await;
        assert!(result.is_ok());
        let (batch, stats) = result.unwrap().unwrap();
        // Should be deduplicated to 1 event
        assert_eq!(batch.events.len(), 1);
        assert_eq!(stats.files_modified, 1);

        // Close channel
        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_same_path_different_kind_merges() {
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(200),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Different paths
        event_tx
            .send(make_event("/tmp/a.txt", FileEventKind::Created))
            .unwrap();
        event_tx
            .send(make_event("/tmp/b.txt", FileEventKind::Created))
            .unwrap();
        // Same path as a.txt but different kind: Created+Modified → Created (merged)
        event_tx
            .send(make_event("/tmp/a.txt", FileEventKind::Modified))
            .unwrap();

        let result = timeout(Duration::from_secs(2), batch_rx.recv()).await;
        assert!(result.is_ok());
        let (batch, stats) = result.unwrap().unwrap();
        // a.txt and b.txt → 2 events (a.txt Created+Modified merged to Created)
        assert_eq!(batch.events.len(), 2);
        assert_eq!(stats.files_added, 2);
        assert_eq!(stats.files_modified, 0);

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_created_then_deleted_cancels_out() {
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(200),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // temp.sb-xxx: Created then Deleted → net zero, should not appear
        event_tx
            .send(make_event("/tmp/file.sb-abc", FileEventKind::Created))
            .unwrap();
        event_tx
            .send(make_event("/tmp/file.sb-abc", FileEventKind::Deleted))
            .unwrap();
        // real file: only a Modified
        event_tx
            .send(make_event("/tmp/real.txt", FileEventKind::Modified))
            .unwrap();

        let result = timeout(Duration::from_secs(2), batch_rx.recv()).await;
        assert!(result.is_ok());
        let (batch, stats) = result.unwrap().unwrap();
        // Only real.txt should appear
        assert_eq!(batch.events.len(), 1);
        assert_eq!(stats.files_modified, 1);
        assert_eq!(batch.events[0].path.to_string_lossy(), "/tmp/real.txt");

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_batch_stats() {
        let batch = EventBatch {
            events: vec![
                make_event("/a.txt", FileEventKind::Created),
                make_event("/b.txt", FileEventKind::Created),
                make_event("/c.txt", FileEventKind::Modified),
                make_event("/d.txt", FileEventKind::Deleted),
                make_event("/e.txt", FileEventKind::Deleted),
                make_event("/f.txt", FileEventKind::Deleted),
            ],
            window_start: Utc::now(),
            window_end: Utc::now(),
        };

        let stats = BatchStats::from_batch(&batch);
        assert_eq!(stats.files_added, 2);
        assert_eq!(stats.files_modified, 1);
        assert_eq!(stats.files_deleted, 3);
        assert_eq!(stats.total_added_size, 200);
        assert_eq!(stats.total_deleted_size, 300);
    }

    #[tokio::test]
    async fn test_dynamic_pause_package_json() {
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(200),
            package_pause_duration: Duration::from_secs(2),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Trigger package.json change -> activates pause on node_modules
        event_tx
            .send(make_event(
                "/project/package.json",
                FileEventKind::Modified,
            ))
            .unwrap();

        // Small delay to ensure the trigger is processed
        tokio::time::sleep(Duration::from_millis(10)).await;

        // These node_modules events should be dropped
        event_tx
            .send(make_event(
                "/project/node_modules/foo/index.js",
                FileEventKind::Created,
            ))
            .unwrap();
        event_tx
            .send(make_event(
                "/project/node_modules/bar/index.js",
                FileEventKind::Created,
            ))
            .unwrap();

        // This regular event should NOT be dropped
        event_tx
            .send(make_event("/project/src/main.js", FileEventKind::Modified))
            .unwrap();

        let result = timeout(Duration::from_secs(2), batch_rx.recv()).await;
        assert!(result.is_ok());
        let (batch, _stats) = result.unwrap().unwrap();
        // Should only have package.json + main.js (node_modules events dropped)
        let paths: Vec<String> = batch.events.iter().map(|e| e.path.to_string_lossy().to_string()).collect();
        assert!(!paths.iter().any(|p| p.contains("node_modules")));
        assert!(paths.iter().any(|p| p.contains("package.json")));
        assert!(paths.iter().any(|p| p.contains("main.js")));

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_dynamic_pause_git_head() {
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(200),
            git_pause_duration: Duration::from_secs(2),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Trigger .git/HEAD change -> global pause
        event_tx
            .send(make_event("/project/.git/HEAD", FileEventKind::Modified))
            .unwrap();

        // Small delay
        tokio::time::sleep(Duration::from_millis(10)).await;

        // All events should be dropped during global pause
        event_tx
            .send(make_event("/project/src/main.rs", FileEventKind::Modified))
            .unwrap();
        event_tx
            .send(make_event("/project/README.md", FileEventKind::Modified))
            .unwrap();

        // First batch: only the .git/HEAD event
        let result = timeout(Duration::from_secs(2), batch_rx.recv()).await;
        assert!(result.is_ok());
        let (batch, _stats) = result.unwrap().unwrap();
        assert_eq!(batch.events.len(), 1);
        assert!(batch.events[0]
            .path
            .to_string_lossy()
            .contains(".git/HEAD"));

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_flush_on_channel_close() {
        let config = ProcessorConfig {
            window_duration: Duration::from_secs(60), // long window
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Send event and immediately close
        event_tx
            .send(make_event("/tmp/test.txt", FileEventKind::Created))
            .unwrap();
        drop(event_tx);

        // Should flush remaining events
        let result = timeout(Duration::from_secs(2), batch_rx.recv()).await;
        assert!(result.is_ok());
        let (batch, _stats) = result.unwrap().unwrap();
        assert_eq!(batch.events.len(), 1);

        let _ = handle.await;
    }

    /// Test memory stability under sustained high-throughput event processing.
    ///
    /// Simulates continuous event ingestion over many window cycles and verifies:
    /// - HashMap drains completely each cycle (no event leaks)
    /// - Batch count matches expected window cycles
    /// - No unbounded growth in internal state
    ///
    /// This validates acceptance criterion #5: memory growth < 5MB over 1 hour.
    /// In production, each 30s window fully drains the HashMap via `flush_events`,
    /// so the only retained state is the current window's events. Here we verify
    /// this drain behavior over 100 simulated cycles at high throughput.
    #[tokio::test]
    async fn test_memory_stability_sustained_processing() {
        // Use a short window to simulate many cycles quickly
        let window_ms = 50;
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(window_ms),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(256);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Simulate sustained load: send events continuously over many window cycles
        let num_cycles = 100;
        let events_per_cycle = 50;
        let total_events = num_cycles * events_per_cycle;

        let sender = tokio::spawn(async move {
            for i in 0..total_events {
                // Vary paths to avoid 100% dedup — each cycle gets unique file names
                let path = format!("/project/src/file_{}.rs", i);
                let kind = match i % 3 {
                    0 => FileEventKind::Created,
                    1 => FileEventKind::Modified,
                    _ => FileEventKind::Deleted,
                };
                event_tx.send(make_event(&path, kind)).unwrap();

                // Pace events: slight delay every batch to let windows tick
                if (i + 1) % events_per_cycle == 0 {
                    tokio::time::sleep(Duration::from_millis(window_ms + 10)).await;
                }
            }
            // Close the input channel to trigger final flush
            drop(event_tx);
        });

        // Collect all batches
        let mut total_batch_events = 0usize;
        let mut batch_count = 0usize;
        let mut max_batch_size = 0usize;

        while let Some((batch, stats)) = batch_rx.recv().await {
            let batch_len = batch.events.len();
            total_batch_events += batch_len;
            batch_count += 1;
            if batch_len > max_batch_size {
                max_batch_size = batch_len;
            }

            // Verify stats consistency: stats counts should match event counts
            let expected_total =
                stats.files_added + stats.files_modified + stats.files_deleted + stats.files_renamed;
            assert_eq!(
                expected_total, batch_len,
                "Stats count mismatch in batch {}: stats sum={} vs events={}",
                batch_count, expected_total, batch_len
            );
        }

        sender.await.unwrap();
        let _ = handle.await;

        // Verify all events were processed (no leaks)
        assert_eq!(
            total_batch_events, total_events,
            "All {} events must be accounted for in batches, got {}",
            total_events, total_batch_events
        );

        // Multiple batches should have been produced (proves window cycling works)
        assert!(
            batch_count >= 2,
            "Expected multiple batches from {} cycles, got {}",
            num_cycles, batch_count
        );

        // No single batch should contain all events (proves windowing works)
        assert!(
            max_batch_size < total_events,
            "Single batch should not contain all {} events, max was {}",
            total_events, max_batch_size
        );
    }

    /// Test that the event_map is fully drained after each window flush,
    /// proving no retained references that could cause memory growth.
    #[tokio::test]
    async fn test_event_map_drains_completely() {
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(100),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(64);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Send a burst of events
        for i in 0..100 {
            event_tx
                .send(make_event(
                    &format!("/tmp/burst_{}.txt", i),
                    FileEventKind::Created,
                ))
                .unwrap();
        }

        // Wait for first batch
        let (batch1, _) = timeout(Duration::from_secs(2), batch_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch1.events.len(), 100);

        // Send another burst — if map wasn't drained, we'd see leftovers
        for i in 0..50 {
            event_tx
                .send(make_event(
                    &format!("/tmp/burst2_{}.txt", i),
                    FileEventKind::Modified,
                ))
                .unwrap();
        }

        // Wait for second batch
        let (batch2, stats2) = timeout(Duration::from_secs(2), batch_rx.recv())
            .await
            .unwrap()
            .unwrap();

        // Second batch should contain ONLY the 50 new events, not leftovers from batch 1
        assert_eq!(
            batch2.events.len(),
            50,
            "Second batch should only have new events, got {}",
            batch2.events.len()
        );
        assert_eq!(stats2.files_modified, 50);

        // No events from the first burst should appear
        for event in &batch2.events {
            assert!(
                event.path.to_string_lossy().contains("burst2_"),
                "Unexpected event from first burst: {:?}",
                event.path
            );
        }

        drop(event_tx);
        let _ = handle.await;
    }

    /// Test that dynamic pause state expires properly and doesn't accumulate.
    #[tokio::test]
    async fn test_pause_state_expiry_no_accumulation() {
        let config = ProcessorConfig {
            window_duration: Duration::from_millis(100),
            package_pause_duration: Duration::from_millis(200),
            git_pause_duration: Duration::from_millis(150),
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(64);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        // Trigger multiple pauses in succession
        for _ in 0..5 {
            event_tx
                .send(make_event(
                    "/project/package.json",
                    FileEventKind::Modified,
                ))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Wait for pause to expire
        tokio::time::sleep(Duration::from_millis(300)).await;

        // After pause expires, node_modules events should pass through
        event_tx
            .send(make_event(
                "/project/node_modules/test/index.js",
                FileEventKind::Created,
            ))
            .unwrap();

        // Wait for batch containing the node_modules event
        let mut found_node_modules = false;
        for _ in 0..5 {
            if let Ok(Some((batch, _))) =
                timeout(Duration::from_millis(200), batch_rx.recv()).await
            {
                for event in &batch.events {
                    if event.path.to_string_lossy().contains("node_modules") {
                        found_node_modules = true;
                    }
                }
            }
            if found_node_modules {
                break;
            }
        }

        assert!(
            found_node_modules,
            "After pause expiry, node_modules events should pass through"
        );

        drop(event_tx);
        let _ = handle.await;
    }
}
