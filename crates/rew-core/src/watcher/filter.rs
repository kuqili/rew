//! Path filter for ignoring noise files/directories.
//!
//! Uses `globset` for efficient glob matching against a list of ignore patterns.
//!
//! ## HOME hidden-directory strategy
//!
//! When watching `~`, most `~/.xxx/` directories are tool caches / runtime
//! data (`.cargo`, `.npm`, `.cursor`, `.claude`, …) that change constantly and
//! are not user content worth protecting.
//!
//! Strategy (applied in `should_ignore`):
//!   - Hidden **directories** directly under HOME → excluded by default.
//!   - Exception: a hard-coded whitelist of dirs that contain credentials or
//!     important user config (`.ssh`, `.gnupg`, `.aws`, `.kube`, …).
//!   - Hidden **files** directly under HOME (`.zshrc`, `.gitconfig`, …) are
//!     NOT affected — they are always monitored regardless of this rule.
//!   - Hidden dirs anywhere deeper in the tree (e.g. `project/.git/`) are
//!     handled by the existing per-name fast-path and glob patterns.

use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};

/// Hidden directories directly under HOME that contain user credentials or
/// important configuration and must NOT be excluded by the broad hidden-dir
/// rule below. Everything else under `~/.xxx/` is treated as tool data.
const HOME_HIDDEN_DIR_WHITELIST: &[&str] = &[
    ".ssh",    // SSH keys / authorized_keys
    ".gnupg",  // GPG keys
    ".gpg",    // alternate GPG dir
    ".aws",    // AWS credentials & config
    ".kube",   // Kubernetes kubeconfig
    ".azure",  // Azure CLI credentials
    ".gcloud", // Google Cloud credentials
    ".config", // XDG config (contains many important per-app configs)
    // Shell histories / sessions are FILES not dirs, so they are unaffected.
];

/// A path filter that determines whether a file event should be ignored.
#[derive(Debug, Clone)]
pub struct PathFilter {
    /// Compiled glob set for matching ignore patterns
    ignore_set: GlobSet,
    /// Original patterns (for debugging/display)
    patterns: Vec<String>,
    /// Resolved HOME directory for the hidden-dir rule (None = rule disabled)
    home_dir: Option<PathBuf>,
}

