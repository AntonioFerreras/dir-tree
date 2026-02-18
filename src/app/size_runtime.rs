//! Background size computation runtime.
//!
//! This module owns worker orchestration and cascade finalization while
//! keeping low-level filesystem math in `core::size`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::app::state::AppState;
use crate::core::size::{self, classify_file, get_dev, is_same_device, DirLocalResult, InodeMap};

#[derive(Debug)]
pub enum SizeUpdate {
    File { path: PathBuf, size: u64 },
    DirLocalDone {
        dir: PathBuf,
        unique_sum: u64,
        hardlinks: InodeMap,
    },
    WorkerDone,
}

/// Shared read-only context available to every worker thread.
struct WorkerCtx {
    /// Paths of directories that are nodes in the display tree.
    /// Workers skip these during their walk because the cascade
    /// handles them separately.
    tree_dirs: HashSet<PathBuf>,
    /// Whether hard-link dedup is enabled.
    dedup_hard_links: bool,
    /// When `true`, don't descend into directories on a different device.
    one_file_system: bool,
    /// Device ID of the root directory (for `one_file_system` checks).
    root_dev: u64,
}

pub struct SizeComputeState {
    generation: u64,
    remaining_workers: usize,
    /// Tree directory nodes sorted deepest-first for O(n) cascade.
    dirs: Vec<PathBuf>,
    parent_dir: HashMap<PathBuf, Option<PathBuf>>,
    pending_children: HashMap<PathBuf, usize>,
    /// Per-dir: accumulated unique_sum from tree-children.
    children_unique: HashMap<PathBuf, u64>,
    /// Per-dir: merged hardlink maps from tree-children.
    children_hardlinks: HashMap<PathBuf, InodeMap>,
    /// Per-dir: the local walk result (unique_sum + hardlinks).
    local_done: HashMap<PathBuf, DirLocalResult>,
    finished: HashSet<PathBuf>,
    /// Shared flag used to signal worker threads to stop early.
    cancel: Arc<AtomicBool>,
}

impl SizeComputeState {
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_scanning(&self) -> bool {
        self.remaining_workers > 0
    }
}

