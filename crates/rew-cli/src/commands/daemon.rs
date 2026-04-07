//! `rew daemon` — Run the rew daemon in the foreground.
//!
//! The daemon watches configured directories, detects anomalies,
//! creates snapshots, and sends notifications.

use rew_core::config::RewConfig;
use rew_core::error::RewResult;
use rew_core::lifecycle;
use rew_core::logging;

/// Run the rew daemon.
///
/// This blocks until a shutdown signal is received (SIGTERM/SIGINT).
pub fn run() -> RewResult<()> {
    let rew_dir = rew_core::rew_home_dir();
    std::fs::create_dir_all(&rew_dir)?;

    // Initialize logging
    let log_dir = logging::log_dir();
    let _log_guard = logging::init_logging(&log_dir)?;

    tracing::info!("rew daemon starting...");

    // DB integrity check
    let db_path = rew_dir.join("snapshots.db");
    lifecycle::check_db_integrity(&db_path)?;

    // Write PID file
    lifecycle::write_pid_file(&rew_dir)?;

    // Load config
    let config_path = rew_dir.join("config.toml");
    let config = if config_path.exists() {
        RewConfig::load(&config_path)?
    } else {
        let default = RewConfig::default();
        default.save(&config_path)?;
        default
    };

    // Setup shutdown signal
    let shutdown = lifecycle::create_shutdown_signal();

    tracing::info!(
        "rew daemon started (pid={}, watching {} dirs)",
        std::process::id(),
        config.watch_dirs.len()
    );

    // Main daemon loop
    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        rew_core::error::RewError::Config(format!("Failed to create tokio runtime: {}", e))
    })?;

    rt.block_on(async {
        daemon_loop(&config, &db_path, &shutdown).await
    })?;

    // Cleanup
    tracing::info!("rew daemon shutting down gracefully...");
    lifecycle::remove_pid_file(&rew_dir);
    tracing::info!("rew daemon stopped");

    Ok(())
}

async fn daemon_loop(
    config: &RewConfig,
    _db_path: &std::path::Path,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> RewResult<()> {
    use rew_core::detector::RuleEngine;
    use rew_core::pipeline;
    use rew_core::traits::AnomalyDetector;
    use rew_core::types::FileEventKind;
    use std::sync::atomic::Ordering;

    // Check if there are valid watch directories
    let has_dirs = config
        .watch_dirs
        .iter()
        .any(|d| d.exists());

    if !has_dirs {
        tracing::warn!("No valid watch directories configured. Waiting for configuration...");
        while !shutdown.load(Ordering::SeqCst) {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
        return Ok(());
    }

    // Start the pipeline
    let mut handle = pipeline::start_pipeline(config)?;

    let rule_engine = RuleEngine::new(config.anomaly_rules.clone(), config.watch_dirs.clone());

    tracing::info!("File watching pipeline started");

    // Main event processing loop
    loop {
        tokio::select! {
            batch_result = handle.recv_batch() => {
                match batch_result {
                    Some((event_batch, _stats)) => {
                        let added = event_batch.count_by_kind(&FileEventKind::Created);
                        let modified = event_batch.count_by_kind(&FileEventKind::Modified);
                        let deleted = event_batch.count_by_kind(&FileEventKind::Deleted);

                        tracing::debug!(
                            "Received batch: {} events (added={}, modified={}, deleted={})",
                            event_batch.events.len(),
                            added,
                            modified,
                            deleted,
                        );

                        // Run anomaly detection
                        let alerts = rule_engine.analyze(&event_batch);
                        let is_anomaly = !alerts.is_empty();

                        if is_anomaly {
                            for alert in &alerts {
                                tracing::warn!(
                                    "Anomaly detected: [{}] {} - {}",
                                    alert.severity,
                                    alert.kind_str(),
                                    alert.description
                                );
                            }
                        }

                        // Log batch processing (snapshot creation requires root/tmutil access)
                        tracing::info!(
                            "Batch processed: {} events, anomaly={}",
                            event_batch.events.len(),
                            is_anomaly
                        );
                    }
                    None => {
                        tracing::info!("Pipeline channel closed");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                if shutdown.load(Ordering::SeqCst) {
                    tracing::info!("Shutdown signal detected, stopping...");
                    break;
                }
            }
        }
    }

    // Stop the pipeline
    handle.stop().await?;

    Ok(())
}
