//! Background full-scan for pre-existing files.
//!
//! On daemon startup, walks all watched directories and stores every file
//! in the ObjectStore (using clonefile CoW — near zero disk cost).
//! This closes the "files that existed before rew started" blind spot.
//!
//! Features:
//! - **Incremental**: uses a manifest (path → mtime+hash) to skip unchanged files
//! - **Non-blocking**: runs on a background thread, doesn't delay FSEvent monitoring
//! - **Respects ignore patterns**: uses the same PathFilter as the watcher
//! - **Progress callback**: optional callback for UI progress reporting

use crate::db::Database;
use crate::objects::ObjectStore;
use crate::watcher::filter::PathFilter;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{info, warn, debug};

/// Result of a full scan operation.
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// Total files scanned (walked)
    pub files_scanned: usize,
    /// Files stored (new or changed)
    pub files_stored: usize,
    /// Files skipped (already in store with same mtime)
    pub files_skipped: usize,
    /// Files that failed to store
    pub files_failed: usize,
    /// Elapsed time
    pub elapsed: std::time::Duration,
}

/// Progress update emitted during scanning.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanProgressUpdate {
    /// Which top-level directory is being scanned
    pub dir: String,
    pub files_scanned: usize,
    pub files_stored: usize,
    pub files_skipped: usize,
    pub files_failed: usize,
    /// Estimated total files (from pre-count), 0 = unknown
    pub files_total_estimate: usize,
}

/// Callback type for progress reporting.
/// Called every 100 files (throttled inside walk_and_store).
pub type ProgressCallback = Box<dyn Fn(ScanProgressUpdate) + Send>;

/// Callback type for per-directory completion.
/// Called once when a directory finishes scanning.
pub type DirCompleteCallback = Box<dyn Fn(String) + Send>;

/// Fast file count for a directory (respects filter, skips hidden dirs).
/// Used to estimate total for progress bars. Only does readdir — no file I/O.
pub fn count_files(dir: &Path, filter: &PathFilter) -> usize {
    let mut count = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if filter.should_ignore(&path) {
                continue;
            }
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // Skip hidden directories and .app bundles
                if !name.starts_with('.') && !name.ends_with(".app") {
                    stack.push(path);
                }
            } else if path.is_file() {
                count += 1;
            }
        }
    }
    count
}

/// Run a full scan of the given directories, storing all files in ObjectStore.
///
/// Uses the `file_index` DB table to skip unchanged files (incremental).
///
/// # Arguments
/// - `watch_dirs`: directories to scan
/// - `ignore_patterns`: glob patterns to skip (e.g. `**/node_modules/**`)
/// - `rew_home`: the `.rew/` directory
/// - `db`: reference to the Database for file_index reads/writes
/// - `on_progress`: optional callback for UI progress reporting (None = silent)
///
/// # Returns
/// `ScanResult` with statistics about the scan.
pub fn full_scan(
    watch_dirs: &[PathBuf],
    ignore_patterns: &[String],
    rew_home: &Path,
    db: &Database,
    on_progress: Option<ProgressCallback>,
    on_dir_complete: Option<DirCompleteCallback>,
    max_file_size: Option<u64>,
) -> ScanResult {
    let start = std::time::Instant::now();

    let objects_root = rew_home.join("objects");

    let filter = match PathFilter::new(ignore_patterns) {
        Ok(f) => f,
        Err(e) => {
            warn!("Failed to build path filter for scan: {}. Scanning without filter.", e);
            PathFilter::new(&[]).unwrap()
        }
    };

    let obj_store = match ObjectStore::new(objects_root) {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to open ObjectStore for scan: {}", e);
            return ScanResult {
                files_scanned: 0,
                files_stored: 0,
                files_skipped: 0,
                files_failed: 0,
                elapsed: start.elapsed(),
            };
        }
    };

    let mut files_scanned = 0usize;
    let mut files_stored = 0usize;
    let mut files_skipped = 0usize;
    let mut files_failed = 0usize;

    for dir in watch_dirs {
        if !dir.exists() {
            warn!("Scan: skipping non-existent directory: {}", dir.display());
            continue;
        }

        fire_progress(&on_progress, dir, 0, 0, 0, 0, 0);

        let estimated_total = count_files(dir, &filter);
        info!("Scan: walking {} (~{} files)", dir.display(), estimated_total);

        let mut dir_scanned = 0usize;
        let mut dir_stored = 0usize;
        let mut dir_skipped = 0usize;
        let mut dir_failed = 0usize;

        // Wrap each directory in a transaction for batch insert performance
        let _ = db.begin_transaction();

        walk_and_store(
            dir,
            &filter,
            &obj_store,
            db,
            &mut dir_scanned,
            &mut dir_stored,
            &mut dir_skipped,
            &mut dir_failed,
            &on_progress,
            dir,
            estimated_total,
            max_file_size,
        );

        let _ = db.commit_transaction();

        files_scanned += dir_scanned;
        files_stored += dir_stored;
        files_skipped += dir_skipped;
        files_failed += dir_failed;

        if let Some(ref cb) = on_dir_complete {
            cb(dir.display().to_string());
        }
    }

    let elapsed = start.elapsed();

    info!(
        "Full scan complete: scanned={}, stored={}, skipped={}, failed={}, elapsed={:.1}s",
        files_scanned, files_stored, files_skipped, files_failed,
        elapsed.as_secs_f64(),
    );

    ScanResult {
        files_scanned,
        files_stored,
        files_skipped,
        files_failed,
        elapsed,
    }
}

