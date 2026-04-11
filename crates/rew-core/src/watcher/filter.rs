//! Path filter for ignoring noise files/directories.
//!
//! Uses `globset` for efficient glob matching against a list of ignore patterns.

use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

/// A path filter that determines whether a file event should be ignored.
#[derive(Debug, Clone)]
pub struct PathFilter {
    /// Compiled glob set for matching ignore patterns
    ignore_set: GlobSet,
    /// Original patterns (for debugging/display)
    patterns: Vec<String>,
}

impl PathFilter {
    /// Create a PathFilter from raw patterns (no merging with defaults).
    fn from_patterns(patterns: &[String]) -> Result<Self, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }
        Ok(Self {
            ignore_set: builder.build()?,
            patterns: patterns.to_vec(),
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
    pub fn builtin_patterns() -> Vec<String> {
        let home = std::env::var("HOME").unwrap_or_default();
        vec![
            // ── rew's own data (MUST be first to avoid recursive storms) ──
            format!("{}/.rew/**", home),
            // ── macOS system / app directories (noisy + permission errors) ─
            format!("{}/Library/**", home),
            format!("{}/Applications/**", home),
            format!("{}/.Trash/**", home),
            format!("{}/.local/**", home),
            // ── AI tool runtime data (high-churn internal files) ────────────
            // These dirs contain AI session state, history, plugin caches etc.
            // that change constantly and are not user content worth protecting.
            format!("{}/.claude-internal/**", home),
            format!("{}/.claude/**", home),
            format!("{}/.cursor/**", home),
            format!("{}/.continue/**", home),
            format!("{}/.codeium/**", home),
            format!("{}/.copilot/**", home),
            // ── CLI tool caches / logs ───────────────────────────────────────
            format!("{}/.npm/_cacache/**", home),
            format!("{}/.npm/_logs/**", home),
            format!("{}/.npm/tmp/**", home),
            format!("{}/.config/configstore/**", home),
            format!("{}/.AppData/**", home),
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

        // Fast path: direct filename checks that don't rely on globset Unicode handling.
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
            // Atomic-write temps with hex suffix: "foo.json.tmp-5885abc" or
            // "foo.json.tmp-ID123" — used by Claude Code, npm, and many CLIs.
            if let Some(idx) = name.find(".tmp-") {
                // Only treat as temp if there's actual content before .tmp-
                if idx > 0 {
                    return true;
                }
            }
            // Lock files for JSON/JSONL/DB files (transient, not user content)
            // Note: Cargo.lock / package-lock.json do NOT end with .lock — they
            // end with -lock.json, so this suffix check is safe.
            if name.ends_with(".lock") || name.ends_with(".LOCK") {
                return true;
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
        }

        // Glob-based check for patterns supplied by the user / config
        if self.ignore_set.is_match(path) {
            return true;
        }
        // Also check against the path as a string for patterns like **/.DS_Store
        if self.ignore_set.is_match(path_str.as_ref() as &str) {
            return true;
        }
        // Check each component for directory-level matching
        // e.g., /Users/foo/node_modules/bar.js should be caught by **/node_modules/**
        for ancestor in path.ancestors() {
            if let Some(name) = ancestor.file_name() {
                let name_str = name.to_string_lossy();
                if matches!(
                    name_str.as_ref(),
                    "node_modules" | ".git" | "target" | "__pycache__"
                    | ".rew" | "Library" | ".Trash"
                    | ".claude-internal" | ".claude" | ".cursor"
                    | ".npm" | ".AppData"
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

        // Check excluded sub-directory/file rules.
        // Two matching modes (backward-compatible):
        //   - Entry contains "/" → relative path from watch_dir. Matches if the
        //     file's relative path equals it (file) or starts with it followed by "/" (dir).
        //   - No "/" → component-name match at any depth (e.g. "node_modules" anywhere).
        if !cfg.exclude_dirs.is_empty() {
            if let Ok(rel) = path.strip_prefix(watch_dir) {
                let rel_str = rel.to_string_lossy();
                for excluded in &cfg.exclude_dirs {
                    if excluded.contains('/') {
                        // Relative path match: dir prefix or exact file match
                        let ex_path = std::path::Path::new(excluded.as_str());
                        if rel == ex_path || rel.starts_with(ex_path) {
                            return true;
                        }
                    } else {
                        // Component name match at any depth
                        for component in rel.components() {
                            if let std::path::Component::Normal(name) = component {
                                if name.to_string_lossy() == excluded.as_str() {
                                    return true;
                                }
                            }
                        }
                    }
                }
                let _ = rel_str; // suppress unused warning
            }
        }

        // Check excluded file extensions
        if !cfg.exclude_extensions.is_empty() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_ascii_lowercase();
                if cfg.exclude_extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext_lower)) {
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
}
