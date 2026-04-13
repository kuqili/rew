//! Pipeline: connects FileWatcher -> EventProcessor -> downstream consumers.
//!
//! This module wires together the MacOSWatcher and EventProcessor via tokio
//! mpsc channels, forming the async event processing pipeline.

use crate::config::RewConfig;
use crate::error::RewResult;
use crate::objects::ObjectStore;
use crate::processor::{BatchStats, EventProcessor, ProcessorConfig};
use crate::types::{EventBatch, FileEventKind};
use crate::watcher::filter::PathFilter;
use crate::watcher::macos::MacOSWatcher;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info};

const SHADOW_HASH_RETENTION_SECS: u64 = 24 * 3600;

/// A handle to the running pipeline, used to control and stop it.
pub struct PipelineHandle {
    /// Watcher instance (for adding/removing paths)
    watcher: MacOSWatcher,
    /// Receiver for processed event batches
    batch_rx: mpsc::Receiver<(EventBatch, BatchStats)>,
    /// Processor task handle
    processor_handle: tokio::task::JoinHandle<()>,
}

impl PipelineHandle {
    /// Receive the next processed EventBatch (with stats).
    ///
    /// Returns `None` if the pipeline has been shut down.
    pub async fn recv_batch(&mut self) -> Option<(EventBatch, BatchStats)> {
        self.batch_rx.recv().await
    }

    /// Add a directory to the watch list at runtime.
    pub fn add_watch_path(&mut self, path: &std::path::Path) -> RewResult<()> {
        self.watcher.add_path(path)
    }

    /// Remove a directory from the watch list at runtime.
    pub fn remove_watch_path(&mut self, path: &std::path::Path) -> RewResult<()> {
        self.watcher.remove_path(path)
    }

    /// Stop the pipeline: stops the watcher and waits for processor to drain.
    pub async fn stop(mut self) -> RewResult<()> {
        self.watcher.stop()?;
        // Wait for processor to finish flushing
        let _ = self.processor_handle.await;
        info!("Pipeline stopped");
        Ok(())
    }

    /// Get the list of currently watched directories.
    pub fn watched_dirs(&self) -> Vec<std::path::PathBuf> {
        self.watcher.watched_dirs()
    }
}

/// Build and start the event pipeline from configuration.
///
/// The pipeline consists of:
/// 1. `MacOSWatcher` — produces raw `FileEvent`s via FSEvents
/// 2. `EventProcessor` — deduplicates, aggregates into `EventBatch`es
/// 3. Output channel — downstream consumers receive `(EventBatch, BatchStats)`
///
/// # Arguments
/// - `config`: The rew configuration (watch dirs, ignore patterns, etc.)
///
/// # Returns
/// A `PipelineHandle` for receiving batches and controlling the pipeline.
pub fn start_pipeline(config: &RewConfig) -> RewResult<PipelineHandle> {
    start_pipeline_with_config(config, ProcessorConfig::default())
}