impl PathFilter {
    /// Create a PathFilter from raw patterns (no merging with defaults).
    fn from_patterns(patterns: &[String]) -> Result<Self, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }
        let home_dir = std::env::var("HOME").ok().map(PathBuf::from);
        Ok(Self {
            ignore_set: builder.build()?,
            patterns: patterns.to_vec(),
            home_dir,
        })
    }

    /// Create a new PathFilter from user-supplied patterns.
    ///
    /// The built-in default patterns (build dirs, OS noise, etc.) are always
    /// included; `patterns` are merged on top so user config can add extras
    /// but never accidentally lose the baseline coverage.
    pub fn new(patterns: &[String]) -> Result<Self, globset::Error> {
        let mut merged = Self::builtin_patterns();
        for p in patterns {
            if !merged.contains(p) {
                merged.push(p.clone());
            }
        }
        Self::from_patterns(&merged)
    }

    /// Built-in patterns that are always active.
    ///
    /// This is the **single source of truth** for all default ignore patterns.
    /// `RewConfig::default_ignore_patterns()` delegates here.
    ///
    /// Note: hidden directories directly under HOME are handled by a runtime
    /// whitelist rule in `should_ignore()`, not by glob patterns, so they
    /// don't need to be listed exhaustively here.
    pub fn builtin_patterns() -> Vec<String> {
        let home = std::env::var("HOME").unwrap_or_default();
        vec![
            // ── rew's own data (MUST be first to avoid recursive storms) ──
            format!("{}/.rew/**", home),
            // ── macOS system / app directories (noisy + permission errors) ─
            format!("{}/Library/**", home),
            format!("{}/Applications/**", home),
            format!("{}/.Trash/**", home),
            // ── Shell history & caches (change on every command / login) ───
            format!("{}/.zsh_history", home),
            format!("{}/.zsh_history.*", home),
            format!("{}/.bash_history", home),
            format!("{}/.sh_history", home),
            format!("{}/.fish_history", home),
            format!("{}/.zcompdump*", home),   // zsh completion dump
            format!("{}/.zsh_sessions/**", home),
            format!("{}/.lesshst", home),
            format!("{}/.viminfo", home),
            format!("{}/.python_history", home),
            format!("{}/.node_repl_history", home),
            // ── z / zoxide jump history ─────────────────────────────────────
            format!("{}/.z", home),
            format!("{}/.z.*", home),
            format!("{}/.zlua", home),
            // ── CLI tool update-notifier caches ──────────────────────────────
            format!("{}/.config/configstore/**", home),
            // ── Log files (always regenerated, never user content) ───────────
            "**/log/*.log".to_string(),
            "**/logs/*.log".to_string(),
            "**/*.log.*".to_string(),          // rotated: app.log.1, app.log.20260411
            // ── App bundles / disk images ──────────────────────────────────
            "**/*.app/**".to_string(),
            "**/*.dmg".to_string(),
            "**/*.pkg".to_string(),
            "**/*.iso".to_string(),
            // ── Version control ────────────────────────────────────────────
            "**/.git/**".to_string(),
            "**/.svn/**".to_string(),
            "**/.hg/**".to_string(),
            // ── Language / runtime build caches ────────────────────────────
            "**/node_modules/**".to_string(),
            "**/target/**".to_string(),
            "**/__pycache__/**".to_string(),
            "**/*.pyc".to_string(),
            "**/*.pyo".to_string(),
            "**/.venv/**".to_string(),
            "**/venv/**".to_string(),
            "**/.tox/**".to_string(),
            "**/.gradle/**".to_string(),
            "**/.m2/**".to_string(),
            "**/vendor/**".to_string(),
            // ── Frontend / bundler output ───────────────────────────────────
            "**/.next/**".to_string(),
            "**/.nuxt/**".to_string(),
            "**/.output/**".to_string(),
            "**/.cache/**".to_string(),
            "**/dist/**".to_string(),
            "**/build/**".to_string(),
            "**/out/**".to_string(),
            "**/.parcel-cache/**".to_string(),
            "**/.turbo/**".to_string(),
            // ── Compiled binary artifacts ───────────────────────────────────
            "**/*.class".to_string(),
            "**/*.o".to_string(),
            "**/*.a".to_string(),
            "**/*.so".to_string(),
            "**/*.dylib".to_string(),
            "**/*.dll".to_string(),
            "**/*.exe".to_string(),
            // ── OS / editor noise ───────────────────────────────────────────
            "**/.DS_Store".to_string(),
            "**/Thumbs.db".to_string(),
            "**/*.swp".to_string(),
            "**/*~".to_string(),
            "**/*.sb-*".to_string(),
            "**/.#*".to_string(),
            "**/*.tmp".to_string(),
            "**/*.temp".to_string(),
            // ── Database journal / WAL files ─────────────────────────────────
            "**/*.db-journal".to_string(),
            "**/*.db-wal".to_string(),
            "**/*.db-shm".to_string(),
        ]
    }

    /// Returns `true` if the given path should be ignored (matches any pattern).
    pub fn should_ignore(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        // ── Fast path: filename-level checks ──────────────────────────────
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            // macOS safe-save (atomic write) temp files: "original.sb-XXXXXXXX-YYYYYY"
            if name.contains(".sb-") {
                return true;
            }
            // Generic temp extensions
            if name.ends_with(".tmp") || name.ends_with(".temp") {
                return true;
            }
            // Claude Code staging: "foo.rs.tmp.73919" (dot-separated)
            if name.contains(".tmp.") {
                return true;
            }
            // Atomic-write temps with hex suffix: "foo.json.tmp-5885abc"
            // Used by Claude Code, npm, many CLIs.
            if let Some(idx) = name.find(".tmp-") {
                if idx > 0 {
                    return true;
                }
            }
            // Transient process-lock files: distinguish from package-manager
            // lock files (Cargo.lock, yarn.lock) which are important user content.
            //
            // Heuristic:
            //   • Hidden file + .lock  → transient  (.claude.json.lock)
            //   • Uppercase .LOCK      → transient  (.zsh_history.LOCK)
            //   • Stem has an extension → transient  (foo.json.lock, db.sqlite.lock)
            //   • Bare name + .lock    → keep        (Cargo.lock, yarn.lock)
            if name.ends_with(".LOCK") {
                return true; // e.g. .zsh_history.LOCK — always transient
            }
            if name.ends_with(".lock") {
                if name.starts_with('.') {
                    return true; // hidden file lock → transient
                }
                // Check if stem (name minus ".lock") has its own extension
                let stem = &name[..name.len() - 5];
                if Path::new(stem).extension().is_some() {
                    return true; // e.g. foo.json.lock → transient
                }
                // Bare name like "Cargo" or "yarn" → package-manager lock → keep
            }
            // SQLite WAL / journal files
            if name.ends_with("-journal") || name.ends_with("-wal") || name.ends_with("-shm") {
                return true;
            }
            // Vim swap files
            if name.ends_with(".swo") {
                return true;
            }
            // Emacs lock files
            if name.starts_with(".#") {
                return true;
            }
            // Known OS noise
            if matches!(name, ".DS_Store" | "Thumbs.db") {
                return true;
            }
            // zsh completion dumps: .zcompdump, .zcompdump-HOST-5.9, .zcompdump-HOST.zwc
            if name.starts_with(".zcompdump") {
                return true;
            }
            // z/zoxide jump history files: .z, .z.12345 (temp writes)
            if name == ".z" || name.starts_with(".z.") {
                return true;
            }
            // Rotated / numbered log files: app.log.1, server.log.20260411
            if name.contains(".log.") {
                return true;
            }
        }

        // ── HOME hidden-directory rule ────────────────────────────────────
        //
        // If this path is INSIDE a hidden directory directly under HOME
        // (e.g. ~/.cargo/bin/rustc), exclude it UNLESS the hidden directory
        // is in HOME_HIDDEN_DIR_WHITELIST (e.g. ~/.ssh/id_rsa → keep).
        //
        // Hidden FILES at the home root (e.g. ~/.zshrc, ~/.gitconfig) are
        // NOT affected because they have no second path component.
        if let Some(ref home) = self.home_dir {
            if let Ok(rel) = path.strip_prefix(home) {
                let mut comps = rel.components();
                if let Some(std::path::Component::Normal(first)) = comps.next() {
                    let dir_name = first.to_string_lossy();
                    // Only applies to hidden directories (starts with '.')
                    // AND only when path goes deeper than just the dir itself.
                    if dir_name.starts_with('.') && comps.next().is_some() {
                        let whitelisted = HOME_HIDDEN_DIR_WHITELIST
                            .iter()
                            .any(|w| dir_name.as_ref() == *w);
                        if !whitelisted {
                            return true;
                        }
                    }
                }
            }
        }

        // ── Glob-based check ──────────────────────────────────────────────
        if self.ignore_set.is_match(path) {
            return true;
        }
        if self.ignore_set.is_match(path_str.as_ref() as &str) {
            return true;
        }

        // ── Component-level fast-path for common noisy dirs ───────────────
        // Catches cases where globset might miss due to absolute path anchoring.
        for ancestor in path.ancestors() {
            if let Some(name) = ancestor.file_name() {
                let name_str = name.to_string_lossy();
                if matches!(
                    name_str.as_ref(),
                    "node_modules" | ".git" | "target" | "__pycache__"
                    | ".rew" | "Library" | ".Trash"
                ) {
                    return true;
                }
            }
        }
        false
    }

    /// Returns `true` if the path should be processed (not ignored).
    pub fn should_process(&self, path: &Path) -> bool {
        !self.should_ignore(path)
    }

    /// Returns the original patterns used to create this filter.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Check whether `path` should be ignored according to a per-directory
    /// ignore config. `watch_dir` is the root of the watched directory that
    /// `path` belongs to. Returns `true` if the file matches any exclude rule.
    pub fn should_ignore_by_dir_config(
        path: &Path,
        watch_dir: &Path,
        cfg: &crate::config::DirIgnoreConfig,
    ) -> bool {
        if cfg.exclude_dirs.is_empty() && cfg.exclude_extensions.is_empty() {
            return false;
        }

        if !cfg.exclude_dirs.is_empty() {
            if let Ok(rel) = path.strip_prefix(watch_dir) {
                let rel_str = rel.to_string_lossy();
                for excluded in &cfg.exclude_dirs {
                    if excluded.contains('/') {
                        let ex_path = std::path::Path::new(excluded.as_str());
                        if rel == ex_path || rel.starts_with(ex_path) {
                            return true;
                        }
                    } else {
                        for component in rel.components() {
                            if let std::path::Component::Normal(name) = component {
                                if name.to_string_lossy() == excluded.as_str() {
                                    return true;
                                }
                            }
                        }
                    }
                }
                let _ = rel_str;
            }
        }

        if !cfg.exclude_extensions.is_empty() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_ascii_lowercase();
                if cfg.exclude_extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext_lower)) {
                    return true;
                }
            }
            // Dotfiles like `.gitconfig` have no extension per Rust's Path API.
            // Match against the full filename so users can exclude them via the
            // "exclude file types" UI (e.g. typing "gitconfig" or ".gitconfig").
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if cfg.exclude_extensions.iter().any(|e| {
                    let pattern = e.strip_prefix('.').unwrap_or(e);
                    // Match ".gitconfig" against pattern "gitconfig"
                    name.eq_ignore_ascii_case(&format!(".{}", pattern))
                }) {
                    return true;
                }
            }
        }

        false
    }
}

