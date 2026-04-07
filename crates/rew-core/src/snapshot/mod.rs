//! APFS snapshot engine and related components.
//!
//! This module provides the macOS-specific snapshot creation, listing, and deletion
//! via the `tmutil` CLI, plus the high-level `MacOSSnapshotEngine` that integrates
//! with the SQLite metadata store.

pub mod macos;
pub mod tmutil;
