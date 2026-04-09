//! Scope rules engine for AI operation boundary enforcement.
//!
//! Parses `.rewscope` YAML files to define:
//! - `allow`: paths where AI can operate freely
//! - `deny`: paths that are blocked (PreToolUse exit 2)
//! - `alert`: patterns that trigger warnings
//!
//! Used by:
//! - `rew hook pre-tool`: check before AI writes/edits/runs commands
//! - RULE-09: real-time scope violation detection

use crate::error::{RewError, RewResult};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Result of a scope check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeResult {
    /// Path is within allowed scope
    Allow,
    /// Path is explicitly denied — block the operation
    Deny(String),
    /// Path triggers an alert but is not blocked
    Alert(String),
}

/// Alert rule: pattern + action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    /// Glob pattern or command pattern
    pub pattern: String,
    /// Action: "block" or "confirm"
    pub action: String,
}

/// Raw .rewscope file structure (deserialized from YAML).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RewScopeFile {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub alert: Vec<AlertRule>,
}

/// Compiled scope rules engine.
pub struct ScopeEngine {
    allow_set: GlobSet,
    deny_set: GlobSet,
    deny_patterns: Vec<String>,
    alert_rules: Vec<(GlobSet, AlertRule)>,
    /// The project root (allow rules are relative to this)
    project_root: Option<PathBuf>,
}

impl ScopeEngine {
    /// Create a ScopeEngine from a .rewscope file path.
    pub fn from_file(path: &Path) -> RewResult<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            RewError::Config(format!("Failed to read .rewscope: {}", e))
        })?;
        let scope: RewScopeFile = serde_yaml::from_str(&content).map_err(|e| {
            RewError::Config(format!("Failed to parse .rewscope YAML: {}", e))
        })?;
        let project_root = path.parent().map(|p| p.to_path_buf());
        Self::from_config(scope, project_root)
    }

    /// Create a ScopeEngine with default rules (no .rewscope file).
    pub fn default_rules(project_root: Option<PathBuf>) -> RewResult<Self> {
        let scope = RewScopeFile {
            allow: vec!["./**".to_string()],
            deny: default_deny_patterns(),
            alert: default_alert_rules(),
        };
        Self::from_config(scope, project_root)
    }

    /// Create from a config struct.
    pub fn from_config(scope: RewScopeFile, project_root: Option<PathBuf>) -> RewResult<Self> {
        let allow_set = build_globset(&scope.allow)?;

        // Combine user deny patterns with built-in defaults
        let mut all_deny = scope.deny.clone();
        for default in default_deny_patterns() {
            if !all_deny.contains(&default) {
                all_deny.push(default);
            }
        }
        let deny_set = build_globset(&all_deny)?;

        let mut alert_rules = Vec::new();
        for rule in &scope.alert {
            let set = build_globset(&[rule.pattern.clone()])?;
            alert_rules.push((set, rule.clone()));
        }

        Ok(ScopeEngine {
            allow_set,
            deny_set,
            deny_patterns: all_deny,
            alert_rules,
            project_root,
        })
    }

    /// Check if a file path is within the allowed scope.
    ///
    /// Returns:
    /// - `Deny(reason)` if path matches any deny pattern → block
    /// - `Alert(reason)` if path matches an alert rule → warn but allow
    /// - `Allow` if path is within allowed scope
    /// - `Deny(reason)` if path is not within any allow pattern
    pub fn check_path(&self, path: &Path) -> ScopeResult {
        let expanded = expand_tilde(path);
        let check_path = &expanded;

        // 1. Check deny rules first (highest priority)
        if self.deny_set.is_match(check_path) {
            let matched = self.deny_patterns.iter()
                .find(|p| {
                    build_globset(&[p.to_string()])
                        .map(|gs| gs.is_match(check_path))
                        .unwrap_or(false)
                })
                .cloned()
                .unwrap_or_else(|| "deny rule".to_string());
            return ScopeResult::Deny(format!(
                "Path '{}' matches deny rule: {}",
                check_path.display(),
                matched
            ));
        }

        // 2. Check alert rules
        for (set, rule) in &self.alert_rules {
            if set.is_match(check_path) {
                return ScopeResult::Alert(format!(
                    "Path '{}' matches alert pattern: {} (action: {})",
                    check_path.display(),
                    rule.pattern,
                    rule.action
                ));
            }
        }

        // 3. Check allow rules
        if !self.allow_set.is_empty() {
            // If we have a project root, check relative to it
            if let Some(root) = &self.project_root {
                if check_path.starts_with(root) {
                    return ScopeResult::Allow;
                }
            }

            // Check against allow patterns directly
            if self.allow_set.is_match(check_path) {
                return ScopeResult::Allow;
            }

            // Path is outside all allow patterns
            return ScopeResult::Deny(format!(
                "Path '{}' is outside allowed scope",
                check_path.display()
            ));
        }

        // No allow rules defined = everything allowed (except deny)
        ScopeResult::Allow
    }

    /// Check a command string against alert rules.
    /// Used for Bash commands in PreToolUse.
    pub fn check_command(&self, command: &str) -> ScopeResult {
        // Built-in dangerous command patterns
        if command.contains("rm -rf /") || command.contains("rm -rf ~/") {
            return ScopeResult::Deny(format!(
                "Dangerous command blocked: {}",
                truncate_str(command, 80)
            ));
        }

        if command.contains("> /dev/") {
            return ScopeResult::Deny(format!(
                "Writing to /dev/ blocked: {}",
                truncate_str(command, 80)
            ));
        }

        // Check alert rules against the command
        for (_, rule) in &self.alert_rules {
            if command.contains(&rule.pattern) && rule.action == "block" {
                return ScopeResult::Deny(format!(
                    "Command matches blocked pattern '{}': {}",
                    rule.pattern,
                    truncate_str(command, 80)
                ));
            }
        }

        ScopeResult::Allow
    }

    /// Get the deny patterns (for debugging/display).
    pub fn deny_patterns(&self) -> &[String] {
        &self.deny_patterns
    }
}

