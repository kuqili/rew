//! Task-end reconciliation: verify and correct change records.
//!
//! When a task completes, reconcile ensures that each change record
//! reflects the **net** difference between the file's state at task
//! start (old_hash) and its actual state on disk at task end.
//!
//! Operations:
//! - **Net-zero removal**: old_hash == current hash → no real change → delete record
//! - **Type correction**: adjust change_type based on actual old/new state
//! - **Line recount**: recalculate lines_added/removed from actual content
//! - **Rename pairing** (Phase 3): match Deleted+Created into Renamed

use crate::db::Database;
use crate::error::RewResult;
use crate::objects::{sha256_file, ObjectStore};
use crate::types::ChangeType;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use tracing::debug;

/// Match git's default-ish rename behavior closely enough for product usage:
/// pure rename always pairs, and rename+partial-edit pairs once similarity
/// reaches this threshold.
const RENAME_SIMILARITY_THRESHOLD: u32 = 50;
const RENAME_SIZE_RATIO_MIN: f64 = 0.30;
const MAX_DP_CREATED_NODES: usize = 18;

pub struct ReconcileResult {
    pub removed: usize,
    pub updated: usize,
    pub renames_paired: usize,
}

/// Reconcile all change records for a completed task.
///
/// Reads the actual file state from disk and compares to each record's
/// old_hash.  Records where old == current (net-zero) are deleted.
/// Remaining records get their change_type, new_hash, and line stats
/// corrected to reflect the true final state.
pub fn reconcile_task(
    db: &Database,
    task_id: &str,
    objects_root: &Path,
) -> RewResult<ReconcileResult> {
    let changes = db.get_changes_for_task(task_id)?;
    let obj_store = ObjectStore::new(objects_root.to_path_buf())?;

    let mut removed = 0usize;
    let mut updated = 0usize;

    for change in &changes {
        let change_id = match change.id {
            Some(id) => id,
            None => continue,
        };

        let current_hash = if change.file_path.exists() {
            sha256_file(&change.file_path).ok()
        } else {
            None
        };

        // Store the current file content in object store if it exists
        if change.file_path.exists() {
            let _ = obj_store.store(&change.file_path);
        }

        match (&change.old_hash, &current_hash) {
            // Net-zero: file unchanged from baseline → delete record
            (Some(old), Some(cur)) if old == cur => {
                debug!(
                    "reconcile: removing net-zero change for {}",
                    change.file_path.display()
                );
                db.delete_change_by_id(change_id)?;
                removed += 1;
            }
            // Temporary: created then deleted within the task → delete record
            (None, None) => {
                debug!(
                    "reconcile: removing ephemeral change for {}",
                    change.file_path.display()
                );
                db.delete_change_by_id(change_id)?;
                removed += 1;
            }
            // File was created (no baseline, now exists)
            (None, Some(_cur)) => {
                let (added, removed_lines) = compute_lines(&obj_store, None, current_hash.as_deref());
                if change.change_type != ChangeType::Created
                    || change.new_hash.as_deref() != current_hash.as_deref()
                    || change.lines_added != added
                    || change.lines_removed != removed_lines
                {
                    db.update_change_reconciled(
                        change_id,
                        &ChangeType::Created,
                        current_hash.as_deref(),
                        added,
                        removed_lines,
                    )?;
                    updated += 1;
                }
            }
            // File was deleted (had baseline, now gone)
            (Some(_old), None) => {
                let (added, removed_lines) = compute_lines(&obj_store, change.old_hash.as_deref(), None);
                if change.change_type != ChangeType::Deleted
                    || change.new_hash.is_some()
                    || change.lines_added != added
                    || change.lines_removed != removed_lines
                {
                    db.update_change_reconciled(
                        change_id,
                        &ChangeType::Deleted,
                        None,
                        added,
                        removed_lines,
                    )?;
                    updated += 1;
                }
            }
            // File was modified (baseline differs from current)
            (Some(_old), Some(_cur)) => {
                let (added, removed_lines) = compute_lines(
                    &obj_store,
                    change.old_hash.as_deref(),
                    current_hash.as_deref(),
                );
                if change.change_type != ChangeType::Modified
                    || change.new_hash.as_deref() != current_hash.as_deref()
                    || change.lines_added != added
                    || change.lines_removed != removed_lines
                {
                    db.update_change_reconciled(
                        change_id,
                        &ChangeType::Modified,
                        current_hash.as_deref(),
                        added,
                        removed_lines,
                    )?;
                    updated += 1;
                }
            }
        }
    }

    // Phase 3: Rename pairing — match Deleted+Created with identical content hash
    let renames_paired = pair_renames(db, task_id, &obj_store)?;

    Ok(ReconcileResult {
        removed,
        updated,
        renames_paired,
    })
}

