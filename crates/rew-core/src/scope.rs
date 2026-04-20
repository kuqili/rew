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
    ///
    /// Empty allow = allow everything except deny patterns.  This avoids false
    /// "outside allowed scope" blocks in multi-repo/monorepo setups where the
    /// AI session may edit files across sibling directories.
    pub fn default_rules(project_root: Option<PathBuf>) -> RewResult<Self> {
        let scope = RewScopeFile {
            allow: vec![],
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
    if let Some(reason) = check_dangerous_command(command) {
        return ScopeResult::Deny(format!(
            "{}: {}",
            reason,
            truncate_str(command, 80)
        ));
    }

    // Protect the rew data directory from any destructive command
    if is_targeting_rew_dir(command) {
        return ScopeResult::Deny(format!(
            "Modifying rew data directory is not allowed: {}",
            truncate_str(command, 80)
        ));
    }

        // Check user-defined alert rules against the command
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
        // rew data directory — snapshots, objects, config must never be touched by AI
        "~/.rew/**".to_string(),
        "/Users/*/.rew/**".to_string(),
        // SSH keys and credentials
        "~/.ssh/**".to_string(),
        "/Users/*/.ssh/**".to_string(),
        // AWS credentials
        "~/.aws/**".to_string(),
        "/Users/*/.aws/**".to_string(),
        // Environment files with secrets.
        // Only the bare .env and known production/staging variants are denied.
        // Template files (.env.example, .env.sample, .env.test) are allowed
        // because they are committed to source control and contain no real secrets.
        "**/.env".to_string(),
        "**/.env.local".to_string(),
        "**/.env.production".to_string(),
        "**/.env.staging".to_string(),
        // macOS system dirs
        "/System/**".to_string(),
        "/Library/**".to_string(),
    ]
}

/// Default alert rules.
///
/// Intentionally empty: all genuinely dangerous command patterns are already
/// handled with precision by `check_dangerous_command()` (catastrophic rm,
/// block-device writes, dd, mkfs, fork bomb).  Broad string-match alert rules
/// like "rm -rf" or "> /dev/" cause false positives on normal AI operations
/// (`rm -rf ./node_modules`, `echo foo > /dev/null`) and are removed here.
fn default_alert_rules() -> Vec<AlertRule> {
    vec![]
}

// ── rew directory protection ─────────────────────────────────────────────────

/// Returns true if the command string contains a path pointing at the rew
/// data directory (`~/.rew` or its absolute form).
///
/// This catches commands like:
///   rm -rf ~/.rew
///   rm -rf /Users/alice/.rew
///   mv ~/.rew /tmp/
fn is_targeting_rew_dir(command: &str) -> bool {
    // Destructive verbs that should never touch ~/.rew
    const DESTRUCTIVE: &[&str] = &["rm ", "rmdir ", "mv ", "shred ", "dd "];

    let is_destructive = DESTRUCTIVE.iter().any(|verb| {
        command.starts_with(verb)
            || command.contains(&format!("; {}", verb))
            || command.contains(&format!("&& {}", verb))
            || command.contains(&format!("|| {}", verb))
    });

    if !is_destructive {
        return false;
    }

    command.contains("/.rew") || command.contains("~/.rew")
}

// ── Dangerous command detection ──────────────────────────────────────────────

/// Check whether a shell command string matches known destructive patterns.
///
/// Returns `Some(reason)` if the command should be blocked, `None` if safe.
///
/// Design: we normalize the command (collapse whitespace, lowercase flags)
/// to catch common variations without pulling in a regex dependency.
///
/// Blocked categories:
/// 1. `rm` with recursive+force flags targeting root, home, or `/*`
/// 2. Writing directly to block devices (`/dev/sd*`, `/dev/nvme*`, etc.)
/// 3. `dd` writing to block devices (disk wipe)
/// 4. `mkfs.*` filesystem formatting
/// 5. `:(){ :|:& };:` fork bomb pattern
fn check_dangerous_command(command: &str) -> Option<&'static str> {
    // Normalize: collapse runs of whitespace for easier matching
    let cmd = command.trim();

    // ── 1. Destructive rm ────────────────────────────────────────────────────
    // Detect `rm` with any combination of -r/-R/--recursive AND -f/--force
    // targeting dangerous paths: /, ~, /*, ~/* etc.
    if is_destructive_rm(cmd) {
        return Some("Destructive rm blocked");
    }

    // ── 2. Writing to raw block devices ─────────────────────────────────────
    // Catches: echo foo > /dev/sda, cat file > /dev/nvme0n1
    for dev_prefix in &["/dev/sd", "/dev/hd", "/dev/nvme", "/dev/disk"] {
        if cmd.contains(dev_prefix) && (cmd.contains('>') || cmd.contains("dd ")) {
            return Some("Writing to block device blocked");
        }
    }

    // ── 3. dd writing to block devices ──────────────────────────────────────
    // Catches: dd if=... of=/dev/sda, dd of=/dev/nvme0n1
    if (cmd.starts_with("dd ") || cmd.contains("; dd ") || cmd.contains("&& dd "))
        && cmd.contains("of=/dev/")
    {
        return Some("dd to block device blocked");
    }

    // ── 4. mkfs — format a filesystem ───────────────────────────────────────
    if cmd.starts_with("mkfs") || cmd.contains(" mkfs") || cmd.contains(";mkfs") {
        return Some("Filesystem formatting command blocked");
    }

    // ── 5. Fork bomb ─────────────────────────────────────────────────────────
    if cmd.contains(":(){ :|:") || cmd.contains(":(){ : |:") {
        return Some("Fork bomb pattern blocked");
    }

    None
}

