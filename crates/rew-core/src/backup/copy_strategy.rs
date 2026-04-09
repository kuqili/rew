//! File copying strategy with filtering and retry logic.
//!
//! Uses the same `globset::GlobSet` matching as `PathFilter` for consistency.

use crate::error::RewResult;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

/// Handles file copying with ignore pattern filtering.
///
/// Uses `globset::GlobSet` (same engine as `PathFilter`) for consistent
/// pattern matching across the entire codebase.
pub struct FileCopyStrategy {
    /// Compiled ignore patterns (globset-based)
    ignore_set: GlobSet,
}

impl FileCopyStrategy {
    /// Create a new copy strategy with ignore patterns.
    pub fn new(ignore_patterns: Vec<String>) -> RewResult<Self> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &ignore_patterns {
            let glob = Glob::new(pattern).map_err(|e| {
                crate::error::RewError::Config(format!("Invalid glob pattern '{}': {}", pattern, e))
            })?;
            builder.add(glob);
        }
        let ignore_set = builder.build().map_err(|e| {
            crate::error::RewError::Config(format!("Failed to build glob set: {}", e))
        })?;

        Ok(FileCopyStrategy { ignore_set })
    }

    /// Check if a path matches any ignore pattern.
    ///
    /// Uses the same globset matching logic as PathFilter for consistency.
    /// Also checks path components for known noise directories.
    pub fn should_skip(&self, path: &Path) -> bool {
        // Direct glob match
        if self.ignore_set.is_match(path) {
            return true;
        }

        // Also check as string (for patterns like **/.DS_Store)
        let path_str = path.to_string_lossy();
        if self.ignore_set.is_match(path_str.as_ref() as &str) {
            return true;
        }

        // Component-level check for known noise directories
        // (same approach as PathFilter::should_ignore)
        for ancestor in path.ancestors() {
            if let Some(name) = ancestor.file_name() {
                let name_str = name.to_string_lossy();
                if matches!(
                    name_str.as_ref(),
                    "node_modules" | ".git" | "target" | "__pycache__"
                ) {
                    return true;
                }
            }
        }

        false
    }

    /// Copy a file to the backup location, preserving directory structure.
    ///
    /// For a file at `/Users/alice/Documents/file.txt`, backed up to
    /// `~/.rew/backups/{snapshot_id}/`, the result will be:
    /// `~/.rew/backups/{snapshot_id}/Users/alice/Documents/file.txt`
    pub fn copy_file(&self, src: &Path, backup_root: &Path) -> RewResult<u64> {
        if self.should_skip(src) {
            return Ok(0);
        }

        // Build the target path: preserve the full absolute path structure
        let target = backup_root.join(src.strip_prefix("/").unwrap_or(src));

        // Create parent directories
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Copy the file and return size
        let size = std::fs::copy(src, &target)?;

        Ok(size)
    }

    /// Attempt to copy a file with retry logic for locked files.
    pub fn copy_file_with_retry(
        &self,
        src: &Path,
        backup_root: &Path,
        max_retries: u32,
    ) -> Result<u64, String> {
        let mut last_error = String::new();

        for attempt in 0..=max_retries {
            match self.copy_file(src, backup_root) {
                Ok(size) => return Ok(size),
                Err(e) => {
                    last_error = format!("{}: {}", src.display(), e);

                    if attempt >= max_retries {
                        return Err(last_error);
                    }

                    if attempt < max_retries {
                        std::thread::sleep(std::time::Duration::from_millis(
                            100 * (attempt as u64 + 1),
                        ));
                    }
                }
            }
        }

        Err(last_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ignore_pattern_matching() {
        let strategy = FileCopyStrategy::new(vec![
            "**/node_modules/**".to_string(),
            "**/.git/**".to_string(),
        ])
        .unwrap();

        assert!(strategy.should_skip(Path::new(
            "/Users/alice/project/node_modules/package/index.js"
        )));
        assert!(strategy.should_skip(Path::new(
            "/Users/alice/project/.git/config"
        )));
        assert!(!strategy.should_skip(Path::new(
            "/Users/alice/project/src/main.rs"
        )));
    }

    #[test]
    fn test_copy_file_preserves_structure() {
        let temp_dir = tempdir().unwrap();
        let backup_root = temp_dir.path();

        let strategy = FileCopyStrategy::new(vec![]).unwrap();

        // Create a source file
        let src_file = temp_dir.path().join("source_file.txt");
        std::fs::write(&src_file, "test content").unwrap();

        // Copy it
        let size = strategy.copy_file(&src_file, backup_root).unwrap();
        assert!(size > 0);

        // Verify it exists at the expected location
        let backed_up = backup_root.join(src_file.strip_prefix("/").unwrap_or(&src_file));
        assert!(backed_up.exists());
    }

    #[test]
    fn test_skip_ignored_files() {
        let temp_dir = tempdir().unwrap();

        let strategy = FileCopyStrategy::new(vec!["**/*.swp".to_string()]).unwrap();

        let src_file = temp_dir.path().join("file.swp");
        std::fs::write(&src_file, "swap file").unwrap();

        let size = strategy.copy_file(&src_file, temp_dir.path()).unwrap();
        assert_eq!(size, 0); // Should be skipped, returning 0
    }

    #[test]
    fn test_skip_node_modules_in_absolute_path() {
        // This was the bug: glob::Pattern couldn't match node_modules
        // in absolute paths consistently. globset handles it correctly.
        let strategy = FileCopyStrategy::new(vec![
            "**/node_modules/**".to_string(),
        ])
        .unwrap();

        // This specific case was failing before the fix
        assert!(strategy.should_skip(Path::new(
            "/var/folders/8n/b87hg7q91670vpqqjw22m_kh0000gn/T/.tmp1eCpKM/source/node_modules/dep/file.js"
        )));
    }

    #[test]
    fn test_skip_git_config_in_temp_dir() {
        let strategy = FileCopyStrategy::new(vec![
            "**/.git/**".to_string(),
        ])
        .unwrap();

        assert!(strategy.should_skip(Path::new(
            "/var/folders/8n/tmp/source/.git/config"
        )));
    }
}
