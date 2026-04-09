//! Content-addressable object store for file backups.
//!
//! Stores file contents under their SHA-256 hash in `.rew/objects/`.
//! Structure: `.rew/objects/ab/cdef1234...` (first 2 chars as subdirectory).
//!
//! Key properties:
//! - **Deduplication**: identical file contents stored only once
//! - **Concurrent safety**: writes use tmp file + atomic rename
//! - **CoW aware**: prefers clonefile on APFS (same volume), falls back to fs::copy

use crate::backup::clonefile;
use crate::error::{RewError, RewResult};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Size of the read buffer for SHA-256 hashing (64 KB).
const HASH_BUF_SIZE: usize = 64 * 1024;

/// Content-addressable object store.
///
/// Files are stored under their SHA-256 hash:
/// `root/ab/cdef0123456789...` where `ab` is the first 2 hex chars.
pub struct ObjectStore {
    /// Root directory (typically `.rew/objects/`)
    root: PathBuf,
}

impl ObjectStore {
    /// Create a new ObjectStore at the given root directory.
    /// Creates the directory if it doesn't exist.
    pub fn new(root: PathBuf) -> RewResult<Self> {
        fs::create_dir_all(&root)?;
        Ok(ObjectStore { root })
    }

    /// Store a file's contents, returning the SHA-256 hash.
    ///
    /// If an object with the same hash already exists, returns immediately
    /// (deduplication). Otherwise, copies the file to the object store
    /// using clonefile (APFS CoW) when possible, falling back to fs::copy.
    ///
    /// Thread/process safe: writes to a temporary file first, then
    /// atomically renames into place.
    pub fn store(&self, source: &Path) -> RewResult<String> {
        if !source.exists() {
            return Err(RewError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Source file not found: {}", source.display()),
            )));
        }

        // Compute hash
        let hash = sha256_file(source)?;
        let obj_path = self.object_path(&hash);

        // Dedup: if already stored, skip
        if obj_path.exists() {
            debug!("Object already exists: {}", &hash[..12]);
            return Ok(hash);
        }

        // Ensure parent directory exists
        if let Some(parent) = obj_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write to temp file first (atomic rename for concurrency safety)
        let tmp_path = self.root.join(format!(".tmp-{}", uuid::Uuid::new_v4()));

        let copy_result = self.clone_or_copy_file(source, &tmp_path);

        match copy_result {
            Ok(()) => {
                // Atomic rename into place
                // If another process already wrote the same hash, rename will
                // overwrite — that's fine since the content is identical.
                fs::rename(&tmp_path, &obj_path).map_err(|e| {
                    // Clean up temp file on rename failure
                    let _ = fs::remove_file(&tmp_path);
                    e
                })?;
                debug!("Stored object: {} from {}", &hash[..12], source.display());
                Ok(hash)
            }
            Err(e) => {
                // Clean up temp file on copy failure
                let _ = fs::remove_file(&tmp_path);
                Err(e)
            }
        }
    }

    /// Store a file using a fast path: inode+mtime+size as key, skipping SHA-256 hash.
    ///
    /// This is ~100x faster than `store()` because it only does metadata lookup + clonefile,
    /// no file content reading. Used for full-scan where we just need a backup copy.
    ///
    /// The key format is: `fast-{inode}-{mtime_secs}-{size}`
    /// This is NOT content-addressable (no dedup across identical files), but it's
    /// sufficient for undo/restore since we just need to recover the file content.
    ///
    /// Returns the fast key on success.
    pub fn store_fast(&self, source: &Path) -> RewResult<String> {
        if !source.exists() {
            return Err(RewError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Source file not found: {}", source.display()),
            )));
        }

        let meta = std::fs::metadata(source)?;

        // Build fast key from inode + mtime + size
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            meta.ino()
        };
        #[cfg(not(unix))]
        let inode = 0u64;

        let mtime = meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let size = meta.len();

        let key = format!("fast-{}-{}-{}", inode, mtime, size);
        let obj_path = self.object_path(&key);

        // Already stored with same key? Skip.
        if obj_path.exists() {
            return Ok(key);
        }

        // Ensure parent directory
        if let Some(parent) = obj_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Direct clonefile — no tmp file needed since key is unique per inode+mtime+size
        let copy_result = self.clone_or_copy_file(source, &obj_path);
        match copy_result {
            Ok(()) => {
                debug!("Fast stored: {} from {}", &key, source.display());
                Ok(key)
            }
            Err(e) => {
                let _ = fs::remove_file(&obj_path);
                Err(e)
            }
        }
    }

    /// Retrieve the filesystem path of a stored object.
    /// Returns None if the object doesn't exist.
    pub fn retrieve(&self, hash: &str) -> Option<PathBuf> {
        let path = self.object_path(hash);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    /// Check if an object with the given hash exists.
    pub fn exists(&self, hash: &str) -> bool {
        self.object_path(hash).exists()
    }

    /// Get the total size of the object store in bytes.
    pub fn total_size(&self) -> RewResult<u64> {
        let mut total = 0u64;
        for entry in walkdir::WalkDir::new(&self.root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
        {
            // Skip temp files
            if entry.file_name().to_string_lossy().starts_with(".tmp-") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
        Ok(total)
    }

    /// Delete an object by hash. Returns true if it existed.
    pub fn delete(&self, hash: &str) -> RewResult<bool> {
        let path = self.object_path(hash);
        if path.exists() {
            fs::remove_file(&path)?;
            // Try to remove empty parent dir (ignore errors)
            if let Some(parent) = path.parent() {
                let _ = fs::remove_dir(parent);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Map a hash to its filesystem path: `root/ab/cdef...`
    fn object_path(&self, hash: &str) -> PathBuf {
        if hash.len() < 4 {
            // Safety: should never happen with SHA-256, but don't panic
            return self.root.join(hash);
        }
        self.root.join(&hash[..2]).join(&hash[2..])
    }

    /// Copy a file using clonefile (APFS CoW) if possible, else fs::copy.
    fn clone_or_copy_file(&self, src: &Path, dest: &Path) -> RewResult<()> {
        clonefile::clone_or_copy(src, dest)?;
        Ok(())
    }

    /// Check if clonefile is likely to work between src and our root.
    fn check_clonefile_support(&self, _src: &Path) -> bool {
        #[cfg(target_os = "macos")]
        {
            clonefile::same_volume(_src, &self.root)
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

/// Compute SHA-256 hash of a file's contents.
pub fn sha256_file(path: &Path) -> RewResult<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF_SIZE];

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_store_and_retrieve() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        // Create a test file
        let src = dir.path().join("test.txt");
        fs::write(&src, "hello world").unwrap();

        // Store it
        let hash = store.store(&src).unwrap();
        assert_eq!(hash.len(), 64); // SHA-256 hex = 64 chars

        // Retrieve it
        let obj_path = store.retrieve(&hash).unwrap();
        assert!(obj_path.exists());
        let content = fs::read_to_string(&obj_path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_deduplication() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        // Create two files with identical content
        let src1 = dir.path().join("file1.txt");
        let src2 = dir.path().join("file2.txt");
        fs::write(&src1, "same content").unwrap();
        fs::write(&src2, "same content").unwrap();

        let hash1 = store.store(&src1).unwrap();
        let hash2 = store.store(&src2).unwrap();

        // Same hash
        assert_eq!(hash1, hash2);

        // Only one object file on disk
        let mut count = 0;
        for entry in walkdir::WalkDir::new(dir.path().join("objects"))
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
        {
            if !entry.file_name().to_string_lossy().starts_with(".tmp-") {
                count += 1;
            }
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn test_different_content_different_hash() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let src1 = dir.path().join("a.txt");
        let src2 = dir.path().join("b.txt");
        fs::write(&src1, "content A").unwrap();
        fs::write(&src2, "content B").unwrap();

        let hash1 = store.store(&src1).unwrap();
        let hash2 = store.store(&src2).unwrap();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_exists() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let src = dir.path().join("test.txt");
        fs::write(&src, "data").unwrap();

        let hash = store.store(&src).unwrap();
        assert!(store.exists(&hash));
        assert!(!store.exists("nonexistent_hash_0000000000000000000000000000000000000000000000000000000000000000"));
    }

    #[test]
    fn test_delete() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let src = dir.path().join("test.txt");
        fs::write(&src, "to be deleted").unwrap();

        let hash = store.store(&src).unwrap();
        assert!(store.exists(&hash));

        let deleted = store.delete(&hash).unwrap();
        assert!(deleted);
        assert!(!store.exists(&hash));

        // Delete again → false
        let deleted_again = store.delete(&hash).unwrap();
        assert!(!deleted_again);
    }

    #[test]
    fn test_store_nonexistent_file() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let result = store.store(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_total_size() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let src1 = dir.path().join("a.txt");
        let src2 = dir.path().join("b.txt");
        fs::write(&src1, "aaaa").unwrap(); // 4 bytes
        fs::write(&src2, "bbbbbbbb").unwrap(); // 8 bytes

        store.store(&src1).unwrap();
        store.store(&src2).unwrap();

        let total = store.total_size().unwrap();
        assert_eq!(total, 12); // 4 + 8
    }

    #[test]
    fn test_sha256_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hash_test.txt");
        fs::write(&path, "hello").unwrap();

        let hash = sha256_file(&path).unwrap();
        // Known SHA-256 of "hello"
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_object_path_structure() {
        let dir = tempdir().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let path = store.object_path(hash);

        // Should be root/ab/cdef1234...
        assert!(path.to_string_lossy().contains("/ab/"));
        assert!(path.to_string_lossy().ends_with("cdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"));
    }
}