/// Returns true if the command is `rm` with recursive+force flags pointing at
/// a dangerous root-level or home-level path.
///
/// Splits the full command on common shell separators (`; && || |`) so that
/// chained invocations like `cd /tmp && rm -rf /usr` are also caught.
fn is_destructive_rm(cmd: &str) -> bool {
    // Split on common shell statement separators so we check each sub-command.
    for part in split_shell_commands(cmd) {
        let tokens: Vec<&str> = part.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        // First token must be "rm" (bare command, possibly prefixed by sudo)
        let first = tokens[0];
        let rest_start = if first == "rm" {
            1
        } else if first == "sudo" && tokens.get(1).copied() == Some("rm") {
            2
        } else {
            continue;
        };

        let mut has_recursive = false;
        let mut has_force = false;
        let mut dangerous_path = false;

        for token in &tokens[rest_start..] {
            if *token == "--" {
                // End-of-options marker; remaining tokens are paths
                continue;
            } else if token.starts_with("--") {
                if *token == "--recursive" || *token == "-R" { has_recursive = true; }
                if *token == "--force"                       { has_force = true; }
            } else if token.starts_with('-') {
                let flags = &token[1..];
                if flags.contains('r') || flags.contains('R') { has_recursive = true; }
                if flags.contains('f')                        { has_force = true; }
            } else {
                // Path argument — strip trailing slashes for uniform comparison,
                // but preserve "/" itself (trim would turn it into "").
                let path = if *token == "/" || *token == "//" {
                    "/"
                } else {
                    token.trim_end_matches('/')
                };
                if is_dangerous_rm_target(path) {
                    dangerous_path = true;
                }
            }
        }

        if has_recursive && has_force && dangerous_path {
            return true;
        }
    }
    false
}

/// Returns true for rm target paths that would cause catastrophic data loss.
fn is_dangerous_rm_target(path: &str) -> bool {
    // Literal root
    if path == "/" || path == "/*" {
        return true;
    }
    // Home directory or home glob: ~, ~/*, ~/Desktop/*, etc.
    if path == "~" || path == "~/*" || path.starts_with("~/") {
        return true;
    }
    // /home/<user> or /home/*
    if path.starts_with("/home/") {
        let after = &path["/home/".len()..];
        // /home/alice  or  /home/*  — one more component means user's home root
        if !after.contains('/') || after.starts_with("*/") {
            return true;
        }
    }
    // Single-level paths directly under root: /usr /etc /var /bin /lib …
    // These are recognisably system directories.
    if path.starts_with('/') {
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if components.len() == 1 {
            return true; // e.g. /usr /etc /tmp /var
        }
    }
    false
}

