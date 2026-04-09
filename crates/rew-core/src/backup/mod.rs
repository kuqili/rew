//! User-level file backup engine.
//!
//! Creates backups of files changed during events, storing them in a mirrored
//! directory structure at ~/.rew/backups/{snapshot_id}/.
//!
//! This is distinct from OS-level snapshots: it actively copies file contents
//! before they're modified or deleted, allowing restores even if OS snapshots aren't available.

pub mod clonefile;
pub mod engine;
pub mod copy_strategy;

pub use engine::{BackupEngine, BackupJob, BackupResult};
pub use copy_strategy::FileCopyStrategy;
pub use clonefile::{clone_or_copy, backup_file_to, CopyMethod};
