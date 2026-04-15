use crate::error::RewResult;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const PRE_TOOL_HASHES_DIR: &str = "pre-tool-hashes";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreToolHashEntry {
    session_key: String,
    file_path: String,
    content_hash: String,
    created_at: String,
}

fn hex_sha256(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn pre_tool_store_root() -> PathBuf {
    pre_tool_store_root_in(&crate::rew_home_dir())
}

pub fn pre_tool_store_root_in(rew_home: &Path) -> PathBuf {
    rew_home.join(PRE_TOOL_HASHES_DIR)
}

pub fn pre_tool_store_root_for_objects_root(objects_root: &Path) -> PathBuf {
    objects_root
        .parent()
        .map(pre_tool_store_root_in)
        .unwrap_or_else(pre_tool_store_root)
}

fn session_dir(root: &Path, session_key: &str) -> PathBuf {
    root.join(hex_sha256(session_key))
}

fn entry_path(root: &Path, session_key: &str, file_path: &str) -> PathBuf {
    session_dir(root, session_key).join(format!("{}.json", hex_sha256(file_path)))
}

pub fn set_pre_tool_hash(session_key: &str, file_path: &str, content_hash: &str) -> RewResult<()> {
    set_pre_tool_hash_in(&pre_tool_store_root(), session_key, file_path, content_hash)
}

pub fn set_pre_tool_hash_in(
    root: &Path,
    session_key: &str,
    file_path: &str,
    content_hash: &str,
) -> RewResult<()> {
    let dest = entry_path(root, session_key, file_path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension(format!("json.tmp-{}", Uuid::new_v4()));
    let entry = PreToolHashEntry {
        session_key: session_key.to_string(),
        file_path: file_path.to_string(),
        content_hash: content_hash.to_string(),
        created_at: Utc::now().to_rfc3339(),
    };
    let data = serde_json::to_vec(&entry)?;
    let mut file = File::create(&tmp)?;
    file.write_all(&data)?;
    file.sync_all()?;
    fs::rename(&tmp, &dest)?;
    Ok(())
}

pub fn get_pre_tool_hash(session_key: &str, file_path: &str) -> RewResult<Option<String>> {
    get_pre_tool_hash_in(&pre_tool_store_root(), session_key, file_path)
}

pub fn get_pre_tool_hash_in(
    root: &Path,
    session_key: &str,
    file_path: &str,
) -> RewResult<Option<String>> {
    let path = entry_path(root, session_key, file_path);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read(&path)?;
    let entry: PreToolHashEntry = serde_json::from_slice(&raw)?;
    if entry.session_key != session_key || entry.file_path != file_path {
        return Ok(None);
    }
    Ok(Some(entry.content_hash))
}

pub fn delete_pre_tool_hashes_for_session(session_key: &str) -> RewResult<()> {
    delete_pre_tool_hashes_for_session_in(&pre_tool_store_root(), session_key)
}

pub fn delete_pre_tool_hashes_for_session_in(root: &Path, session_key: &str) -> RewResult<()> {
    let dir = session_dir(root, session_key);
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    Ok(())
}

pub fn delete_all_pre_tool_hashes() -> RewResult<()> {
    delete_all_pre_tool_hashes_in(&pre_tool_store_root())
}

pub fn delete_all_pre_tool_hashes_in(root: &Path) -> RewResult<()> {
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::tempdir;

    #[test]
    fn set_and_get_pre_tool_hash_round_trip() {
        let dir = tempdir().unwrap();
        set_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt", "sha-1").unwrap();
        let hash = get_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt").unwrap();
        assert_eq!(hash.as_deref(), Some("sha-1"));
    }

    #[test]
    fn same_session_overwrites_same_path_atomically() {
        let dir = tempdir().unwrap();
        set_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt", "sha-old").unwrap();
        set_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt", "sha-new").unwrap();
        let hash = get_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt").unwrap();
        assert_eq!(hash.as_deref(), Some("sha-new"));
    }

    #[test]
    fn different_sessions_same_path_remain_isolated() {
        let dir = tempdir().unwrap();
        set_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt", "sha-a").unwrap();
        set_pre_tool_hash_in(dir.path(), "workbuddy:1", "/tmp/a.txt", "sha-b").unwrap();
        assert_eq!(
            get_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt")
                .unwrap()
                .as_deref(),
            Some("sha-a")
        );
        assert_eq!(
            get_pre_tool_hash_in(dir.path(), "workbuddy:1", "/tmp/a.txt")
                .unwrap()
                .as_deref(),
            Some("sha-b")
        );
    }

    #[test]
    fn delete_session_cleans_only_that_session() {
        let dir = tempdir().unwrap();
        set_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt", "sha-a").unwrap();
        set_pre_tool_hash_in(dir.path(), "workbuddy:1", "/tmp/b.txt", "sha-b").unwrap();
        delete_pre_tool_hashes_for_session_in(dir.path(), "cursor:1").unwrap();
        assert!(
            get_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            get_pre_tool_hash_in(dir.path(), "workbuddy:1", "/tmp/b.txt")
                .unwrap()
                .as_deref(),
            Some("sha-b")
        );
    }

    #[test]
    fn delete_all_cleans_every_session() {
        let dir = tempdir().unwrap();
        set_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt", "sha-a").unwrap();
        set_pre_tool_hash_in(dir.path(), "workbuddy:1", "/tmp/b.txt", "sha-b").unwrap();
        delete_all_pre_tool_hashes_in(dir.path()).unwrap();
        assert!(
            get_pre_tool_hash_in(dir.path(), "cursor:1", "/tmp/a.txt")
                .unwrap()
                .is_none()
        );
        assert!(
            get_pre_tool_hash_in(dir.path(), "workbuddy:1", "/tmp/b.txt")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn concurrent_writes_for_multiple_files_do_not_collide() {
        let dir = tempdir().unwrap();
        let root = Arc::new(dir.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(4));
        let mut threads = Vec::new();
        for idx in 0..4 {
            let root = root.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                let file_path = format!("/tmp/file-{idx}.txt");
                let hash = format!("sha-{idx}");
                set_pre_tool_hash_in(&root, "cursor:batch", &file_path, &hash).unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        for idx in 0..4 {
            let file_path = format!("/tmp/file-{idx}.txt");
            let expected = format!("sha-{idx}");
            assert_eq!(
                get_pre_tool_hash_in(&root, "cursor:batch", &file_path)
                    .unwrap()
                    .as_deref(),
                Some(expected.as_str())
            );
        }
    }
}
