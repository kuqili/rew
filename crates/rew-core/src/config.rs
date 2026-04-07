//! Configuration management for rew.
//!
//! Reads/writes ~/.rew/config.toml with watch directories, ignore patterns,
//! anomaly rules, and retention policy.

use crate::error::RewResult;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level rew configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewConfig {
    /// Directories to watch for file changes
    pub watch_dirs: Vec<PathBuf>,

    /// File/directory patterns to ignore (glob syntax)
    pub ignore_patterns: Vec<String>,

    /// Anomaly detection rule thresholds
    pub anomaly_rules: AnomalyRulesConfig,

    /// Snapshot retention policy
    pub retention_policy: RetentionPolicyConfig,
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

impl Default for RewConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        Self {
            watch_dirs: vec![
                home.join("Desktop"),
                home.join("Documents"),
                home.join("Downloads"),
            ],
            ignore_patterns: vec![
                "**/node_modules/**".to_string(),
                "**/.git/**".to_string(),
                "**/target/**".to_string(),
                "**/__pycache__/**".to_string(),
                "**/.DS_Store".to_string(),
                "**/Thumbs.db".to_string(),
                "**/*.swp".to_string(),
                "**/*~".to_string(),
            ],
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
    pub fn load(path: &Path) -> RewResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: RewConfig = toml::from_str(&content)?;
        Ok(config)
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
