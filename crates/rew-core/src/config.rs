//! Configuration management for rew.
//!
//! Reads/writes ~/.rew/config.toml with watch directories, ignore patterns,
//! anomaly rules, and retention policy.

use crate::error::RewResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level rew configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewConfig {
    /// Directories to watch for file changes
    pub watch_dirs: Vec<PathBuf>,

    /// File/directory patterns to ignore (glob syntax)
    pub ignore_patterns: Vec<String>,

    /// Maximum file size to scan (bytes). Files larger are skipped.
    /// Default: 100MB. None = no limit.
    pub max_file_size_bytes: Option<u64>,

    /// How many seconds of file-monitoring events to group into one timeline
    /// checkpoint. Events within the same window are merged into a single
    /// "监听窗口" task so the timeline stays readable.
    /// Default: 600 (10 minutes). Valid range: 60–7200.
    #[serde(default = "RewConfig::default_monitoring_window_secs")]
    pub monitoring_window_secs: u64,

    /// Per-directory ignore rules. Key = watch dir absolute path.
    #[serde(default)]
    pub dir_ignore: HashMap<String, DirIgnoreConfig>,

    /// Anomaly detection rule thresholds
    pub anomaly_rules: AnomalyRulesConfig,

    /// Snapshot retention policy
    pub retention_policy: RetentionPolicyConfig,
}

/// Per-directory ignore configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DirIgnoreConfig {
    /// Sub-directory names to exclude (e.g., "build", "dist", ".cache")
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
    /// File extensions to exclude, without dot (e.g., "log", "tmp", "bak")
    #[serde(default)]
    pub exclude_extensions: Vec<String>,
}

/// Thresholds for anomaly detection rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyRulesConfig {
    /// RULE-01: Bulk delete threshold (HIGH)
    pub bulk_delete_high: u32,

    /// RULE-02: Small bulk delete threshold (MEDIUM)
    pub bulk_delete_medium: u32,

    /// RULE-03: Total deletion size threshold in bytes (HIGH)
    pub delete_size_high_bytes: u64,

    /// RULE-04: Bulk modify threshold (MEDIUM)
    pub bulk_modify_medium: u32,

    /// RULE-07: Non-package modify threshold (MEDIUM)
    pub non_package_modify_medium: u32,

    /// Time window for event aggregation in seconds
    pub window_secs: u32,

    /// Sensitive files that trigger RULE-06
    pub sensitive_files: Vec<String>,
}

/// Snapshot retention policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicyConfig {
    /// Keep all snapshots within this many seconds (default: 3600 = 1 hour)
    pub keep_all_secs: u64,

    /// Keep hourly snapshots within this many seconds (default: 86400 = 24 hours)
    pub keep_hourly_secs: u64,

    /// Keep daily snapshots within this many seconds (default: 2592000 = 30 days)
    pub keep_daily_secs: u64,

    /// Anomaly-triggered snapshots are retained this multiplier longer
    pub anomaly_retention_multiplier: u32,
}

impl RewConfig {
    /// The canonical set of default ignore patterns.
    ///
    /// Kept as a standalone function so both `Default` and `load()` can use it
    /// to ensure existing configs on disk are always up-to-date.
    pub fn default_ignore_patterns() -> Vec<String> {
        vec![
            // OS/editor temporaries
            "**/.DS_Store".to_string(),
            "**/Thumbs.db".to_string(),
            "**/*.swp".to_string(),
            "**/*~".to_string(),
            "**/.#*".to_string(),       // Emacs lock files
            "**/*.tmp".to_string(),
            "**/*.temp".to_string(),
            // macOS safe-save (atomic write) temp files.
            // Apps write to a ".sb-XXXXXXXX-YYYYYY" file, atomically swap it
            // with the original, then delete the temp. These intermediate events
            // are noise and should never be recorded or restored.
            "**/*.sb-*".to_string(),
            // Applications (not user data)
            "**/*.app/**".to_string(),
            // Installers & disk images (can re-download)
            "**/*.dmg".to_string(),
            "**/*.pkg".to_string(),
            "**/*.iso".to_string(),
            "**/*.msi".to_string(),
            "**/*.exe".to_string(),
            // Development artifacts (auto-generated, not user data)
            "**/node_modules/**".to_string(),
            "**/.git/**".to_string(),
            "**/target/**".to_string(),
            "**/__pycache__/**".to_string(),
        ]
    }

