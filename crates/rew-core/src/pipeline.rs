//! Pipeline: connects FileWatcher -> EventProcessor -> downstream consumers.
//!
//! This module wires together the MacOSWatcher and EventProcessor via tokio
//! mpsc channels, forming the async event processing pipeline.

use crate::config::RewConfig;
use crate::error::RewResult;
use crate::processor::{BatchStats, EventProcessor, ProcessorConfig};
use crate::types::EventBatch;
use crate::watcher::filter::PathFilter;
use crate::watcher::macos::MacOSWatcher;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

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
    // Build path filter from config
    let filter = PathFilter::new(&config.ignore_patterns)
        .map_err(|e| crate::error::RewError::Config(format!("Invalid glob pattern: {}", e)))?;

    // Create and start watcher
    let mut watcher = MacOSWatcher::new(filter);
    let event_rx = watcher.start(&config.watch_dirs)?;

    // Create event processor
    let processor = EventProcessor::new(processor_config);

    // Create batch output channel
    let (batch_tx, batch_rx) = mpsc::channel(64);

    // Spawn processor task
    let processor_handle = tokio::spawn(async move {
        processor.run(event_rx, batch_tx).await;
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
}