#[derive(Clone)]
struct RenameCandidate {
    deleted_id: i64,
    created_id: i64,
    old_path: String,
    old_hash: String,
    score: u32,
    weight: i64,
    lines_added: u32,
    lines_removed: u32,
}

#[derive(Default)]
struct ObjectInfo {
    size: Option<u64>,
    bytes: Option<Vec<u8>>,
    text_like: Option<bool>,
}

/// Pair Deleted+Created changes into Renamed.
///
/// Pairing strategy:
/// 1. Exact hash match (pure rename) first.
/// 2. Only for the remaining items, build cheap rename candidates.
/// 3. Expensive text similarity runs only after cheap filters pass.
/// 4. Select the best non-conflicting set per connected component.
fn pair_renames(
    db: &Database,
    task_id: &str,
    obj_store: &ObjectStore,
) -> RewResult<usize> {
    let changes = db.get_changes_for_task(task_id)?;

    let deleted: Vec<_> = changes
        .iter()
        .filter(|c| c.change_type == ChangeType::Deleted && c.old_hash.is_some() && c.id.is_some())
        .collect();
    let created: Vec<_> = changes
        .iter()
        .filter(|c| c.change_type == ChangeType::Created && c.new_hash.is_some() && c.id.is_some())
        .collect();

    if deleted.is_empty() || created.is_empty() {
        return Ok(0);
    }

    let mut paired = 0usize;
    let mut used_created: HashSet<i64> = HashSet::new();
    let mut used_deleted: HashSet<i64> = HashSet::new();

    // Stage 1: exact hash pairing — cheapest and most reliable.
    let mut exact_created_by_hash: HashMap<&str, Vec<&crate::types::Change>> = HashMap::new();
    for c in &created {
        exact_created_by_hash
            .entry(c.new_hash.as_ref().unwrap().as_str())
            .or_default()
            .push(*c);
    }

    for d in &deleted {
        let d_id = d.id.unwrap();
        let d_hash = d.old_hash.as_ref().unwrap();

        let Some(bucket) = exact_created_by_hash.get_mut(d_hash.as_str()) else {
            continue;
        };
        if bucket.is_empty() {
            continue;
        }

        let best_idx = bucket
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| cheap_name_score(&d.file_path, &c.file_path))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let created_change = bucket.swap_remove(best_idx);
        let created_id = created_change.id.unwrap();

        db.update_change_rename_paired(
            created_id,
            &d.file_path.to_string_lossy(),
            Some(d_hash.as_str()),
            0,
            0,
        )?;
        db.delete_change_by_id(d_id)?;

        used_created.insert(created_id);
        used_deleted.insert(d_id);
        paired += 1;
    }

    let remaining_deleted: Vec<_> = deleted
        .into_iter()
        .filter(|d| !used_deleted.contains(&d.id.unwrap()))
        .collect();
    let remaining_created: Vec<_> = created
        .into_iter()
        .filter(|c| !used_created.contains(&c.id.unwrap()))
        .collect();

    if remaining_deleted.is_empty() || remaining_created.is_empty() {
        return Ok(paired);
    }

    let mut object_cache: HashMap<String, ObjectInfo> = HashMap::new();
    let mut candidates: Vec<RenameCandidate> = Vec::new();

    for d in &remaining_deleted {
        let d_id = d.id.unwrap();
        let d_hash = d.old_hash.as_ref().unwrap();

        for c in &remaining_created {
            let c_id = c.id.unwrap();
            let c_hash = c.new_hash.as_ref().unwrap();

            if !extensions_compatible(&d.file_path, &c.file_path) {
                continue;
            }

            let name_score = cheap_name_score(&d.file_path, &c.file_path);

            let Some(old_size) = object_size(obj_store, &mut object_cache, d_hash) else {
                continue;
            };
            let Some(new_size) = object_size(obj_store, &mut object_cache, c_hash) else {
                continue;
            };
            if !size_ratio_ok(old_size, new_size) {
                continue;
            }

            let old_is_text = is_text_like(obj_store, &mut object_cache, d_hash).unwrap_or(false);
            let new_is_text = is_text_like(obj_store, &mut object_cache, c_hash).unwrap_or(false);
            if !old_is_text || !new_is_text {
                continue;
            }

            let old_bytes = match object_bytes(obj_store, &mut object_cache, d_hash).cloned() {
                Some(b) => b,
                None => continue,
            };
            let new_bytes = match object_bytes(obj_store, &mut object_cache, c_hash).cloned() {
                Some(b) => b,
                None => continue,
            };

            let Some(score) = crate::diff::similarity_score(&old_bytes, &new_bytes) else {
                continue;
            };
            if score < RENAME_SIMILARITY_THRESHOLD {
                continue;
            }

            let (lines_added, lines_removed) =
                crate::diff::count_changed_lines(&old_bytes, &new_bytes);
            let weight = (score as i64) * 1_000 + (name_score as i64);

            candidates.push(RenameCandidate {
                deleted_id: d_id,
                created_id: c_id,
                old_path: d.file_path.to_string_lossy().to_string(),
                old_hash: d_hash.clone(),
                score,
                weight,
                lines_added,
                lines_removed,
            });
        }
    }

    if candidates.is_empty() {
        return Ok(paired);
    }

    for candidate in select_best_candidate_set(&candidates) {
        db.update_change_rename_paired(
            candidate.created_id,
            &candidate.old_path,
            Some(candidate.old_hash.as_str()),
            candidate.lines_added,
            candidate.lines_removed,
        )?;
        db.delete_change_by_id(candidate.deleted_id)?;
        paired += 1;

        debug!(
            "reconcile: paired rename {} → created#{} (score={})",
            candidate.old_path,
            candidate.created_id,
            candidate.score,
        );
    }

    Ok(paired)
}