    pub fn default_monitoring_window_secs() -> u64 {
        600
    }

    /// Merge any missing default patterns into this config.
    ///
    /// Called after loading a config from disk so that configs created before
    /// a new default pattern was added automatically receive the update.
    pub fn ensure_default_patterns(&mut self) {
        for pattern in Self::default_ignore_patterns() {
            if !self.ignore_patterns.contains(&pattern) {
                self.ignore_patterns.push(pattern);
            }
        }
    }
}

impl Default for RewConfig {
    fn default() -> Self {
        Self {
            watch_dirs: vec![],
            ignore_patterns: Self::default_ignore_patterns(),
            max_file_size_bytes: None,
            monitoring_window_secs: Self::default_monitoring_window_secs(),
            dir_ignore: HashMap::new(),
            anomaly_rules: AnomalyRulesConfig::default(),
            retention_policy: RetentionPolicyConfig::default(),
        }
    }
}

impl Default for AnomalyRulesConfig {
    fn default() -> Self {
        Self {
            bulk_delete_high: 20,
            bulk_delete_medium: 5,
            delete_size_high_bytes: 100 * 1024 * 1024, // 100MB
            bulk_modify_medium: 50,
            non_package_modify_medium: 30,
            window_secs: 30,
            sensitive_files: vec![
                ".env".to_string(),
                ".env.local".to_string(),
                ".gitconfig".to_string(),
                ".ssh/config".to_string(),
                ".npmrc".to_string(),
                ".pypirc".to_string(),
            ],
        }
    }
}

impl Default for RetentionPolicyConfig {
    fn default() -> Self {
        Self {
            keep_all_secs: 3600,          // 1 hour
            keep_hourly_secs: 86400,      // 24 hours
            keep_daily_secs: 2592000,     // 30 days
            anomaly_retention_multiplier: 2,
        }
    }
}

impl RewConfig {
    /// Load configuration from a TOML file.
    ///
    /// After loading, missing default patterns are automatically merged in so
    /// that existing configs on disk stay compatible when new defaults are added.
    /// Also deduplicates watch_dirs to remove redundant sub-directories.
    pub fn load(path: &Path) -> RewResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: RewConfig = toml::from_str(&content)?;
        config.ensure_default_patterns();
        config.dedup_watch_dirs();
        Ok(config)
    }

    /// Remove redundant sub-directories from watch_dirs.
    ///
    /// If directory A is an ancestor of directory B, B is removed because A
    /// already covers it. For example, [~/Desktop, ~/Desktop/project] → [~/Desktop].
    pub fn dedup_watch_dirs(&mut self) {
        // Sort by depth (shorter paths first) so parents are processed before children
        self.watch_dirs.sort_by_key(|p| p.components().count());
        let mut kept: Vec<std::path::PathBuf> = Vec::new();
        for candidate in self.watch_dirs.drain(..) {
            // If any already-kept dir is a prefix of candidate, candidate is redundant
            let is_covered = kept.iter().any(|k| candidate.starts_with(k));
            if !is_covered {
                kept.push(candidate);
            }
        }
        self.watch_dirs = kept;
    }

    /// Save configuration to a TOML file.
    pub fn save(&self, path: &Path) -> RewResult<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::error::RewError::Serialization(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_config_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = RewConfig::default();
        config.save(&path).unwrap();

        let loaded = RewConfig::load(&path).unwrap();
        assert_eq!(loaded.watch_dirs.len(), 3);
        assert_eq!(loaded.anomaly_rules.bulk_delete_high, 20);
        assert_eq!(loaded.retention_policy.keep_all_secs, 3600);
    }
}