/// Split a shell command string on common statement separators.
/// Returns an iterator over individual command fragments.
fn split_shell_commands(cmd: &str) -> Vec<&str> {
    // We split on &&, ||, ;  (but not | alone to avoid false positives in paths)
    // This is intentionally conservative — we don't parse full shell syntax.
    let mut parts: Vec<&str> = vec![cmd];
    for sep in &["&&", "||", ";"] {
        let mut expanded = Vec::new();
        for part in &parts {
            for fragment in part.split(sep) {
                expanded.push(fragment.trim());
            }
        }
        parts = expanded;
    }
    parts
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

        // Bare .env always denied (contains real secrets)
        let result = engine.check_path(Path::new("/Users/alice/project/.env"));
        assert!(matches!(result, ScopeResult::Deny(_)));

        // Known sensitive variants denied
        let result2 = engine.check_path(Path::new("/Users/alice/project/.env.local"));
        assert!(matches!(result2, ScopeResult::Deny(_)));

        let result3 = engine.check_path(Path::new("/Users/alice/project/.env.production"));
        assert!(matches!(result3, ScopeResult::Deny(_)));

        let result4 = engine.check_path(Path::new("/Users/alice/project/.env.staging"));
        assert!(matches!(result4, ScopeResult::Deny(_)));

        // Template files committed to source control must be allowed
        let result5 = engine.check_path(Path::new("/Users/alice/project/.env.example"));
        assert_eq!(result5, ScopeResult::Allow, ".env.example should not be denied");

        let result6 = engine.check_path(Path::new("/Users/alice/project/.env.sample"));
        assert_eq!(result6, ScopeResult::Allow, ".env.sample should not be denied");

        let result7 = engine.check_path(Path::new("/Users/alice/project/.env.test"));
        assert_eq!(result7, ScopeResult::Allow, ".env.test should not be denied");
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

    // ── Dangerous rm variations ───────────────────────────────────────────────

    #[test]
    fn test_rm_classic_root() {
        assert!(check_dangerous_command("rm -rf /").is_some());
        assert!(check_dangerous_command("rm -rf /").is_some());
    }

    #[test]
    fn test_rm_reversed_flags() {
        // -fr instead of -rf
        assert!(check_dangerous_command("rm -fr /").is_some());
        assert!(check_dangerous_command("rm -fr ~/").is_some());
    }

    #[test]
    fn test_rm_long_flags() {
        assert!(check_dangerous_command("rm --recursive --force /").is_some());
        assert!(check_dangerous_command("rm --force --recursive ~/").is_some());
    }

    #[test]
    fn test_rm_split_flags() {
        // Flags split into separate tokens: rm -r -f /
        assert!(check_dangerous_command("rm -r -f /").is_some());
        assert!(check_dangerous_command("rm -f -r ~/").is_some());
    }

    #[test]
    fn test_rm_home_glob() {
        assert!(check_dangerous_command("rm -rf ~/*").is_some());
        assert!(check_dangerous_command("rm -rf ~/Desktop/*").is_some());
    }

    #[test]
    fn test_rm_root_subdir() {
        // rm -rf /usr is catastrophic even if not literally /
        assert!(check_dangerous_command("rm -rf /usr").is_some());
        assert!(check_dangerous_command("rm -rf /etc").is_some());
    }

    #[test]
    fn test_rm_safe_paths_allowed() {
        // Normal project paths must NOT be blocked
        assert!(check_dangerous_command("rm -rf ./tmp").is_none());
        assert!(check_dangerous_command("rm -rf /tmp/build").is_none());
        assert!(check_dangerous_command("rm -rf ./node_modules").is_none());
        assert!(check_dangerous_command("ls -la").is_none());
    }

    #[test]
    fn test_command_check_rm_rf() {
        let engine = ScopeEngine::default_rules(None).unwrap();

        let result = engine.check_command("rm -rf ~/Desktop/*");
        assert!(matches!(result, ScopeResult::Deny(_)));

        let result2 = engine.check_command("ls -la");
        assert_eq!(result2, ScopeResult::Allow);
    }

    // ── Block device writes ──────────────────────────────────────────────────

    #[test]
    fn test_command_check_dev_sda() {
        assert!(check_dangerous_command("echo foo > /dev/sda").is_some());
        assert!(check_dangerous_command("cat file.img > /dev/nvme0n1").is_some());
    }

    #[test]
    fn test_command_check_dd_wipe() {
        assert!(check_dangerous_command("dd if=/dev/zero of=/dev/sda").is_some());
        assert!(check_dangerous_command("dd if=/dev/urandom of=/dev/nvme0n1").is_some());
    }

    #[test]
    fn test_command_check_mkfs() {
        assert!(check_dangerous_command("mkfs.ext4 /dev/sda1").is_some());
        assert!(check_dangerous_command("mkfs -t vfat /dev/disk2").is_some());
    }

    #[test]
    fn test_command_check_fork_bomb() {
        assert!(check_dangerous_command(":(){ :|:& };:").is_some());
    }

    #[test]
    fn test_dev_null_write_allowed() {
        // Writing to /dev/null is harmless
        assert!(check_dangerous_command("echo foo > /dev/null").is_none());
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

    // ── rew directory protection ─────────────────────────────────────────────

    #[test]
    fn test_rew_dir_path_denied() {
        let engine = ScopeEngine::default_rules(None).unwrap();
        let home = dirs::home_dir().unwrap();

        let result = engine.check_path(&home.join(".rew/snapshots.db"));
        assert!(matches!(result, ScopeResult::Deny(_)), "~/.rew/snapshots.db should be denied");

        let result2 = engine.check_path(&home.join(".rew/objects/ab/cdef"));
        assert!(matches!(result2, ScopeResult::Deny(_)), "~/.rew/objects/** should be denied");
    }

    #[test]
    fn test_rew_dir_rm_command_denied() {
        let engine = ScopeEngine::default_rules(None).unwrap();

        let r1 = engine.check_command("rm -rf ~/.rew");
        assert!(matches!(r1, ScopeResult::Deny(_)), "rm -rf ~/.rew should be denied");

        let r2 = engine.check_command("rm -rf /Users/alice/.rew");
        assert!(matches!(r2, ScopeResult::Deny(_)), "rm -rf /Users/alice/.rew should be denied");

        let r3 = engine.check_command("mv ~/.rew /tmp/rew-backup");
        assert!(matches!(r3, ScopeResult::Deny(_)), "mv ~/.rew ... should be denied");
    }

    #[test]
    fn test_rew_dir_read_allowed_at_command_level() {
        // cat/grep on ~/.rew is not a destructive verb, command check should pass
        // (path-level check would still deny if the hook checks the path separately)
        let engine = ScopeEngine::default_rules(None).unwrap();
        let result = engine.check_command("cat ~/.rew/snapshots.db");
        assert_eq!(result, ScopeResult::Allow);
    }

    // ── Regression tests: false-positive interceptions ───────────────────────
    // These tests document current mis-blocking behaviour and are expected to
    // FAIL before the fixes are applied, then PASS afterwards.

    #[test]
    fn regression_rm_rf_safe_paths_allowed_via_engine() {
        // rm -rf on project build artefacts is normal AI behaviour.
        // check_dangerous_command() already allows these; the default_alert_rules
        // "rm -rf" string match is the only thing blocking them.
        let engine = ScopeEngine::default_rules(None).unwrap();

        let r1 = engine.check_command("rm -rf ./node_modules");
        assert_eq!(r1, ScopeResult::Allow, "rm -rf ./node_modules should be allowed");

        let r2 = engine.check_command("rm -rf ./dist");
        assert_eq!(r2, ScopeResult::Allow, "rm -rf ./dist should be allowed");

        let r3 = engine.check_command("rm -rf /tmp/build");
        assert_eq!(r3, ScopeResult::Allow, "rm -rf /tmp/build should be allowed");
    }

    #[test]
    fn regression_dev_null_allowed_via_engine() {
        // Writing to /dev/null is harmless.  check_dangerous_command() explicitly
        // allows it, but the default_alert_rules "> /dev/" string match blocks it.
        let engine = ScopeEngine::default_rules(None).unwrap();

        let result = engine.check_command("echo foo > /dev/null");
        assert_eq!(result, ScopeResult::Allow, "echo > /dev/null should be allowed");

        let result2 = engine.check_command("cat file.txt > /dev/null");
        assert_eq!(result2, ScopeResult::Allow, "cat > /dev/null should be allowed");
    }

    #[test]
    fn regression_env_example_not_denied() {
        // .env.example is a template file committed to source control.
        // The **/.env.* glob incorrectly matches it.
        let engine = ScopeEngine::default_rules(None).unwrap();

        let result = engine.check_path(Path::new("/Users/alice/project/.env.example"));
        assert_eq!(result, ScopeResult::Allow, ".env.example should be allowed");

        let result2 = engine.check_path(Path::new("/Users/alice/project/.env.sample"));
        assert_eq!(result2, ScopeResult::Allow, ".env.sample should be allowed");

        let result3 = engine.check_path(Path::new("/Users/alice/project/.env.test"));
        assert_eq!(result3, ScopeResult::Allow, ".env.test should be allowed");
    }

    #[test]
    fn regression_default_rules_allow_cross_directory() {
        // When no .rewscope exists, default_rules() should not block files that
        // are outside the inferred project_root.  Multi-repo setups (e.g. a
        // monorepo with web/ and aiproxy/ siblings) must not be mis-blocked.
        let root = std::path::PathBuf::from("/Users/alice/code/lawyer-helper/lawyer-helper-web");
        let engine = ScopeEngine::default_rules(Some(root)).unwrap();

        // File in a sibling directory — previously blocked as "outside allowed scope"
        let result = engine.check_path(Path::new(
            "/Users/alice/code/lawyer-helper/lawyer-helper-aiproxy/app/services/backend_client.py",
        ));
        assert_eq!(result, ScopeResult::Allow, "sibling directory file should be allowed by default rules");
    }

    #[test]
    fn regression_rm_rf_dangerous_still_blocked() {
        // Sanity check: genuinely dangerous rm commands must remain blocked
        // even after the alert-rule removal.  This test should pass both before
        // and after the fix.
        let engine = ScopeEngine::default_rules(None).unwrap();

        let r1 = engine.check_command("rm -rf ~/Desktop/*");
        assert!(matches!(r1, ScopeResult::Deny(_)), "rm -rf ~/Desktop/* must still be denied");

        let r2 = engine.check_command("rm -rf /");
        assert!(matches!(r2, ScopeResult::Deny(_)), "rm -rf / must still be denied");

        let r3 = engine.check_command("rm -rf /usr");
        assert!(matches!(r3, ScopeResult::Deny(_)), "rm -rf /usr must still be denied");
    }
}