/// Default deny patterns — always active even without .rewscope file.
fn default_deny_patterns() -> Vec<String> {
    vec![
        // SSH keys and credentials
        "~/.ssh/**".to_string(),
        "/Users/*/.ssh/**".to_string(),
        // AWS credentials
        "~/.aws/**".to_string(),
        "/Users/*/.aws/**".to_string(),
        // Environment files with secrets
        "**/.env".to_string(),
        "**/.env.*".to_string(),
        // macOS system dirs
        "/System/**".to_string(),
        "/Library/**".to_string(),
    ]
}

/// Default alert rules.
fn default_alert_rules() -> Vec<AlertRule> {
    vec![
        AlertRule {
            pattern: "rm -rf".to_string(),
            action: "block".to_string(),
        },
        AlertRule {
            pattern: "> /dev/".to_string(),
            action: "block".to_string(),
        },
    ]
}

/// Expand ~ to the user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if path_str.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path_str[2..]);
        }
    }
    path.to_path_buf()
}

fn build_globset(patterns: &[String]) -> RewResult<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        // Expand ~ in patterns before compiling
        let expanded = if p.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                format!("{}/{}", home.display(), &p[2..])
            } else {
                p.clone()
            }
        } else {
            p.clone()
        };
        let glob = Glob::new(&expanded).map_err(|e| {
            RewError::Config(format!("Invalid scope glob '{}': {}", p, e))
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|e| {
        RewError::Config(format!("Failed to build scope globset: {}", e))
    })
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..max_len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_deny_ssh() {
        let engine = ScopeEngine::default_rules(None).unwrap();
        let home = dirs::home_dir().unwrap();

        let result = engine.check_path(&home.join(".ssh/id_rsa"));
        assert!(matches!(result, ScopeResult::Deny(_)));
    }

    #[test]
    fn test_default_deny_aws() {
        let engine = ScopeEngine::default_rules(None).unwrap();
        let home = dirs::home_dir().unwrap();

        let result = engine.check_path(&home.join(".aws/credentials"));
        assert!(matches!(result, ScopeResult::Deny(_)));
    }

    #[test]
    fn test_default_deny_env() {
        let engine = ScopeEngine::default_rules(None).unwrap();

        let result = engine.check_path(Path::new("/Users/alice/project/.env"));
        assert!(matches!(result, ScopeResult::Deny(_)));

        let result2 = engine.check_path(Path::new("/Users/alice/project/.env.local"));
        assert!(matches!(result2, ScopeResult::Deny(_)));
    }

    #[test]
    fn test_allow_normal_project_files() {
        let dir = tempdir().unwrap();
        let engine = ScopeEngine::default_rules(Some(dir.path().to_path_buf())).unwrap();

        let result = engine.check_path(&dir.path().join("src/main.rs"));
        assert_eq!(result, ScopeResult::Allow);
    }

    #[test]
    fn test_custom_deny_from_yaml() {
        let dir = tempdir().unwrap();
        let scope_file = dir.path().join(".rewscope");
        std::fs::write(
            &scope_file,
            r#"
allow:
  - "./**"
deny:
  - "~/Desktop/**"
  - "~/Documents/**"
alert: []
"#,
        )
        .unwrap();

        let engine = ScopeEngine::from_file(&scope_file).unwrap();
        let home = dirs::home_dir().unwrap();

        let result = engine.check_path(&home.join("Desktop/important.docx"));
        assert!(matches!(result, ScopeResult::Deny(_)));

        let result2 = engine.check_path(&home.join("Documents/report.pdf"));
        assert!(matches!(result2, ScopeResult::Deny(_)));
    }

    #[test]
    fn test_command_check_rm_rf() {
        let engine = ScopeEngine::default_rules(None).unwrap();

        let result = engine.check_command("rm -rf ~/Desktop/*");
        assert!(matches!(result, ScopeResult::Deny(_)));

        let result2 = engine.check_command("ls -la");
        assert_eq!(result2, ScopeResult::Allow);
    }

    #[test]
    fn test_command_check_dev_null() {
        let engine = ScopeEngine::default_rules(None).unwrap();

        let result = engine.check_command("echo foo > /dev/sda");
        assert!(matches!(result, ScopeResult::Deny(_)));
    }

    #[test]
    fn test_system_dirs_denied() {
        let engine = ScopeEngine::default_rules(None).unwrap();

        let result = engine.check_path(Path::new("/System/Library/Frameworks/foo"));
        assert!(matches!(result, ScopeResult::Deny(_)));

        let result2 = engine.check_path(Path::new("/Library/Preferences/com.apple.foo"));
        assert!(matches!(result2, ScopeResult::Deny(_)));
    }

    #[test]
    fn test_empty_scope_allows_all() {
        let scope = RewScopeFile {
            allow: vec![],
            deny: vec![],
            alert: vec![],
        };
        // Still has built-in denies
        let engine = ScopeEngine::from_config(scope, None).unwrap();

        // Normal paths allowed (no allow rules = allow all except deny)
        let result = engine.check_path(Path::new("/tmp/test.txt"));
        assert_eq!(result, ScopeResult::Allow);
    }
}
