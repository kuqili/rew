//! rew-core: Core library for rew — AI 时代的文件安全网
//!
//! This crate defines the shared traits, types, configuration, database,
//! and error handling used by both rew-tauri and rew-cli.

pub mod backup;
pub mod baseline;
pub mod config;
pub mod diff;
pub mod db;
pub mod detector;
pub mod error;
pub mod file_index;
pub mod hook_events;
pub mod hooks;
pub mod lifecycle;
pub mod logging;
pub mod notifier;
pub mod objects;
pub mod pipeline;
pub mod processor;
pub mod reconcile;
pub mod restore;
pub mod scanner;
pub mod scope;
pub mod snapshot;
pub mod storage;
pub mod traits;
pub mod types;
pub mod watcher;

use std::path::PathBuf;

/// Returns the rew home directory (~/.rew/)
pub fn rew_home_dir() -> PathBuf {
    let home = dirs::home_dir().expect("Could not determine home directory");
    home.join(".rew")
}

/// Returns the stable path for the rew CLI binary (~/.rew/bin/rew).
/// All hook configurations point to this path so they survive app updates.
pub fn rew_cli_bin_path() -> PathBuf {
    rew_home_dir().join("bin").join("rew")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rew_home_dir() {
        let dir = rew_home_dir();
        assert!(dir.ends_with(".rew"));
    }
}