pub fn start_size_computation(
    state: &mut AppState,
    tx: &tokio::sync::mpsc::UnboundedSender<(u64, SizeUpdate)>,
) -> SizeComputeState {
    state.size_compute_generation = state.size_compute_generation.wrapping_add(1);
    let generation = state.size_compute_generation;

    let cancel = Arc::new(AtomicBool::new(false));

    // Build a set of all directory paths that are nodes in the display tree.
    let mut tree_dirs = HashSet::new();
    for node in &state.tree.nodes {
        if node.meta.is_dir {
            tree_dirs.insert(node.meta.path.clone());
        }
    }

    let mut dirs = Vec::new();
    let mut dir_depth: HashMap<PathBuf, usize> = HashMap::new();
    let mut parent_dir: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    let mut pending_children: HashMap<PathBuf, usize> = HashMap::new();
    let mut children_unique: HashMap<PathBuf, u64> = HashMap::new();
    let mut children_hardlinks: HashMap<PathBuf, InodeMap> = HashMap::new();
    let mut local_done: HashMap<PathBuf, DirLocalResult> = HashMap::new();
    let mut jobs: VecDeque<PathBuf> = VecDeque::new();

    for node in &state.tree.nodes {
        if !node.meta.is_dir {
            continue;
        }

        let dir_path = node.meta.path.clone();
        dirs.push(dir_path.clone());
        dir_depth.insert(dir_path.clone(), node.depth);

        let parent_path = node.parent.and_then(|pid| {
            let p = &state.tree.nodes[pid];
            if p.meta.is_dir {
                Some(p.meta.path.clone())
            } else {
                None
            }
        });
        parent_dir.insert(dir_path.clone(), parent_path);

        let child_dir_count = node
            .children
            .iter()
            .filter(|&&cid| state.tree.nodes[cid].meta.is_dir)
            .count();

        pending_children.insert(dir_path.clone(), child_dir_count);
        children_unique.insert(dir_path.clone(), 0);
        children_hardlinks.insert(dir_path.clone(), InodeMap::new());

        // Reuse cached local result if available.
        if let Some(cached) = state.dir_local_sums.get(&dir_path) {
            local_done.insert(dir_path, cached.clone());
        } else {
            jobs.push_back(dir_path);
        }
    }

    // Sort dirs deepest-first for O(n) cascade finalization.
    dirs.sort_by(|a, b| {
        let da = dir_depth.get(a).copied().unwrap_or(0);
        let db = dir_depth.get(b).copied().unwrap_or(0);
        db.cmp(&da)
    });

    let queue = Arc::new(Mutex::new(jobs));
    let dedup_hard_links = state.config.dedup_hard_links;
    let one_file_system = state.config.one_file_system;
    let root_dev = get_dev(&state.cwd);
    let ctx = Arc::new(WorkerCtx {
        tree_dirs,
        dedup_hard_links,
        one_file_system,
        root_dev,
    });

    let max_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);

    let job_count = queue.lock().ok().map_or(0, |q| q.len());
    let worker_count = max_threads.min(job_count.max(1));

    if job_count > 0 {
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let cancel = Arc::clone(&cancel);
            let ctx = Arc::clone(&ctx);
            std::thread::spawn(move || {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }

                    let dir = {
                        let mut q = match queue.lock() {
                            Ok(guard) => guard,
                            Err(_) => break,
                        };
                        match q.pop_front() {
                            Some(d) => d,
                            None => break,
                        }
                    };

                    let entries = match std::fs::read_dir(&dir) {
                        Ok(e) => e,
                        Err(_) => {
                            let _ = tx.send((
                                generation,
                                SizeUpdate::DirLocalDone {
                                    dir,
                                    unique_sum: 0,
                                    hardlinks: InodeMap::new(),
                                },
                            ));
                            continue;
                        }
                    };

                    let mut unique_sum: u64 = 0;
                    let mut hardlinks = InodeMap::new();

                    for entry in entries.flatten() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let ft = match entry.file_type() {
                            Ok(ft) => ft,
                            Err(_) => continue,
                        };
                        let path = entry.path();

                        if ft.is_file() {
                            if let Ok(meta) = entry.metadata() {
                                let s = meta.len();
                                let _ = tx.send((
                                    generation,
                                    SizeUpdate::File {
                                        path: path.clone(),
                                        size: s,
                                    },
                                ));
                                let (size, inode_key) = classify_file(&meta, ctx.dedup_hard_links);
                                match inode_key {
                                    None => unique_sum = unique_sum.saturating_add(size),
                                    Some(key) => {
                                        hardlinks.entry(key).or_insert(size);
                                    }
                                }
                            }
                        } else if ft.is_dir() {
                            if ctx.tree_dirs.contains(&path) {
                                // Tree child dir — cascade handles it.
                            } else if ctx.one_file_system {
                                // Check mount boundary before descending.
                                if let Ok(meta) = std::fs::metadata(&path) {
                                    if is_same_device(&meta, ctx.root_dev) {
                                        let (sub_unique, sub_hardlinks) = size::recursive_dir_size(
                                            &path,
                                            &cancel,
                                            ctx.dedup_hard_links,
                                            true,
                                            ctx.root_dev,
                                        );
                                        unique_sum = unique_sum.saturating_add(sub_unique);
                                        for (k, v) in sub_hardlinks {
                                            hardlinks.entry(k).or_insert(v);
                                        }
                                    }
                                }
                            } else {
                                let (sub_unique, sub_hardlinks) = size::recursive_dir_size(
                                    &path,
                                    &cancel,
                                    ctx.dedup_hard_links,
                                    false,
                                    0,
                                );
                                unique_sum = unique_sum.saturating_add(sub_unique);
                                for (k, v) in sub_hardlinks {
                                    hardlinks.entry(k).or_insert(v);
                                }
                            }
                        } else if ft.is_symlink() {
                            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                                let s = meta.len();
                                let _ = tx.send((
                                    generation,
                                    SizeUpdate::File {
                                        path: path.clone(),
                                        size: s,
                                    },
                                ));
                                unique_sum = unique_sum.saturating_add(s);
                            }
                        }
                    }

                    let _ = tx.send((
                        generation,
                        SizeUpdate::DirLocalDone {
                            dir,
                            unique_sum,
                            hardlinks,
                        },
                    ));
                }

                let _ = tx.send((generation, SizeUpdate::WorkerDone));
            });
        }
    }

    SizeComputeState {
        generation,
        remaining_workers: if job_count > 0 { worker_count } else { 0 },
        dirs,
        parent_dir,
        pending_children,
        children_unique,
        children_hardlinks,
        local_done,
        finished: HashSet::new(),
        cancel,
    }
}