fn select_best_candidate_set(candidates: &[RenameCandidate]) -> Vec<RenameCandidate> {
    let mut deleted_to_candidates: HashMap<i64, Vec<usize>> = HashMap::new();
    let mut created_to_candidates: HashMap<i64, Vec<usize>> = HashMap::new();

    for (idx, c) in candidates.iter().enumerate() {
        deleted_to_candidates.entry(c.deleted_id).or_default().push(idx);
        created_to_candidates.entry(c.created_id).or_default().push(idx);
    }

    let mut visited_deleted: HashSet<i64> = HashSet::new();
    let mut selected = Vec::new();

    for deleted_id in deleted_to_candidates.keys().copied().collect::<Vec<_>>() {
        if visited_deleted.contains(&deleted_id) {
            continue;
        }

        let mut queue = VecDeque::from([deleted_id]);
        let mut component_deleted = HashSet::new();
        let mut component_created = HashSet::new();
        let mut component_candidate_ids = HashSet::new();

        while let Some(did) = queue.pop_front() {
            if !component_deleted.insert(did) {
                continue;
            }
            visited_deleted.insert(did);

            if let Some(edge_ids) = deleted_to_candidates.get(&did) {
                for &edge_id in edge_ids {
                    component_candidate_ids.insert(edge_id);
                    let cid = candidates[edge_id].created_id;
                    if component_created.insert(cid) {
                        if let Some(created_edge_ids) = created_to_candidates.get(&cid) {
                            for &created_edge_id in created_edge_ids {
                                component_candidate_ids.insert(created_edge_id);
                                queue.push_back(candidates[created_edge_id].deleted_id);
                            }
                        }
                    }
                }
            }
        }

        let component_candidates: Vec<RenameCandidate> = component_candidate_ids
            .into_iter()
            .map(|idx| candidates[idx].clone())
            .collect();
        let component_created_vec: Vec<i64> = component_created.into_iter().collect();

        let component_best = if component_created_vec.len() <= MAX_DP_CREATED_NODES {
            solve_component_exact(&component_candidates, &component_created_vec)
        } else {
            solve_component_greedy(&component_candidates)
        };

        selected.extend(component_best);
    }

    selected
}

