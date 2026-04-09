//! APFS clonefile (CoW) backup support.
//!
//! Provides zero-cost file cloning on macOS APFS via the `clonefile(2)` syscall.
//! Falls back to `fs::copy` when clonefile is unavailable (cross-volume, non-APFS,
//! non-macOS).
//!
//! Used by:
//! - Hook pre-tool: backup a file before AI modifies it (< 1ms)
//! - ObjectStore: store file contents with deduplication

use crate::error::{RewError, RewResult};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Result of a clone/copy operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyMethod {
    /// Used APFS clonefile (zero-cost CoW)
    Clonefile,
    /// Used regular fs::copy
    FallbackCopy,
    /// File was skipped (didn't exist, etc.)
    Skipped,
}

/// Clone or copy a file to a destination.
///
/// Strategy:
/// 1. If source and dest are on the same APFS volume → `clonefile(2)` (O(1), zero space)
/// 2. Otherwise → `fs::copy`
///
/// Returns the method used and the destination path.
pub fn clone_or_copy(src: &Path, dest: &Path) -> RewResult<CopyMethod> {
    if !src.exists() {
        return Ok(CopyMethod::Skipped);
    }

    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Try clonefile first on macOS
    #[cfg(target_os = "macos")]
    {
        // Check against dest's parent dir (dest file doesn't exist yet)
        let dest_check = dest.parent().unwrap_or(dest);
        if same_volume(src, dest_check) {
            match try_clonefile(src, dest) {
                Ok(()) => {
                    debug!("clonefile: {} → {}", src.display(), dest.display());
                    return Ok(CopyMethod::Clonefile);
                }
                Err(e) => {
                    debug!("clonefile failed (falling back to copy): {}", e);
                }
            }
        }
    }

    // Fallback: regular copy
    std::fs::copy(src, dest)?;
    debug!("fs::copy: {} → {}", src.display(), dest.display());
    Ok(CopyMethod::FallbackCopy)
}

/// Backup a file to the objects directory, preserving the original path structure.
///
/// For pre-tool hook: given `/Users/alice/project/src/main.rs` and backup root
/// `.rew/shadow/`, creates `.rew/shadow/Users/alice/project/src/main.rs`.
///
/// Uses clonefile when possible for zero-cost backup.
pub fn backup_file_to(src: &Path, backup_root: &Path) -> RewResult<(PathBuf, CopyMethod)> {
    let relative = src.strip_prefix("/").unwrap_or(src);
    let dest = backup_root.join(relative);
    let method = clone_or_copy(src, &dest)?;
    Ok((dest, method))
}

/// Try to clonefile (macOS APFS CoW clone).
#[cfg(target_os = "macos")]
pub fn try_clonefile(src: &Path, dest: &Path) -> RewResult<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src_c = CString::new(src.as_os_str().as_bytes())
        .map_err(|e| RewError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;
    let dest_c = CString::new(dest.as_os_str().as_bytes())
        .map_err(|e| RewError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;

    let ret = unsafe { libc::clonefile(src_c.as_ptr(), dest_c.as_ptr(), 0) };

    if ret == 0 {
        Ok(())
    } else {
        Err(RewError::Io(std::io::Error::last_os_error()))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn try_clonefile(_src: &Path, _dest: &Path) -> RewResult<()> {
    Err(RewError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "clonefile not available on this platform",
    )))
}

/// Check if two paths are on the same volume (macOS: compare statfs mount point).
#[cfg(target_os = "macos")]
pub fn same_volume(a: &Path, b: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let a_c = match CString::new(a.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let b_c = match CString::new(b.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return false,
    };

    unsafe {
        let mut stat_a: libc::statfs = std::mem::zeroed();
        let mut stat_b: libc::statfs = std::mem::zeroed();

        if libc::statfs(a_c.as_ptr(), &mut stat_a) != 0 {
            return false;
        }
        if libc::statfs(b_c.as_ptr(), &mut stat_b) != 0 {
            return false;
        }

        stat_a.f_mntonname == stat_b.f_mntonname
    }
}

#[cfg(not(target_os = "macos"))]
pub fn same_volume(_a: &Path, _b: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_clone_or_copy_creates_dest() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("original.txt");
        std::fs::write(&src, "hello world").unwrap();

        let dest = dir.path().join("backup/copy.txt");
        let method = clone_or_copy(&src, &dest).unwrap();

        assert!(dest.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello world");
        // On macOS same-volume tempdir, should use clonefile
        #[cfg(target_os = "macos")]
        assert_eq!(method, CopyMethod::Clonefile);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(method, CopyMethod::FallbackCopy);
    }

    #[test]
    fn test_clone_or_copy_nonexistent_returns_skipped() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("nonexistent.txt");
        let dest = dir.path().join("backup.txt");

        let method = clone_or_copy(&src, &dest).unwrap();
        assert_eq!(method, CopyMethod::Skipped);
        assert!(!dest.exists());
    }

    #[test]
    fn test_backup_file_to() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("project/src/main.rs");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "fn main() {}").unwrap();

        let backup_root = dir.path().join("shadow");
        let (dest, method) = backup_file_to(&src, &backup_root).unwrap();

        assert!(dest.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "fn main() {}");
        assert_ne!(method, CopyMethod::Skipped);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_same_volume_tempdir() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        assert!(same_volume(&a, &b));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_clonefile_produces_identical_content() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("cloned.txt");
        std::fs::write(&src, "important data 12345").unwrap();

        try_clonefile(&src, &dest).unwrap();

        assert!(dest.exists());
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "important data 12345"
        );
    }
}
