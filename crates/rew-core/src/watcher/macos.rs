//! macOS-specific file watcher implementation using FSEvents via the `notify` crate.

use crate::error::{RewError, RewResult};
use crate::types::{FileEvent, FileEventKind};
use crate::watcher::filter::PathFilter;
use chrono::Utc;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// macOS file watcher that uses FSEvents under the hood.
///
/// Wraps the `notify` crate's `RecommendedWatcher` (which uses FSEvents on macOS).
/// Events are filtered through a `PathFilter` before being forwarded.
pub struct MacOSWatcher {
    /// The underlying notify watcher (None when stopped)
    watcher: Option<RecommendedWatcher>,
    /// Directories currently being watched
    watched_dirs: Arc<Mutex<Vec<PathBuf>>>,
    /// Path filter for ignoring noise
    filter: PathFilter,
}

impl MacOSWatcher {
    /// Create a new MacOSWatcher with the given path filter.
    pub fn new(filter: PathFilter) -> Self {
        Self {
            watcher: None,
            watched_dirs: Arc::new(Mutex::new(Vec::new())),
            filter,
        }
    }

    /// Start watching the given directories.
    ///
    /// Returns an `UnboundedReceiver` that will receive filtered `FileEvent`s.
    pub fn start(
        &mut self,
        dirs: &[PathBuf],
    ) -> RewResult<mpsc::UnboundedReceiver<FileEvent>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let filter = self.filter.clone();

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    let kind = match event.kind {
                        EventKind::Create(_) => Some(FileEventKind::Created),
                        EventKind::Modify(modify_kind) => {
                            use notify::event::ModifyKind;
                            match modify_kind {
                                // On macOS/APFS, `rm` often triggers a Name-change
                                // event (atomic unlink via rename) rather than Remove.
                                // Check at the earliest possible moment — right here in
                                // the notify callback — whether the file still exists.
                                // If it doesn't, treat the event as Deleted so we never
                                // persist "renamed" for a plain `rm`.
                                ModifyKind::Name(_) => {
                                    let first_path = event.paths.first();
                                    if first_path.map(|p| p.exists()).unwrap_or(true) {
                                        Some(FileEventKind::Renamed)
                                    } else {
                                        Some(FileEventKind::Deleted)
                                    }
                                }
                                _ => Some(FileEventKind::Modified),
                            }
                        }
                        EventKind::Remove(_) => Some(FileEventKind::Deleted),
                        _ => None,
                    };

                    if let Some(kind) = kind {
                        for path in &event.paths {
                            if filter.should_process(path) {
                                let file_event = FileEvent {
                                    path: path.clone(),
                                    kind: kind.clone(),
                                    timestamp: Utc::now(),
                                    size_bytes: std::fs::metadata(path)
                                        .ok()
                                        .map(|m| m.len()),
                                };
                                info!("FSE ✓ {:?} {}", file_event.kind, file_event.path.display());
                                if tx.send(file_event).is_err() {
                                    warn!("Event channel closed, stopping event forwarding");
                                    return;
                                }
                            } else {
                                debug!("FSE ✗ filtered {:?} {}", kind, path.display());
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Watch error: {:?}", e);
                }
            }
        })
        .map_err(|e| RewError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        self.watcher = Some(watcher);

        // Watch all requested directories
        for dir in dirs {
            self.add_path(dir)?;
        }

        info!("MacOSWatcher started, watching {} directories", dirs.len());

        Ok(rx)
    }

    /// Add a directory to the watch list.
    pub fn add_path(&mut self, path: &Path) -> RewResult<()> {
        if let Some(ref mut watcher) = self.watcher {
            if !path.exists() {
                warn!("Watch path does not exist, skipping: {:?}", path);
                return Ok(());
            }
            watcher
                .watch(path, RecursiveMode::Recursive)
                .map_err(|e| {
                    RewError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to watch {:?}: {}", path, e),
                    ))
                })?;
            let mut dirs = self.watched_dirs.lock().unwrap();
            dirs.push(path.to_path_buf());
            info!("Now watching: {:?}", path);
            Ok(())
        } else {
            Err(RewError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Watcher not started",
            )))
        }
    }

    /// Remove a directory from the watch list.
    pub fn remove_path(&mut self, path: &Path) -> RewResult<()> {
        if let Some(ref mut watcher) = self.watcher {
            watcher.unwatch(path).map_err(|e| {
                RewError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to unwatch {:?}: {}", path, e),
                ))
            })?;
            let mut dirs = self.watched_dirs.lock().unwrap();
            dirs.retain(|d| d != path);
            info!("Stopped watching: {:?}", path);
            Ok(())
        } else {
            Err(RewError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Watcher not started",
            )))
        }
    }

    /// Stop watching and release resources.
    pub fn stop(&mut self) -> RewResult<()> {
        self.watcher = None;
        let mut dirs = self.watched_dirs.lock().unwrap();
        dirs.clear();
        info!("MacOSWatcher stopped");
        Ok(())
    }

    /// Returns the list of currently watched directories.
    pub fn watched_dirs(&self) -> Vec<PathBuf> {
        self.watched_dirs.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_creation() {
        let filter = PathFilter::default();
        let watcher = MacOSWatcher::new(filter);
        assert!(watcher.watcher.is_none());
        assert!(watcher.watched_dirs().is_empty());
    }
}
