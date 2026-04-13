use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use rew_core::baseline::resolve_baseline;
use rew_core::db::Database;
use rew_core::objects::ObjectStore;
use rew_core::reconcile::reconcile_task;
use rew_core::types::{Change, ChangeType, FileEventKind, Task, TaskStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormChange {
    kind: char,
    path: String,
    old_path: Option<String>,
}

struct GitRepoEnv {
    _dir: TempDir,
    repo_root: PathBuf,
    db: Database,
    objects_root: PathBuf,
    task_id: String,
}

impl GitRepoEnv {
    fn new() -> Self {
        let workspace_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/git-semantics");
        fs::create_dir_all(&workspace_tmp).unwrap();
        let dir = tempfile::tempdir_in(&workspace_tmp).unwrap();
        let repo_root = dir.path().join("repo");
        let empty_template = dir.path().join("empty-git-template");
        fs::create_dir_all(&repo_root).unwrap();
        fs::create_dir_all(&empty_template).unwrap();

        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(format!("--template={}", empty_template.to_string_lossy()))
            .current_dir(&repo_root)
            .status()
            .unwrap();
        assert!(status.success());

        let _ = Command::new("git")
            .args(["config", "user.email", "rew-tests@example.com"])
            .current_dir(&repo_root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "rew tests"])
            .current_dir(&repo_root)
            .status();

        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        db.initialize().unwrap();
        let objects_root = dir.path().join("objects");
        fs::create_dir_all(&objects_root).unwrap();

        let task_id = "task_git_semantics".to_string();
        let task = Task {
            id: task_id.clone(),
            prompt: Some("git semantics".into()),
            tool: Some("test-tool".into()),
            started_at: chrono::Utc::now(),
            completed_at: None,
            status: TaskStatus::Active,
            risk_level: None,
            summary: None,
            cwd: Some(repo_root.to_string_lossy().to_string()),
        };
        db.create_task(&task).unwrap();

        Self {
            _dir: dir,
            repo_root,
            db,
            objects_root,
            task_id,
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.repo_root.join(rel)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn remove(&self, rel: &str) {
        let _ = fs::remove_file(self.path(rel));
    }

    fn rename(&self, from: &str, to: &str) {
        let from_path = self.path(from);
        let to_path = self.path(to);
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::rename(from_path, to_path).unwrap();
    }

    fn copy(&self, from: &str, to: &str) {
        let to_path = self.path(to);
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::copy(self.path(from), to_path).unwrap();
    }

    fn git_stdout(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn git_add_all(&self) {
        self.git_stdout(&["add", "-A"]);
    }

    fn git_commit_all(&self, message: &str) {
        self.git_add_all();
        let output = Command::new("git")
            .args(["commit", "--allow-empty", "-qm", message])
            .current_dir(&self.repo_root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_tracked_files(&self) -> Vec<String> {
        self.git_stdout(&["ls-files"])
            .lines()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn seed_baseline_from_git(&self) {
        let store = ObjectStore::new(self.objects_root.clone()).unwrap();
        for rel in self.git_tracked_files() {
            let abs = self.path(&rel);
            let sha = store.store(&abs).unwrap();
            self.db
                .upsert_file_index(&abs.to_string_lossy(), 1, &sha, Some(&sha))
                .unwrap();
        }
    }

    fn setup_baseline(&self, files: &[(&str, &str)]) {
        for (rel, content) in files {
            self.write(rel, content);
        }
        self.git_commit_all("baseline");
        self.seed_baseline_from_git();
    }

    fn simulate_event(&self, rel: &str, event_kind: FileEventKind) {
        let path = self.path(rel);
        let baseline = resolve_baseline(&self.db, &self.task_id, &path, None);
        let new_hash = if path.exists() {
            let store = ObjectStore::new(self.objects_root.clone()).unwrap();
            Some(store.store(&path).unwrap())
        } else {
            None
        };

        let change_type = match event_kind {
            FileEventKind::Created => {
                if baseline.existed {
                    ChangeType::Modified
                } else {
                    ChangeType::Created
                }
            }
            FileEventKind::Modified => {
                if baseline.existed {
                    ChangeType::Modified
                } else {
                    ChangeType::Created
                }
            }
            FileEventKind::Deleted => ChangeType::Deleted,
            FileEventKind::Renamed => {
                if path.exists() {
                    if baseline.existed {
                        ChangeType::Renamed
                    } else {
                        ChangeType::Created
                    }
                } else {
                    ChangeType::Deleted
                }
            }
        };

        let (old_hash, final_new_hash) = match change_type {
            ChangeType::Created => (None, new_hash),
            ChangeType::Modified | ChangeType::Renamed => (baseline.hash, new_hash),
            ChangeType::Deleted => {
                if baseline.existed {
                    (baseline.hash, None)
                } else {
                    (None, None)
                }
            }
        };

        let change = Change {
            id: None,
            task_id: self.task_id.clone(),
            file_path: path,
            change_type,
            old_hash,
            new_hash: final_new_hash,
            diff_text: None,
            lines_added: 0,
            lines_removed: 0,
            restored_at: None,
            attribution: Some("test".into()),
            old_file_path: None,
        };
        self.db.upsert_change(&change).unwrap();
    }

    fn finalize_rew(&self) -> Vec<Change> {
        reconcile_task(&self.db, &self.task_id, &self.objects_root).unwrap();
        self.db.get_changes_for_task(&self.task_id).unwrap()
    }

    fn git_name_status(&self) -> Vec<NormChange> {
        self.git_add_all();
        self.git_stdout(&["diff", "--cached", "--name-status", "-M"])
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                let parts: Vec<_> = line.split('\t').collect();
                let code = parts[0];
                let kind = code.chars().next().unwrap();
                if kind == 'R' {
                    NormChange {
                        kind: 'R',
                        old_path: Some(parts[1].to_string()),
                        path: parts[2].to_string(),
                    }
                } else {
                    NormChange {
                        kind,
                        old_path: None,
                        path: parts[1].to_string(),
                    }
                }
            })
            .collect()
    }

    fn git_status_porcelain(&self) -> Vec<NormChange> {
        self.git_add_all();
        self.git_stdout(&["status", "--porcelain=v1"])
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                let kind = line.chars().next().unwrap();
                let rest = line[3..].to_string();
                if kind == 'R' {
                    let parts: Vec<_> = rest.split(" -> ").collect();
                    NormChange {
                        kind: 'R',
                        old_path: Some(parts[0].to_string()),
                        path: parts[1].to_string(),
                    }
                } else {
                    NormChange {
                        kind,
                        old_path: None,
                        path: rest,
                    }
                }
            })
            .collect()
    }

    fn git_numstat_totals(&self) -> (u32, u32) {
        self.git_add_all();
        let mut added = 0u32;
        let mut removed = 0u32;
        for line in self.git_stdout(&["diff", "--cached", "--numstat", "-M"]).lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parts: Vec<_> = line.split('\t').collect();
            added += parts[0].parse::<u32>().unwrap_or(0);
            removed += parts[1].parse::<u32>().unwrap_or(0);
        }
        (added, removed)
    }

    fn rew_norm_changes(&self, changes: &[Change]) -> Vec<NormChange> {
        changes
            .iter()
            .map(|c| NormChange {
                kind: match c.change_type {
                    ChangeType::Created => 'A',
                    ChangeType::Modified => 'M',
                    ChangeType::Deleted => 'D',
                    ChangeType::Renamed => 'R',
                },
                path: self.rel(&c.file_path),
                old_path: c.old_file_path.as_ref().map(|p| self.rel(p)),
            })
            .collect()
    }

    fn rew_numstat_totals(&self, changes: &[Change]) -> (u32, u32) {
        (
            changes.iter().map(|c| c.lines_added).sum(),
            changes.iter().map(|c| c.lines_removed).sum(),
        )
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.repo_root)
            .unwrap()
            .to_string_lossy()
            .to_string()
    }
}