/// Iteratively walk a directory and store files in the ObjectStore.
/// Uses file_index DB table for incremental skip logic.
#[allow(clippy::too_many_arguments)]
fn walk_and_store(
    dir: &Path,
    filter: &PathFilter,
    store: &ObjectStore,
    db: &Database,
    scanned: &mut usize,
    stored: &mut usize,
    skipped: &mut usize,
    failed: &mut usize,
    on_progress: &Option<ProgressCallback>,
    root_dir: &Path,
    total_estimate: usize,
    max_file_size: Option<u64>,
) {
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current_dir) = stack.pop() {
        let entries = match std::fs::read_dir(&current_dir) {
            Ok(e) => e,
            Err(e) => {
                debug!("Scan: cannot read {}: {}", current_dir.display(), e);
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            if filter.should_ignore(&path) {
                continue;
            }

            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') && name != ".." && name != "." {
                    continue;
                }
                if name.ends_with(".app") {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                *scanned += 1;

                if let Some(max_size) = max_file_size {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if meta.len() > max_size {
                            *skipped += 1;
                            if *scanned % 100 == 0 {
                                fire_progress(on_progress, root_dir, *scanned, *stored, *skipped, *failed, total_estimate);
                            }
                            continue;
                        }
                    }
                }

                let path_key = path.to_string_lossy().to_string();
                let current_mtime = file_mtime_secs(&path);

                // Check file_index in DB: skip if mtime unchanged and object exists
                if let Some(mtime) = current_mtime {
                    if let Ok(Some((db_mtime, db_hash, _))) = db.get_file_index(&path_key) {
                        if db_mtime == mtime && store.exists(&db_hash) {
                            *skipped += 1;
                            if *scanned % 100 == 0 {
                                fire_progress(on_progress, root_dir, *scanned, *stored, *skipped, *failed, total_estimate);
                            }
                            continue;
                        }
                    }
                }

                match store.store_fast(&path) {
                    Ok(hash) => {
                        *stored += 1;
                        if let Some(mtime) = current_mtime {
                            let _ = db.upsert_file_index(&path_key, mtime, &hash, None);
                        }
                    }
                    Err(e) => {
                        *failed += 1;
                        debug!("Scan: failed to store {}: {}", path.display(), e);
                    }
                }

                if *scanned % 100 == 0 {
                    fire_progress(on_progress, root_dir, *scanned, *stored, *skipped, *failed, total_estimate);
                }

                if *scanned % 1000 == 0 {
                    info!("Scan progress: {} files scanned, {} stored, {} skipped", scanned, stored, skipped);
                }
            }
        }
    }
}

/// Fire progress callback if present.
fn fire_progress(
    on_progress: &Option<ProgressCallback>,
    root_dir: &Path,
    scanned: usize,
    stored: usize,
    skipped: usize,
    failed: usize,
    total_estimate: usize,
) {
    if let Some(cb) = on_progress {
        cb(ScanProgressUpdate {
            dir: root_dir.display().to_string(),
            files_scanned: scanned,
            files_stored: stored,
            files_skipped: skipped,
            files_failed: failed,
            files_total_estimate: total_estimate,
        });
    }
}