fn solve_component_greedy(candidates: &[RenameCandidate]) -> Vec<RenameCandidate> {
    let mut ordered = candidates.to_vec();
    ordered.sort_by(|a, b| {
        b.weight
            .cmp(&a.weight)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.lines_added.cmp(&b.lines_added))
            .then_with(|| a.lines_removed.cmp(&b.lines_removed))
    });

    let mut used_deleted = HashSet::new();
    let mut used_created = HashSet::new();
    let mut selected = Vec::new();

    for c in ordered {
        if used_deleted.contains(&c.deleted_id) || used_created.contains(&c.created_id) {
            continue;
        }
        used_deleted.insert(c.deleted_id);
        used_created.insert(c.created_id);
        selected.push(c);
    }

    selected
}

fn solve_component_exact(
    candidates: &[RenameCandidate],
    created_ids: &[i64],
) -> Vec<RenameCandidate> {
    let mut created_index = HashMap::new();
    for (idx, cid) in created_ids.iter().enumerate() {
        created_index.insert(*cid, idx);
    }

    let mut deleted_ids: Vec<i64> = candidates.iter().map(|c| c.deleted_id).collect();
    deleted_ids.sort_unstable();
    deleted_ids.dedup();

    let per_deleted: Vec<Vec<RenameCandidate>> = deleted_ids
        .iter()
        .map(|did| {
            let mut edges: Vec<RenameCandidate> = candidates
                .iter()
                .filter(|c| c.deleted_id == *did)
                .cloned()
                .collect();
            edges.sort_by(|a, b| b.weight.cmp(&a.weight));
            edges
        })
        .collect();

    let mut memo: HashMap<(usize, u64), i64> = HashMap::new();
    fn best_score(
        pos: usize,
        used_mask: u64,
        per_deleted: &[Vec<RenameCandidate>],
        created_index: &HashMap<i64, usize>,
        memo: &mut HashMap<(usize, u64), i64>,
    ) -> i64 {
        if pos >= per_deleted.len() {
            return 0;
        }
        if let Some(v) = memo.get(&(pos, used_mask)) {
            return *v;
        }

        let mut best = best_score(pos + 1, used_mask, per_deleted, created_index, memo);
        for edge in &per_deleted[pos] {
            let bit = 1u64 << created_index[&edge.created_id];
            if used_mask & bit != 0 {
                continue;
            }
            let score =
                edge.weight + best_score(pos + 1, used_mask | bit, per_deleted, created_index, memo);
            best = best.max(score);
        }

        memo.insert((pos, used_mask), best);
        best
    }

    let _ = best_score(0, 0, &per_deleted, &created_index, &mut memo);

    let mut selected = Vec::new();
    let mut used_mask = 0u64;
    for pos in 0..per_deleted.len() {
        let skip_score = best_score(pos + 1, used_mask, &per_deleted, &created_index, &mut memo);
        let current_best = *memo.get(&(pos, used_mask)).unwrap_or(&skip_score);
        if current_best == skip_score {
            continue;
        }

        for edge in &per_deleted[pos] {
            let bit = 1u64 << created_index[&edge.created_id];
            if used_mask & bit != 0 {
                continue;
            }
            let score =
                edge.weight + best_score(pos + 1, used_mask | bit, &per_deleted, &created_index, &mut memo);
            if score == current_best {
                selected.push(edge.clone());
                used_mask |= bit;
                break;
            }
        }
    }

    selected
}