impl Default for PathFilter {
    fn default() -> Self {
        Self::from_patterns(&Self::builtin_patterns())
            .expect("Default patterns should be valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn home() -> String {
        std::env::var("HOME").unwrap_or_else(|_| "/Users/testuser".to_string())
    }

    #[test]
    fn test_ignore_node_modules() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/project/node_modules/express/index.js"
        )));
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/project/node_modules/.package-lock.json"
        )));
    }

    #[test]
    fn test_ignore_git_dir() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/project/.git/objects/abc123"
        )));
    }

    #[test]
    fn test_ignore_ds_store() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/project/.DS_Store"
        )));
    }

    #[test]
    fn test_ignore_target_dir() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/project/target/debug/build"
        )));
    }

    #[test]
    fn test_ignore_pycache() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/project/__pycache__/module.pyc"
        )));
    }

    #[test]
    fn test_allow_normal_files() {
        let filter = PathFilter::default();
        assert!(filter.should_process(&PathBuf::from(
            "/Users/foo/project/src/main.rs"
        )));
        assert!(filter.should_process(&PathBuf::from(
            "/Users/foo/Documents/report.txt"
        )));
    }

    #[test]
    fn test_custom_patterns() {
        let patterns = vec!["**/*.log".to_string(), "**/temp/**".to_string()];
        let filter = PathFilter::new(&patterns).unwrap();
        assert!(filter.should_ignore(&PathBuf::from("/var/log/app.log")));
        assert!(filter.should_ignore(&PathBuf::from("/Users/foo/temp/data.txt")));
        assert!(filter.should_process(&PathBuf::from("/Users/foo/src/main.rs")));
    }

    #[test]
    fn test_ignore_swap_files() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/project/src/.main.rs.swp"
        )));
    }

    #[test]
    fn test_home_hidden_dir_rule() {
        let filter = PathFilter::default();
        let h = home();

        // Hidden dirs under HOME → excluded (tool data)
        assert!(filter.should_ignore(&PathBuf::from(format!("{}/.cargo/bin/rustc", h))));
        assert!(filter.should_ignore(&PathBuf::from(format!("{}/.npm/_cacache/foo", h))));
        assert!(filter.should_ignore(&PathBuf::from(format!("{}/.cursor/settings.json", h))));
        assert!(filter.should_ignore(&PathBuf::from(format!("{}/.oh-my-zsh/themes/foo.zsh", h))));
        assert!(filter.should_ignore(&PathBuf::from(format!("{}/.nvm/versions/node/v18/bin/node", h))));

        // Whitelisted hidden dirs under HOME → kept (credentials/important config)
        assert!(filter.should_process(&PathBuf::from(format!("{}/.ssh/id_rsa", h))));
        assert!(filter.should_process(&PathBuf::from(format!("{}/.gnupg/pubring.kbx", h))));
        assert!(filter.should_process(&PathBuf::from(format!("{}/.aws/credentials", h))));
        assert!(filter.should_process(&PathBuf::from(format!("{}/.kube/config", h))));

        // Hidden FILES directly at HOME root → never affected by this rule
        assert!(filter.should_process(&PathBuf::from(format!("{}/.zshrc", h))));
        assert!(filter.should_process(&PathBuf::from(format!("{}/.gitconfig", h))));
    }

    #[test]
    fn test_tmp_hex_pattern() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(&PathBuf::from(
            "/Users/foo/.config/foo.json.tmp-5885abc123"
        )));
        assert!(filter.should_ignore(&PathBuf::from(
            "/tmp/update-notifier.json.tmp-1234567890abcdef"
        )));
    }

    #[test]
    fn test_lock_files() {
        let filter = PathFilter::default();
        // Transient process-lock files → ignored
        assert!(filter.should_ignore(&PathBuf::from("/Users/foo/.claude.json.lock")));
        assert!(filter.should_ignore(&PathBuf::from("/tmp/db.sqlite.lock")));
        assert!(filter.should_ignore(&PathBuf::from("/tmp/foo.LOCK")));
        assert!(filter.should_ignore(&PathBuf::from("/Users/foo/.zsh_history.LOCK")));
        // Package-manager lock files (bare stem) → kept (important user content)
        assert!(filter.should_process(&PathBuf::from("/Users/foo/project/Cargo.lock")));
        assert!(filter.should_process(&PathBuf::from("/Users/foo/project/yarn.lock")));
        assert!(filter.should_process(&PathBuf::from("/Users/foo/project/pnpm-lock.yaml")));
    }
}