fn sorted(mut changes: Vec<NormChange>) -> Vec<NormChange> {
    changes.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.old_path.cmp(&b.old_path))
            .then_with(|| a.path.cmp(&b.path))
    });
    changes
}

fn assert_git_aligned(env: &GitRepoEnv, rew_changes: &[Change]) {
    let git_name_status = sorted(env.git_name_status());
    let git_status = sorted(env.git_status_porcelain());
    let rew_norm = sorted(env.rew_norm_changes(rew_changes));

    assert_eq!(git_name_status, git_status, "git name-status and porcelain diverged");
    assert_eq!(rew_norm, git_name_status, "rew final semantics diverged from git");
    assert_eq!(
        env.rew_numstat_totals(rew_changes),
        env.git_numstat_totals(),
        "rew line totals diverged from git numstat"
    );
}

#[test]
fn git_modify_existing_file() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "a\nb\nc\n")]);

    env.write("a.txt", "a\nb changed\nc\nd\n");
    env.simulate_event("a.txt", FileEventKind::Modified);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_delete_existing_file() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "a\nb\nc\n")]);

    env.remove("a.txt");
    env.simulate_event("a.txt", FileEventKind::Deleted);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_create_new_file() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[]);

    env.write("new.txt", "hello\nworld\n");
    env.simulate_event("new.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_create_then_delete_net_zero() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[]);

    env.write("temp.txt", "temp\n");
    env.simulate_event("temp.txt", FileEventKind::Created);
    env.remove("temp.txt");
    env.simulate_event("temp.txt", FileEventKind::Deleted);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_modify_then_revert_net_zero() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "same\ncontent\n")]);

    env.write("a.txt", "different\ncontent\n");
    env.simulate_event("a.txt", FileEventKind::Modified);
    env.write("a.txt", "same\ncontent\n");

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_delete_then_recreate_same_content() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "same\ncontent\n")]);

    env.remove("a.txt");
    env.simulate_event("a.txt", FileEventKind::Deleted);
    env.write("a.txt", "same\ncontent\n");
    env.simulate_event("a.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_delete_then_recreate_different_content() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "same\ncontent\n")]);

    env.remove("a.txt");
    env.simulate_event("a.txt", FileEventKind::Deleted);
    env.write("a.txt", "new\ncontent\nmore\n");
    env.simulate_event("a.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_pure_rename() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "line1\nline2\n")]);

    env.rename("a.txt", "b.txt");
    env.simulate_event("a.txt", FileEventKind::Deleted);
    env.simulate_event("b.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_rename_then_small_edit() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n")]);

    env.rename("a.txt", "b.txt");
    env.write("b.txt", "1\n2\n3\n4\n5 changed\n6\n7\n8\n9\n10\n11\n");
    env.simulate_event("a.txt", FileEventKind::Deleted);
    env.simulate_event("b.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_rename_then_large_edit() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[(
        "a.txt",
        "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n",
    )]);

    env.rename("a.txt", "b.txt");
    env.write(
        "b.txt",
        "x1\nx2\nx3\nx4\nx5\nx6\nx7\nx8\nx9\nx10\nx11\nx12\nx13\nx14\nx15\nx16\nx17\n18\n19\n20\n",
    );
    env.simulate_event("a.txt", FileEventKind::Deleted);
    env.simulate_event("b.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_copy_then_modify() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "base\nline\n")]);

    env.copy("a.txt", "b.txt");
    env.write("b.txt", "base\nline\nextra\n");
    env.simulate_event("b.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_copy_rename_then_modify() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "base\nline\n")]);

    env.copy("a.txt", "b.txt");
    env.simulate_event("b.txt", FileEventKind::Created);
    env.rename("b.txt", "c.txt");
    env.simulate_event("b.txt", FileEventKind::Deleted);
    env.write("c.txt", "base\nline\nextra\n");
    env.simulate_event("c.txt", FileEventKind::Created);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_copy_then_delete() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "base\nline\n")]);

    env.copy("a.txt", "b.txt");
    env.simulate_event("b.txt", FileEventKind::Created);
    env.remove("b.txt");
    env.simulate_event("b.txt", FileEventKind::Deleted);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}

#[test]
fn git_copy_rename_then_delete() {
    let env = GitRepoEnv::new();
    env.setup_baseline(&[("a.txt", "base\nline\n")]);

    env.copy("a.txt", "b.txt");
    env.simulate_event("b.txt", FileEventKind::Created);
    env.rename("b.txt", "c.txt");
    env.simulate_event("b.txt", FileEventKind::Deleted);
    env.simulate_event("c.txt", FileEventKind::Created);
    env.remove("c.txt");
    env.simulate_event("c.txt", FileEventKind::Deleted);

    let changes = env.finalize_rew();
    assert_git_aligned(&env, &changes);
}
