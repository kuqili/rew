//! Text diff computation using the `similar` crate (Patience algorithm).
//!
//! ## Design decisions
//!
//! - **Algorithm**: Patience diff — fewest spurious matches in code, used by git
//!   when lines are unique.  Falls back to Myers for files with many repeated lines.
//! - **On-demand only**: We do NOT store the full diff text in SQLite (too large
//!   for thousands of changes). Callers retrieve old/new content from the object
//!   store and call `compute_diff` when the user actually needs the diff.
//! - **Summaries at record time**: `count_changed_lines` is cheap enough to call
//!   in the daemon pipeline so `lines_added` / `lines_removed` are always populated.

use crate::objects::ObjectStore;
use similar::{Algorithm, ChangeTag, TextDiff};
use tracing::warn;

/// Maximum file size we will attempt to diff (bytes). Files larger than this
/// return a human-readable notice instead of a diff.
const MAX_DIFF_BYTES: usize = 1024 * 1024; // 1 MB

/// Number of unchanged context lines shown around each changed hunk.
const CONTEXT_LINES: usize = 3;

/// Result of a diff operation.
#[derive(Debug, Clone)]
pub struct DiffResult {
    /// Unified diff text (unified format, ready for display).
    pub text: String,
    /// Number of inserted lines.
    pub lines_added: u32,
    /// Number of deleted lines.
    pub lines_removed: u32,
}

const MISSING_OBJECT_DIFF_NOTICE: &str =
    "[对象缺失，无法精确显示 diff；该变更的历史对象文件当前不可读取]\n";

/// Compute a Patience diff between `old` and `new` byte slices.
///
/// Returns `None` when the content is binary (contains null bytes).
/// Returns a `DiffResult` with a placeholder message for oversized files.
pub fn compute_diff(old: &[u8], new: &[u8], old_name: &str, new_name: &str) -> Option<DiffResult> {
    // Binary detection: scan first 8 KB for null bytes
    if is_binary(old) || is_binary(new) {
        return None;
    }

    // Oversized file guard
    if old.len() > MAX_DIFF_BYTES || new.len() > MAX_DIFF_BYTES {
        return Some(DiffResult {
            text: format!(
                "[文件过大，不显示 diff（超过 {} KB）]\n",
                MAX_DIFF_BYTES / 1024
            ),
            lines_added: 0,
            lines_removed: 0,
        });
    }

    let old_str = std::str::from_utf8(old).ok()?;
    let new_str = std::str::from_utf8(new).ok()?;

    if old_str == new_str {
        return Some(DiffResult {
            text: "[内容未变化]\n".to_string(),
            lines_added: 0,
            lines_removed: 0,
        });
    }

    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_lines(old_str, new_str);

    let mut output = String::with_capacity(new_str.len() / 2);
    let mut lines_added: u32 = 0;
    let mut lines_removed: u32 = 0;

    // Unified diff header
    output.push_str(&format!("--- {}\n", old_name));
    output.push_str(&format!("+++ {}\n", new_name));

    for group in diff.grouped_ops(CONTEXT_LINES) {
        if group.is_empty() {
            continue;
        }

        // Compute hunk header ranges
        let first = &group[0];
        let last = &group[group.len() - 1];

        let old_start = first.old_range().start;
        let old_end = last.old_range().end;
        let new_start = first.new_range().start;
        let new_end = last.new_range().end;

        // git-style unified diff counts: use 0 for empty ranges
        let old_count = old_end - old_start;
        let new_count = new_end - new_start;

        if old_count == 1 {
            output.push_str(&format!("@@ -{} ", old_start + 1));
        } else {
            output.push_str(&format!("@@ -{},{} ", old_start + 1, old_count));
        }
        if new_count == 1 {
            output.push_str(&format!("+{} @@\n", new_start + 1));
        } else {
            output.push_str(&format!("+{},{} @@\n", new_start + 1, new_count));
        }

        for op in &group {
            for change in diff.iter_changes(op) {
                match change.tag() {
                    ChangeTag::Delete => {
                        output.push('-');
                        lines_removed += 1;
                    }
                    ChangeTag::Insert => {
                        output.push('+');
                        lines_added += 1;
                    }
                    ChangeTag::Equal => {
                        output.push(' ');
                    }
                }
                output.push_str(change.value());
                // Ensure line ends with newline
                if !change.value().ends_with('\n') {
                    output.push('\n');
                }
            }
        }
    }

    Some(DiffResult {
        text: output,
        lines_added,
        lines_removed,
    })
}