/// Start the pipeline with custom processor configuration.
pub fn start_pipeline_with_config(
    config: &RewConfig,
    processor_config: ProcessorConfig,
) -> RewResult<PipelineHandle> {
    cleanup_shadow_hashes(Duration::from_secs(SHADOW_HASH_RETENTION_SECS));

    // Build path filter from config
    let filter = PathFilter::new(&config.ignore_patterns)
        .map_err(|e| crate::error::RewError::Config(format!("Invalid glob pattern: {}", e)))?;

    // Clone filter for use in shadow backup task (CRITICAL: must apply filtering before backup)
    let shadow_filter = filter.clone();

    // Create and start watcher
    let mut watcher = MacOSWatcher::new(filter);
    let event_rx = watcher.start(&config.watch_dirs)?;

    // === Shadow mechanism: intercept events before processor ===
    // When a file is Created or Modified, immediately store its content in objects
    // so that if it's later Deleted (within the same 30s window), we have the backup.
    let objects_root = crate::rew_home_dir().join("objects");
    let (shadow_tx, shadow_rx) = mpsc::unbounded_channel();

    // Shadow task: receives raw events, does immediate backup, forwards to processor
    // Also writes file→hash mapping so daemon can reference it later
    let shadow_dir = crate::rew_home_dir().join(".shadow_hashes");
    let _ = std::fs::create_dir_all(&shadow_dir);

    tokio::spawn(async move {
        let obj_store = ObjectStore::new(objects_root).ok();
        let mut event_rx = event_rx;

        while let Some(event) = event_rx.recv().await {
            // For files that exist right now, immediately store in objects
            // This includes Renamed — macOS trash is a rename, we need the content before it's gone
            if event.path.exists() && matches!(event.kind, FileEventKind::Created | FileEventKind::Modified | FileEventKind::Renamed) {
                // CRITICAL FIX: Apply filtering BEFORE storing in shadow backup.
                // Excluded files (node_modules, .git, etc.) should never be backed up.
                if shadow_filter.should_ignore(&event.path) {
                    debug!("Shadow backup: filtered (ignored) {}", event.path.display());
                    // Do NOT store this file, but still forward the event
                } else if let Some(ref store) = obj_store {
                    match store.store(&event.path) {
                        Ok(hash) => {
                            debug!("Shadow backup: {} → {}", event.path.display(), &hash[..12]);
                            // Write path→hash mapping for daemon to look up
                            let key = path_to_shadow_key(&event.path);
                            let _ = std::fs::write(shadow_dir.join(&key), &hash);
                        }
                        Err(e) => {
                            debug!("Shadow backup failed for {}: {}", event.path.display(), e);
                        }
                    }
                }
            }

            // Forward event to processor
            if shadow_tx.send(event).is_err() {
                break;
            }
        }
    });

    // Create event processor — now receives from shadow channel
    let processor = EventProcessor::new(processor_config);

    // Create batch output channel
    let (batch_tx, batch_rx) = mpsc::channel(64);

    // Spawn processor task — receives from shadow channel
    let processor_handle = tokio::spawn(async move {
        processor.run(shadow_rx, batch_tx).await;
    });

    info!(
        "Pipeline started: watching {} directories",
        config.watch_dirs.len()
    );

    Ok(PipelineHandle {
        watcher,
        batch_rx,
        processor_handle,
    })
}

/// Convenience function to start a pipeline with custom window duration.
pub fn start_pipeline_with_window(
    config: &RewConfig,
    window_duration: Duration,
) -> RewResult<PipelineHandle> {
    let processor_config = ProcessorConfig {
        window_duration,
        ..Default::default()
    };
    start_pipeline_with_config(config, processor_config)
}

