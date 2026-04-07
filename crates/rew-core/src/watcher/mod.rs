//! File watcher module.
//!
//! Contains the path filter, platform-specific watcher implementations,
//! and the common watcher interface.

pub mod filter;
pub mod macos;

// Re-export for convenience
pub use filter::PathFilter;
pub use macos::MacOSWatcher;