/// Fast line-count-only summary — used at record time to populate
/// `lines_added` / `lines_removed` without storing the full diff.
///
/// Returns `(added, removed)`.  Binary or oversized files return `(0, 0)`.
pub fn count_changed_lines(old: &[u8], new: &[u8]) -> (u32, u32) {
    if is_binary(old) || is_binary(new) {
        return (0, 0);
    }
    if old.len() > MAX_DIFF_BYTES || new.len() > MAX_DIFF_BYTES {
        return (0, 0);
    }
    let old_str = match std::str::from_utf8(old) {
        Ok(s) => s,
        Err(_) => return (0, 0),
    };
    let new_str = match std::str::from_utf8(new) {
        Ok(s) => s,
        Err(_) => return (0, 0),
    };

    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_lines(old_str, new_str);

    let mut added = 0u32;
    let mut removed = 0u32;
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            match change.tag() {
                ChangeTag::Insert => added += 1,
                ChangeTag::Delete => removed += 1,
                ChangeTag::Equal => {}
            }
        }
    }
    (added, removed)
}

fn read_store_bytes_for_diff(
    store: &ObjectStore,
    hash: Option<&str>,
    side: &str,
) -> Option<Vec<u8>> {
    let Some(hash) = hash else {
        return Some(Vec::new());
    };

    let Some(path) = store.retrieve(hash) else {
        warn!(
            hash = %hash,
            side,
            "Diff object missing; skipping precise line stats instead of treating it as empty content"
        );
        return None;
    };

    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(err) => {
            warn!(
                hash = %hash,
                side,
                error = %err,
                "Diff object unreadable; skipping precise line stats instead of treating it as empty content"
            );
            None
        }
    }
}

/// Compute line stats from object-store hashes without silently turning a
/// missing object into empty content.
///
/// `None` still means semantic emptiness for Created/Deleted comparisons.
/// But when a hash is present and the object is missing/unreadable, we log and
/// return `(0, 0)` rather than misreporting the change as a full-file add/delete.
pub fn count_changed_lines_from_store(
    store: &ObjectStore,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
) -> (u32, u32) {
    let Some(old_bytes) = read_store_bytes_for_diff(store, old_hash, "old") else {
        return (0, 0);
    };
    let Some(new_bytes) = read_store_bytes_for_diff(store, new_hash, "new") else {
        return (0, 0);
    };
    count_changed_lines(&old_bytes, &new_bytes)
}

/// Compute a display diff from object-store hashes without pretending that a
/// missing object equals empty file content.
///
/// When a referenced object is missing/unreadable, this returns a human-readable
/// placeholder diff instead of fabricating an empty-side diff.
pub fn compute_diff_from_store(
    store: &ObjectStore,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
    old_name: &str,
    new_name: &str,
) -> Option<DiffResult> {
    let Some(old_bytes) = read_store_bytes_for_diff(store, old_hash, "old") else {
        return Some(DiffResult {
            text: MISSING_OBJECT_DIFF_NOTICE.to_string(),
            lines_added: 0,
            lines_removed: 0,
        });
    };
    let Some(new_bytes) = read_store_bytes_for_diff(store, new_hash, "new") else {
        return Some(DiffResult {
            text: MISSING_OBJECT_DIFF_NOTICE.to_string(),
            lines_added: 0,
            lines_removed: 0,
        });
    };
    compute_diff(&old_bytes, &new_bytes, old_name, new_name)
}