/// Get a file's mtime as seconds since epoch.
fn file_mtime_secs(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db(rew_home: &Path) -> Database {
        let db = Database::open(&rew_home.join("snapshots.db")).unwrap();
        db.initialize().unwrap();
        db
    }

    #[test]
    fn test_full_scan_basic() {
        let dir = tempdir().unwrap();
        let rew_home = tempdir().unwrap();
        let db = test_db(rew_home.path());

        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("b.txt"), "world").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("c.txt"), "nested").unwrap();

        let result = full_scan(
            &[dir.path().to_path_buf()],
            &["**/.DS_Store".to_string()],
            rew_home.path(),
            &db,
            None,
            None,
            None,
        );

        assert_eq!(result.files_scanned, 3);
        assert_eq!(result.files_stored, 3);
        assert_eq!(result.files_skipped, 0);
        assert_eq!(result.files_failed, 0);
    }

    #[test]
    fn test_incremental_scan_skips_unchanged() {
        let dir = tempdir().unwrap();
        let rew_home = tempdir().unwrap();
        let db = test_db(rew_home.path());

        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("b.txt"), "world").unwrap();

        let r1 = full_scan(&[dir.path().to_path_buf()], &[], rew_home.path(), &db, None, None, None);
        assert_eq!(r1.files_stored, 2);

        let r2 = full_scan(&[dir.path().to_path_buf()], &[], rew_home.path(), &db, None, None, None);
        assert_eq!(r2.files_stored, 0);
        assert_eq!(r2.files_skipped, 2);
    }

    #[test]
    fn test_incremental_scan_detects_changes() {
        let dir = tempdir().unwrap();
        let rew_home = tempdir().unwrap();
        let db = test_db(rew_home.path());

        std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
        let r1 = full_scan(&[dir.path().to_path_buf()], &[], rew_home.path(), &db, None, None, None);
        assert_eq!(r1.files_stored, 1);

        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(dir.path().join("a.txt"), "v2-modified").unwrap();

        let r2 = full_scan(&[dir.path().to_path_buf()], &[], rew_home.path(), &db, None, None, None);
        assert_eq!(r2.files_stored, 1);
        assert_eq!(r2.files_skipped, 0);
    }

    #[test]
    fn test_scan_respects_ignore_patterns() {
        let dir = tempdir().unwrap();
        let rew_home = tempdir().unwrap();
        let db = test_db(rew_home.path());

        std::fs::write(dir.path().join("good.txt"), "keep").unwrap();
        let nm = dir.path().join("node_modules");
        std::fs::create_dir(&nm).unwrap();
        std::fs::write(nm.join("pkg.js"), "ignored").unwrap();

        let result = full_scan(
            &[dir.path().to_path_buf()],
            &["**/node_modules/**".to_string()],
            rew_home.path(),
            &db,
            None,
            None,
            None,
        );

        assert_eq!(result.files_scanned, 1);
        assert_eq!(result.files_stored, 1);
    }

    #[test]
    fn test_scan_nonexistent_dir() {
        let rew_home = tempdir().unwrap();
        let db = test_db(rew_home.path());
        let result = full_scan(&[PathBuf::from("/nonexistent/path/abc")], &[], rew_home.path(), &db, None, None, None);
        assert_eq!(result.files_scanned, 0);
    }

    #[test]
    fn test_count_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("c.txt"), "c").unwrap();

        let filter = PathFilter::new(&[]).unwrap();
        assert_eq!(count_files(dir.path(), &filter), 3);
    }

    #[test]
    fn test_progress_callback_fires() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let dir = tempdir().unwrap();
        let rew_home = tempdir().unwrap();
        let db = test_db(rew_home.path());

        for i in 0..210 {
            std::fs::write(dir.path().join(format!("f{}.txt", i)), format!("content{}", i)).unwrap();
        }

        let callback_count = Arc::new(AtomicUsize::new(0));
        let cc = callback_count.clone();

        let result = full_scan(
            &[dir.path().to_path_buf()],
            &[],
            rew_home.path(),
            &db,
            Some(Box::new(move |_update| {
                cc.fetch_add(1, Ordering::Relaxed);
            })),
            None,
            None,
        );

        assert_eq!(result.files_scanned, 210);
        assert!(callback_count.load(Ordering::Relaxed) >= 2, "Should fire callback at least twice for 210 files");
    }
}