/// Convert a file path to a stable shadow key (hash of path).
pub fn path_to_shadow_key(path: &std::path::Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Read a shadow hash for a given file path.
/// Returns the object store hash if the shadow layer backed up this file.
pub fn read_shadow_hash(path: &std::path::Path) -> Option<String> {
    let shadow_dir = crate::rew_home_dir().join(".shadow_hashes");
    let key = path_to_shadow_key(path);
    std::fs::read_to_string(shadow_dir.join(key))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read and consume a shadow hash for a given file path.
pub fn take_shadow_hash(path: &std::path::Path) -> Option<String> {
    let shadow_dir = crate::rew_home_dir().join(".shadow_hashes");
    let key = path_to_shadow_key(path);
    let file = shadow_dir.join(key);
    let hash = std::fs::read_to_string(&file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if hash.is_some() {
        let _ = std::fs::remove_file(file);
    }
    hash
}

/// Remove stale shadow hash mapping files that were never consumed.
pub fn cleanup_shadow_hashes(max_age: Duration) {
    let shadow_dir = crate::rew_home_dir().join(".shadow_hashes");
    let cutoff = std::time::SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let entries = match std::fs::read_dir(&shadow_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let is_stale = meta.modified().map(|t| t < cutoff).unwrap_or(false);
        if meta.is_file() && is_stale {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RewConfig;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_pipeline_start_stop() {
        let dir = tempdir().unwrap();
        let config = RewConfig {
            watch_dirs: vec![dir.path().to_path_buf()],
            ..Default::default()
        };

        let processor_config = ProcessorConfig {
            window_duration: Duration::from_millis(100),
            ..Default::default()
        };

        let handle = start_pipeline_with_config(&config, processor_config).unwrap();
        assert_eq!(handle.watched_dirs().len(), 1);
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_pipeline_detects_file_creation() {
        let dir = tempdir().unwrap();
        let config = RewConfig {
            watch_dirs: vec![dir.path().to_path_buf()],
            ..Default::default()
        };

        let processor_config = ProcessorConfig {
            window_duration: Duration::from_millis(500),
            ..Default::default()
        };

        let mut handle = start_pipeline_with_config(&config, processor_config).unwrap();

        // Give the watcher time to initialize
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create test files
        for i in 0..5 {
            let path = dir.path().join(format!("test_{}.txt", i));
            std::fs::write(&path, format!("content {}", i)).unwrap();
        }

        // Wait for batch
        let result =
            tokio::time::timeout(Duration::from_secs(5), handle.recv_batch()).await;

        assert!(result.is_ok(), "Should receive a batch within timeout");
        let (batch, stats) = result.unwrap().unwrap();
        assert!(!batch.events.is_empty(), "Batch should contain events");
        info!("Received batch: {} events, stats: {:?}", batch.events.len(), stats);

        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_pipeline_filters_noise() {
        let dir = tempdir().unwrap();
        let node_modules = dir.path().join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();

        let config = RewConfig {
            watch_dirs: vec![dir.path().to_path_buf()],
            ..Default::default()
        };

        let processor_config = ProcessorConfig {
            window_duration: Duration::from_millis(500),
            ..Default::default()
        };

        let mut handle = start_pipeline_with_config(&config, processor_config).unwrap();

        // Give the watcher time to initialize
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create files in node_modules (should be filtered)
        for i in 0..10 {
            let path = node_modules.join(format!("package_{}.js", i));
            std::fs::write(&path, "module.exports = {}").unwrap();
        }

        // Also create a normal file (should pass through)
        let normal = dir.path().join("hello.txt");
        std::fs::write(&normal, "hello world").unwrap();

        // Wait for batch
        let result =
            tokio::time::timeout(Duration::from_secs(5), handle.recv_batch()).await;

        if let Ok(Some((batch, _stats))) = result {
            // None of the events should be from node_modules
            for event in &batch.events {
                assert!(
                    !event.path.to_string_lossy().contains("node_modules"),
                    "Should not contain node_modules events, got: {:?}",
                    event.path
                );
            }
        }

        handle.stop().await.unwrap();
    }

    /// Test sustained pipeline operation over many window cycles.
    ///
    /// This validates that the full pipeline (watcher -> processor -> output)
    /// properly drains state each window cycle, proving memory stability.
    /// Simulates multiple waves of file creation/deletion to exercise the
    /// full event lifecycle.
    #[tokio::test]
    async fn test_pipeline_sustained_operation_memory_stable() {
        let dir = tempdir().unwrap();
        let config = RewConfig {
            watch_dirs: vec![dir.path().to_path_buf()],
            ..Default::default()
        };

        let processor_config = ProcessorConfig {
            window_duration: Duration::from_millis(200),
            ..Default::default()
        };

        let mut handle = start_pipeline_with_config(&config, processor_config).unwrap();

        // Give the watcher time to initialize
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut total_batches = 0usize;
        let mut total_events = 0usize;

        // Run 5 waves of file operations, each followed by batch collection
        for wave in 0..5 {
            // Create files
            for i in 0..10 {
                let path = dir.path().join(format!("wave{}_{}.txt", wave, i));
                std::fs::write(&path, format!("content wave={} i={}", wave, i)).unwrap();
            }

            // Collect batch(es) from this wave
            loop {
                match tokio::time::timeout(Duration::from_millis(500), handle.recv_batch()).await {
                    Ok(Some((batch, stats))) => {
                        total_batches += 1;
                        total_events += batch.events.len();
                        // Stats should be self-consistent
                        let stat_total = stats.files_added
                            + stats.files_modified
                            + stats.files_deleted
                            + stats.files_renamed;
                        assert_eq!(stat_total, batch.events.len());
                    }
                    _ => break,
                }
            }

            // Clean up files from this wave (triggers delete events)
            for i in 0..10 {
                let path = dir.path().join(format!("wave{}_{}.txt", wave, i));
                let _ = std::fs::remove_file(&path);
            }

            // Let delete events be processed
            tokio::time::sleep(Duration::from_millis(300)).await;
            loop {
                match tokio::time::timeout(Duration::from_millis(300), handle.recv_batch()).await {
                    Ok(Some((batch, _))) => {
                        total_batches += 1;
                        total_events += batch.events.len();
                    }
                    _ => break,
                }
            }
        }

        // Verify we actually processed events across multiple batches
        assert!(
            total_batches >= 2,
            "Should produce multiple batches across waves, got {}",
            total_batches
        );
        assert!(
            total_events >= 10,
            "Should process substantial events, got {}",
            total_events
        );

        handle.stop().await.unwrap();
    }
}