/// Compute a git-like line similarity score between two text contents.
///
/// Returns `Some(0..=100)` for text files and `None` for binary / oversized
/// inputs where a text-based similarity heuristic would be misleading.
///
/// This is intentionally approximate rather than a byte-for-byte reimplementation
/// of git's internals. It uses the ratio of unchanged lines to the larger side's
/// total line count, which is good enough for rename+edit pairing.
pub fn similarity_score(old: &[u8], new: &[u8]) -> Option<u32> {
    if is_binary(old) || is_binary(new) {
        return None;
    }
    if old.len() > MAX_DIFF_BYTES || new.len() > MAX_DIFF_BYTES {
        return None;
    }

    let old_str = std::str::from_utf8(old).ok()?;
    let new_str = std::str::from_utf8(new).ok()?;

    if old_str == new_str {
        return Some(100);
    }

    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_lines(old_str, new_str);

    let old_total = old_str.lines().count();
    let new_total = new_str.lines().count();
    let denom = old_total.max(new_total);
    if denom == 0 {
        return Some(100);
    }

    let mut unchanged = 0usize;
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            if matches!(change.tag(), ChangeTag::Equal) {
                unchanged += 1;
            }
        }
    }

    Some(((unchanged * 100) / denom) as u32)
}

fn is_binary(data: &[u8]) -> bool {
    // Only scan the first 8 KB — sufficient heuristic, avoids reading huge files
    data[..data.len().min(8192)].contains(&0u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_basic_diff() {
        let old = b"line1\nline2\nline3\n";
        let new = b"line1\nline2 modified\nline3\nline4\n";
        let result = compute_diff(old, new, "old.txt", "new.txt").unwrap();
        assert!(result.text.contains("-line2\n"));
        assert!(result.text.contains("+line2 modified\n"));
        assert!(result.text.contains("+line4\n"));
        assert_eq!(result.lines_added, 2);
        assert_eq!(result.lines_removed, 1);
    }

    #[test]
    fn test_binary_detection() {
        let binary = b"hello\x00world";
        assert!(compute_diff(binary, b"anything", "a", "b").is_none());
    }

    #[test]
    fn test_count_changed_lines() {
        let old = b"a\nb\nc\n";
        let new = b"a\nb modified\nc\nd\n";
        let (added, removed) = count_changed_lines(old, new);
        assert_eq!(added, 2);
        assert_eq!(removed, 1);
    }

    #[test]
    fn test_identical_files() {
        let content = b"no change\n";
        let result = compute_diff(content, content, "a", "b").unwrap();
        assert!(result.text.contains("内容未变化"));
        assert_eq!(result.lines_added, 0);
        assert_eq!(result.lines_removed, 0);
    }

    #[test]
    fn test_count_changed_lines_from_store_missing_old_hash_does_not_look_like_full_file_add() {
        let dir = TempDir::new().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let new_path = dir.path().join("new.txt");
        std::fs::write(&new_path, "a\nb changed\nc\nd\n").unwrap();
        let new_hash = store.store(&new_path).unwrap();

        let (added, removed) =
            count_changed_lines_from_store(&store, Some("sha_missing_old"), Some(&new_hash));
        assert_eq!(
            (added, removed),
            (0, 0),
            "Missing old object must not be misreported as full-file addition"
        );
    }

    #[test]
    fn test_compute_diff_from_store_missing_old_hash_returns_notice() {
        let dir = TempDir::new().unwrap();
        let store = ObjectStore::new(dir.path().join("objects")).unwrap();

        let new_path = dir.path().join("new.txt");
        std::fs::write(&new_path, "a\nb changed\nc\nd\n").unwrap();
        let new_hash = store.store(&new_path).unwrap();

        let result = compute_diff_from_store(
            &store,
            Some("sha_missing_old"),
            Some(&new_hash),
            "a/file.txt",
            "b/file.txt",
        )
        .expect("missing object should produce a placeholder diff");

        assert!(result.text.contains("对象缺失"));
        assert_eq!(result.lines_added, 0);
        assert_eq!(result.lines_removed, 0);
    }
}
