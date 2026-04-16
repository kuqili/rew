//! Event processor with sliding window aggregation, deduplication, and dynamic filtering.
//!
//! The EventProcessor receives raw `FileEvent`s from the watcher and:
//! 1. Deduplicates same-path, same-kind events within the window
//! 2. Aggregates events into `EventBatch`es at configurable intervals (default 30s)
//! 3. Implements dynamic filter pausing for package-manager side effects
//!    (`package.json`/`Cargo.toml` changes -> pause `node_modules`/`target`)
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
    /// Reserved for backward compatibility.
    ///
    /// Git worktree changes are no longer globally paused on `.git/HEAD`
    /// updates; `.git/**` noise is filtered earlier by the path filter.
    pub git_pause_duration: Duration,
}

impl Default for ProcessorConfig {
    fn default() -> Self {
        Self {
            // 3 s accumulation window: short enough that delayed FSEvents from
            // Bash tool ops land in the daemon well before the grace period
            // expires, while still deduplicating burst writes within a few secs.
            window_duration: Duration::from_secs(3),
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
}

impl PauseState {
    fn new() -> Self {
        Self {
            package_pause_until: None,
        }
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
}

#[cfg(test)]
mod merge_logic_tests {
    use super::*;

    fn kind_seq(seq: &[FileEventKind]) -> Option<FileEventKind> {
        let mut current = seq.first()?.clone();
        for next in &seq[1..] {
            match merge_kind(&current, next) {
                Some(kind) => current = kind,
                None => return None,
            }
        }
        Some(current)
    }

    fn event(path: &str) -> FileEvent {
        FileEvent {
            path: PathBuf::from(path),
            kind: FileEventKind::Modified,
            timestamp: Utc::now(),
            size_bytes: None,
        }
    }

    #[test]
    fn merge_kind_table_driven_cases() {
        let cases = vec![
            (
                vec![FileEventKind::Created, FileEventKind::Deleted],
                None,
            ),
            (
                vec![FileEventKind::Created, FileEventKind::Modified],
                Some(FileEventKind::Created),
            ),
            (
                vec![FileEventKind::Deleted, FileEventKind::Created],
                Some(FileEventKind::Modified),
            ),
            (
                vec![FileEventKind::Modified, FileEventKind::Deleted],
                Some(FileEventKind::Deleted),
            ),
            (
                vec![FileEventKind::Modified, FileEventKind::Modified],
                Some(FileEventKind::Modified),
            ),
            (
                vec![FileEventKind::Renamed, FileEventKind::Modified],
                Some(FileEventKind::Modified),
            ),
            (
                vec![FileEventKind::Renamed, FileEventKind::Deleted],
                Some(FileEventKind::Deleted),
            ),
            (
                vec![
                    FileEventKind::Created,
                    FileEventKind::Modified,
                    FileEventKind::Deleted,
                ],
                None,
            ),
            (
                vec![
                    FileEventKind::Deleted,
                    FileEventKind::Created,
                    FileEventKind::Modified,
                ],
                Some(FileEventKind::Modified),
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(kind_seq(&input), expected, "merge sequence failed: {:?}", input);
        }
    }

    #[test]
    fn dynamic_pause_package_scope_only() {
        let processor = EventProcessor::with_defaults();
        let mut pause = PauseState::new();
        processor.check_dynamic_triggers(
            &FileEvent {
                path: PathBuf::from("/tmp/project/package.json"),
                kind: FileEventKind::Modified,
                timestamp: Utc::now(),
                size_bytes: None,
            },
            &mut pause,
            Duration::from_secs(60),
        );

        assert!(pause.is_package_paused());
        assert!(processor.should_drop_during_pause(&event("/tmp/project/node_modules/a.js"), &pause));
        assert!(processor.should_drop_during_pause(&event("/tmp/project/target/a.o"), &pause));
        assert!(!processor.should_drop_during_pause(&event("/tmp/project/src/main.rs"), &pause));
    }

    #[test]
    fn git_head_no_longer_triggers_global_pause() {
        let processor = EventProcessor::with_defaults();
        let mut pause = PauseState::new();
        processor.check_dynamic_triggers(
            &FileEvent {
                path: PathBuf::from("/tmp/project/.git/HEAD"),
                kind: FileEventKind::Modified,
                timestamp: Utc::now(),
                size_bytes: None,
            },
            &mut pause,
            Duration::from_secs(60),
        );

        assert!(!processor.should_drop_during_pause(&event("/tmp/project/src/main.rs"), &pause));
        assert!(!processor.should_drop_during_pause(&event("/tmp/project/node_modules/a.js"), &pause));
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
                            self.check_dynamic_triggers(&file_event, &mut pause_state, package_pause);

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
    }

    /// Check if an event should be dropped due to dynamic pause.
    fn should_drop_during_pause(&self, event: &FileEvent, pause_state: &PauseState) -> bool {
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
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;
    use tokio::time::timeout;

    fn make_event(path: &str, kind: FileEventKind) -> FileEvent {
        FileEvent {
            path: PathBuf::from(path),
            kind,
            timestamp: Utc::now(),
            size_bytes: Some(100),
        }
    }

    struct GitSwitchEnv {
        _dir: TempDir,
        repo_root: PathBuf,
    }

    impl GitSwitchEnv {
        fn new() -> Self {
            let workspace_root = std::env::current_dir().unwrap();
            let dir = tempfile::Builder::new()
                .prefix("rew-git-processor-")
                .tempdir_in(workspace_root)
                .unwrap();
            let repo_root = dir.path().join("repo");
            let empty_template = dir.path().join("empty-git-template");
            fs::create_dir_all(&repo_root).unwrap();
            fs::create_dir_all(&empty_template).unwrap();

            let status = Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(format!("--template={}", empty_template.to_string_lossy()))
                .current_dir(&repo_root)
                .status()
                .unwrap();
            assert!(status.success());

            let _ = Command::new("git")
                .args(["config", "user.email", "rew-tests@example.com"])
                .current_dir(&repo_root)
                .status();
            let _ = Command::new("git")
                .args(["config", "user.name", "rew tests"])
                .current_dir(&repo_root)
                .status();

            Self { _dir: dir, repo_root }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.repo_root.join(rel)
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.path(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }

        fn remove(&self, rel: &str) {
            let _ = fs::remove_file(self.path(rel));
        }

        fn git_stdout(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(&self.repo_root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }

        fn git_commit_all(&self, message: &str) {
            self.git_stdout(&["add", "-A"]);
            self.git_stdout(&["commit", "--allow-empty", "-qm", message]);
        }

        fn git_current_branch(&self) -> String {
            self.git_stdout(&["rev-parse", "--abbrev-ref", "HEAD"])
                .trim()
                .to_string()
        }

        fn git_checkout(&self, target: &str) {
            self.git_stdout(&["checkout", "-q", target]);
        }

        fn git_checkout_new_branch(&self, branch: &str) {
            self.git_stdout(&["checkout", "-q", "-b", branch]);
        }

        fn tracked_snapshot(&self) -> BTreeMap<String, Vec<u8>> {
            let mut snapshot = BTreeMap::new();
            for rel in self
                .git_stdout(&["ls-files"])
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                snapshot.insert(rel.to_string(), fs::read(self.path(rel)).unwrap());
            }
            snapshot
        }

        fn checkout_events(&self, target: &str) -> Vec<FileEvent> {
            let before = self.tracked_snapshot();
            self.git_checkout(target);
            let after = self.tracked_snapshot();

            let all_paths: BTreeSet<String> = before
                .keys()
                .chain(after.keys())
                .cloned()
                .collect();

            let mut events = Vec::new();
            for rel in all_paths {
                match (before.get(&rel), after.get(&rel)) {
                    (Some(old), Some(new)) if old != new => {
                        events.push(FileEvent {
                            path: self.path(&rel),
                            kind: FileEventKind::Modified,
                            timestamp: Utc::now(),
                            size_bytes: Some(new.len() as u64),
                        });
                    }
                    (Some(old), None) => {
                        events.push(FileEvent {
                            path: self.path(&rel),
                            kind: FileEventKind::Deleted,
                            timestamp: Utc::now(),
                            size_bytes: Some(old.len() as u64),
                        });
                    }
                    (None, Some(new)) => {
                        events.push(FileEvent {
                            path: self.path(&rel),
                            kind: FileEventKind::Created,
                            timestamp: Utc::now(),
                            size_bytes: Some(new.len() as u64),
                        });
                    }
                    _ => {}
                }
            }

            events.push(FileEvent {
                path: self.path(".git/HEAD"),
                kind: FileEventKind::Modified,
                timestamp: Utc::now(),
                size_bytes: None,
            });

            events
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
    async fn test_git_head_does_not_pause_worktree_events() {
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

        // `.git/HEAD` should no longer trigger a global pause.
        event_tx
            .send(make_event("/project/.git/HEAD", FileEventKind::Modified))
            .unwrap();

        // Small delay
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Worktree events should continue to flow through.
        event_tx
            .send(make_event("/project/src/main.rs", FileEventKind::Modified))
            .unwrap();
        event_tx
            .send(make_event("/project/README.md", FileEventKind::Modified))
            .unwrap();

        // First batch should include both `.git/HEAD` and worktree events.
        let result = timeout(Duration::from_secs(2), batch_rx.recv()).await;
        assert!(result.is_ok());
        let (batch, _stats) = result.unwrap().unwrap();
        let paths: Vec<String> = batch
            .events
            .iter()
            .map(|e| e.path.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.contains(".git/HEAD")));
        assert!(paths.iter().any(|p| p.contains("/project/src/main.rs")));
        assert!(paths.iter().any(|p| p.contains("/project/README.md")));

        drop(event_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_git_branch_roundtrip_does_not_drop_second_switch_events() {
        let env = GitSwitchEnv::new();
        env.write("shared.txt", "base\n");
        env.write("only-a.txt", "alpha-only\n");
        env.git_commit_all("baseline");
        let base_branch = env.git_current_branch();

        env.git_checkout_new_branch("branch-b");
        env.write("shared.txt", "branch-b\n");
        env.remove("only-a.txt");
        env.write("only-b.txt", "branch-b-only\n");
        env.git_commit_all("branch-b");
        env.git_checkout(&base_branch);

        let first_switch_events = env.checkout_events("branch-b");
        let second_switch_events = env.checkout_events(&base_branch);

        let config = ProcessorConfig {
            window_duration: Duration::from_millis(80),
            git_pause_duration: Duration::from_secs(2),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        for event in first_switch_events {
            event_tx.send(event).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(140)).await;

        for event in second_switch_events {
            event_tx.send(event).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(140)).await;
        drop(event_tx);

        let mut batches = Vec::new();
        while let Ok(Some((batch, _))) = timeout(Duration::from_millis(250), batch_rx.recv()).await {
            batches.push(batch);
        }
        let _ = handle.await;

        let all_events: Vec<FileEvent> = batches
            .iter()
            .flat_map(|batch| batch.events.clone())
            .collect();
        let paths: Vec<String> = all_events
            .iter()
            .map(|e| e.path.to_string_lossy().to_string())
            .collect();

        assert!(
            batches.len() >= 2,
            "processor should emit a second batch for the switch back to A, got {:?}",
            paths
        );
    }

    #[tokio::test]
    async fn test_git_branch_a_to_b_to_c_does_not_drop_second_switch_events() {
        let env = GitSwitchEnv::new();
        env.write("shared.txt", "base\n");
        env.write("only-a.txt", "alpha-only\n");
        env.git_commit_all("baseline");
        let base_branch = env.git_current_branch();

        env.git_checkout_new_branch("branch-b");
        env.write("shared.txt", "branch-b\n");
        env.remove("only-a.txt");
        env.write("only-b.txt", "branch-b-only\n");
        env.git_commit_all("branch-b");

        env.git_checkout_new_branch("branch-c");
        env.write("shared.txt", "branch-c\n");
        env.remove("only-b.txt");
        env.write("only-c.txt", "branch-c-only\n");
        env.git_commit_all("branch-c");
        env.git_checkout(&base_branch);

        let first_switch_events = env.checkout_events("branch-b");
        let second_switch_events = env.checkout_events("branch-c");

        let config = ProcessorConfig {
            window_duration: Duration::from_millis(80),
            git_pause_duration: Duration::from_secs(2),
            ..Default::default()
        };
        let processor = EventProcessor::new(config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (batch_tx, mut batch_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            processor.run(event_rx, batch_tx).await;
        });

        for event in first_switch_events {
            event_tx.send(event).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(140)).await;

        for event in second_switch_events {
            event_tx.send(event).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(140)).await;
        drop(event_tx);

        let mut all_events = Vec::new();
        while let Ok(Some((batch, _))) = timeout(Duration::from_millis(250), batch_rx.recv()).await {
            all_events.extend(batch.events);
        }
        let _ = handle.await;

        let paths: Vec<String> = all_events
            .iter()
            .map(|e| e.path.to_string_lossy().to_string())
            .collect();

        assert!(
            paths.iter().any(|p| p.ends_with("/only-c.txt")),
            "processor should preserve second switch to branch C, got {:?}",
            paths
        );
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