/// Process a single size update message.  Returns `true` if a `DirLocalDone`
/// was applied (meaning `finalize_ready_dirs` should be called afterward).
pub fn apply_size_update(
    state: &mut AppState,
    size_compute: &mut Option<SizeComputeState>,
    generation: u64,
    update: SizeUpdate,
) -> bool {
    if generation != state.size_compute_generation {
        return false;
    }
    let Some(ref mut compute) = size_compute else {
        return false;
    };
    if compute.generation != generation {
        return false;
    }
    match update {
        SizeUpdate::File { path, size } => {
            state.file_sizes.insert(path, size);
            false
        }
        SizeUpdate::DirLocalDone {
            dir,
            unique_sum,
            hardlinks,
        } => {
            let result = DirLocalResult {
                unique_sum,
                hardlinks,
            };
            // Cache for future recomputes.
            state.dir_local_sums.insert(dir.clone(), result.clone());
            compute.local_done.insert(dir, result);
            true
        }
        SizeUpdate::WorkerDone => {
            compute.remaining_workers = compute.remaining_workers.saturating_sub(1);
            false
        }
    }
}

/// O(n) cascade: process dirs deepest-first, merging hardlink maps bottom-up.
///
/// Each directory's total = unique_bytes + sum(hardlink_map.values()), where
/// hardlink_map is the union of the dir's own hardlinks and all children's
/// hardlink maps.  This means a hard-linked file counts independently in
/// each leaf directory, but is deduped in any common ancestor.
pub fn finalize_ready_dirs(state: &mut AppState, compute: &mut SizeComputeState) {
    for i in 0..compute.dirs.len() {
        let dir = compute.dirs[i].clone();
        if compute.finished.contains(&dir) {
            continue;
        }

        // Check readiness without removing yet.
        if !compute.local_done.contains_key(&dir) {
            continue;
        }
        let pending = *compute.pending_children.get(&dir).unwrap_or(&0);
        if pending != 0 {
            continue;
        }

        // Take ownership — no cloning.
        let local = compute.local_done.remove(&dir).expect("local_done checked");
        let children_unique = compute.children_unique.remove(&dir).unwrap_or(0);
        let children_hl = compute.children_hardlinks.remove(&dir).unwrap_or_default();

        let total_unique = local.unique_sum.saturating_add(children_unique);

        // Merge hardlink maps: pick the larger map as the base to minimise
        // insertions, then extend from the smaller one.
        let mut merged_hardlinks;
        if local.hardlinks.len() >= children_hl.len() {
            merged_hardlinks = local.hardlinks;
            for (k, v) in children_hl {
                merged_hardlinks.entry(k).or_insert(v);
            }
        } else {
            merged_hardlinks = children_hl;
            for (k, v) in local.hardlinks {
                merged_hardlinks.entry(k).or_insert(v);
            }
        }

        let hardlink_bytes: u64 = merged_hardlinks.values().sum();
        let total = total_unique.saturating_add(hardlink_bytes);

        state.dir_sizes.insert(dir.clone(), total);
        compute.finished.insert(dir.clone());

        // Propagate to parent — move the merged map, don't copy.
        if let Some(Some(parent)) = compute.parent_dir.get(&dir) {
            if let Some(remaining) = compute.pending_children.get_mut(parent) {
                *remaining = remaining.saturating_sub(1);
            }
            if let Some(sum) = compute.children_unique.get_mut(parent) {
                *sum = sum.saturating_add(total_unique);
            }
            // Merge into parent's children_hardlinks.  If the parent has
            // no accumulated map yet, just move ours in wholesale.
            let parent_hl = compute.children_hardlinks.entry(parent.clone()).or_default();
            if parent_hl.is_empty() {
                *parent_hl = merged_hardlinks;
            } else {
                for (k, v) in merged_hardlinks {
                    parent_hl.entry(k).or_insert(v);
                }
            }
        }
    }
}