fn object_size(
    obj_store: &ObjectStore,
    cache: &mut HashMap<String, ObjectInfo>,
    hash: &str,
) -> Option<u64> {
    let entry = cache.entry(hash.to_string()).or_default();
    if entry.size.is_none() {
        let path = obj_store.retrieve(hash)?;
        entry.size = std::fs::metadata(path).ok().map(|m| m.len());
    }
    entry.size
}

fn object_bytes<'a>(
    obj_store: &ObjectStore,
    cache: &'a mut HashMap<String, ObjectInfo>,
    hash: &str,
) -> Option<&'a Vec<u8>> {
    let entry = cache.entry(hash.to_string()).or_default();
    if entry.bytes.is_none() {
        let path = obj_store.retrieve(hash)?;
        entry.bytes = std::fs::read(path).ok();
    }
    entry.bytes.as_ref()
}

fn is_text_like(
    obj_store: &ObjectStore,
    cache: &mut HashMap<String, ObjectInfo>,
    hash: &str,
) -> Option<bool> {
    if let Some(v) = cache.get(hash).and_then(|entry| entry.text_like) {
        return Some(v);
    }
    let bytes = object_bytes(obj_store, cache, hash)?.clone();
    let is_text = !bytes[..bytes.len().min(8192)].contains(&0u8);
    cache.entry(hash.to_string()).or_default().text_like = Some(is_text);
    Some(is_text)
}

fn extensions_compatible(old_path: &Path, new_path: &Path) -> bool {
    match (old_path.extension(), new_path.extension()) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

fn size_ratio_ok(old_size: u64, new_size: u64) -> bool {
    if old_size == 0 || new_size == 0 {
        return old_size == new_size;
    }
    let min_size = old_size.min(new_size) as f64;
    let max_size = old_size.max(new_size) as f64;
    (min_size / max_size) >= RENAME_SIZE_RATIO_MIN
}

fn cheap_name_score(old_path: &Path, new_path: &Path) -> u32 {
    let old_name = old_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let new_name = new_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if old_name.is_empty() || new_name.is_empty() {
        return 0;
    }
    if old_name == new_name {
        return 100;
    }
    if old_name.contains(&new_name) || new_name.contains(&old_name) {
        return 85;
    }

    let prefix_len = old_name
        .chars()
        .zip(new_name.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let prefix_score = ((prefix_len * 100) / old_name.len().max(new_name.len())) as u32;

    let old_tokens = tokenize_name(&old_name);
    let new_tokens = tokenize_name(&new_name);
    let common_tokens = old_tokens.intersection(&new_tokens).count();
    let token_score = if old_tokens.is_empty() || new_tokens.is_empty() {
        0
    } else {
        ((common_tokens * 100) / old_tokens.len().max(new_tokens.len())) as u32
    };

    prefix_score.max(token_score)
}

fn tokenize_name(name: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut current = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            out.insert(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.insert(current);
    }
    out
}

/// Compute (lines_added, lines_removed) from object store content.
fn compute_lines(
    obj_store: &ObjectStore,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
) -> (u32, u32) {
    let read_content = |hash: &str| -> Option<Vec<u8>> {
        let path = obj_store.retrieve(hash)?;
        std::fs::read(path).ok()
    };

    let old_bytes = old_hash.and_then(read_content).unwrap_or_default();
    let new_bytes = new_hash.and_then(read_content).unwrap_or_default();

    crate::diff::count_changed_lines(&old_bytes, &new_bytes)
}
