//! Benchmark the main algorithm stages on a real directory.
//!
//! Usage:
//!   cargo run --release --example bench_scan -- [PATH] [THREADS]
//!
//! Defaults to $HOME if no path is given. THREADS defaults to the number of
//! available cores; pass `1` to force single-threaded.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ───────────────────────────────────────── helpers ────────────

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    for &unit in UNITS {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} PiB")
}

// ───────────────────────────────────────── stage 1 ───────────

/// Stage 1: WalkBuilder tree walk (mirrors `build_tree`).
fn stage_tree_walk(root: &Path, max_depth: usize) -> (usize, usize, usize, usize, std::time::Duration) {
    let start = Instant::now();

    let walker = ignore::WalkBuilder::new(root)
        .max_depth(Some(max_depth))
        .hidden(false)
        .git_ignore(true)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    let mut entries = 0usize;
    let mut dirs = 0usize;
    let mut files = 0usize;
    let mut symlinks = 0usize;

    for entry in walker.flatten() {
        entries += 1;
        if let Some(ft) = entry.file_type() {
            if ft.is_dir() {
                dirs += 1;
            } else if ft.is_symlink() {
                symlinks += 1;
            } else {
                files += 1;
            }
        }
    }

    (entries, dirs, files, symlinks, start.elapsed())
}

// ───────────────────────────────────────── stage 2 ───────────

/// Stage 2: shallow readdir + stat for tree-visible dirs only.
fn stage_shallow_stat(tree_dirs: &[PathBuf]) -> (usize, u64, std::time::Duration) {
    let start = Instant::now();
    let mut files_statted = 0usize;
    let mut total_bytes: u64 = 0;

    for dir in tree_dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total_bytes = total_bytes.saturating_add(meta.len());
                    files_statted += 1;
                }
            } else if ft.is_symlink() {
                if let Ok(meta) = std::fs::symlink_metadata(&entry.path()) {
                    total_bytes = total_bytes.saturating_add(meta.len());
                    files_statted += 1;
                }
            }
        }
    }

    (files_statted, total_bytes, start.elapsed())
}

// ───────────────────────────────────────── stage 3 ───────────

/// Check if a file should be charged (hard-link dedup).
#[cfg(unix)]
fn should_charge(
    meta: &std::fs::Metadata,
    seen: &Mutex<HashSet<(u64, u64)>>,
) -> bool {
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() <= 1 {
        return true;
    }
    let key = (meta.dev(), meta.ino());
    seen.lock().unwrap_or_else(|e| e.into_inner()).insert(key)
}

#[cfg(not(unix))]
fn should_charge(_meta: &std::fs::Metadata, _seen: &Mutex<HashSet<(u64, u64)>>) -> bool {
    true
}

/// Single-threaded recursive walk (DFS), with optional dedup.
fn stage_recursive_walk_single(root: &Path, dedup: bool) -> (usize, usize, u64, std::time::Duration) {
    let start = Instant::now();
    let mut total: u64 = 0;
    let mut files_statted = 0usize;
    let mut dirs_visited = 0usize;
    let mut stack = vec![root.to_path_buf()];
    let seen = Mutex::new(HashSet::new());

    while let Some(current) = stack.pop() {
        dirs_visited += 1;
        let entries = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    files_statted += 1;
                    if !dedup || should_charge(&meta, &seen) {
                        total = total.saturating_add(meta.len());
                    }
                }
            } else if ft.is_symlink() {
                if let Ok(meta) = std::fs::symlink_metadata(&entry.path()) {
                    total = total.saturating_add(meta.len());
                    files_statted += 1;
                }
            }
        }
    }

    (files_statted, dirs_visited, total, start.elapsed())
}

/// Multi-threaded recursive walk with work-stealing queue + optional dedup.
fn stage_recursive_walk_multi(root: &Path, num_threads: usize, dedup: bool) -> (usize, usize, u64, std::time::Duration) {
    let start = Instant::now();

    let total_bytes = Arc::new(AtomicU64::new(0));
    let files_statted = Arc::new(AtomicUsize::new(0));
    let dirs_visited = Arc::new(AtomicUsize::new(0));
    let seen_inodes: Arc<Mutex<HashSet<(u64, u64)>>> = Arc::new(Mutex::new(HashSet::new()));

    let queue: Arc<Mutex<VecDeque<PathBuf>>> = Arc::new(Mutex::new(VecDeque::new()));
    queue.lock().unwrap().push_back(root.to_path_buf());

    let active = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut handles = Vec::new();
    for _ in 0..num_threads {
        let queue = Arc::clone(&queue);
        let total_bytes = Arc::clone(&total_bytes);
        let files_statted = Arc::clone(&files_statted);
        let dirs_visited = Arc::clone(&dirs_visited);
        let active = Arc::clone(&active);
        let done = Arc::clone(&done);
        let seen_inodes = Arc::clone(&seen_inodes);

        handles.push(std::thread::spawn(move || {
            loop {
                let dir = {
                    let mut q = queue.lock().unwrap();
                    q.pop_front()
                };

                let dir = match dir {
                    Some(d) => {
                        active.fetch_add(1, Ordering::SeqCst);
                        d
                    }
                    None => {
                        if active.load(Ordering::SeqCst) == 0 {
                            done.store(true, Ordering::SeqCst);
                            break;
                        }
                        if done.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::yield_now();
                        continue;
                    }
                };

                dirs_visited.fetch_add(1, Ordering::Relaxed);

                let entries = match std::fs::read_dir(&dir) {
                    Ok(e) => e,
                    Err(_) => {
                        active.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                };

                let mut local_bytes: u64 = 0;
                let mut local_files: usize = 0;
                let mut new_dirs: Vec<PathBuf> = Vec::new();

                for entry in entries.flatten() {
                    let ft = match entry.file_type() {
                        Ok(ft) => ft,
                        Err(_) => continue,
                    };
                    if ft.is_dir() {
                        new_dirs.push(entry.path());
                    } else if ft.is_file() {
                        if let Ok(meta) = entry.metadata() {
                            local_files += 1;
                            if !dedup || should_charge(&meta, &seen_inodes) {
                                local_bytes = local_bytes.saturating_add(meta.len());
                            }
                        }
                    } else if ft.is_symlink() {
                        if let Ok(meta) = std::fs::symlink_metadata(&entry.path()) {
                            local_bytes = local_bytes.saturating_add(meta.len());
                            local_files += 1;
                        }
                    }
                }

                if !new_dirs.is_empty() {
                    let mut q = queue.lock().unwrap();
                    for d in new_dirs {
                        q.push_back(d);
                    }
                }

                total_bytes.fetch_add(local_bytes, Ordering::Relaxed);
                files_statted.fetch_add(local_files, Ordering::Relaxed);
                active.fetch_sub(1, Ordering::SeqCst);
            }
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    let elapsed = start.elapsed();
    (
        files_statted.load(Ordering::Relaxed),
        dirs_visited.load(Ordering::Relaxed),
        total_bytes.load(Ordering::Relaxed),
        elapsed,
    )
}

// ───────────────────────────────────────── stage 4 ───────────

/// Cascade simulation — O(n²) version (the old algorithm).
fn stage_cascade_old(
    dirs: &[PathBuf],
    local_sums: &HashMap<PathBuf, u64>,
    parent_map: &HashMap<PathBuf, Option<PathBuf>>,
    child_counts: &HashMap<PathBuf, usize>,
) -> (HashMap<PathBuf, u64>, std::time::Duration) {
    let start = Instant::now();

    let mut pending: HashMap<PathBuf, usize> = child_counts.clone();
    let mut children_sum: HashMap<PathBuf, u64> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    let mut finished: HashMap<PathBuf, u64> = HashMap::new();

    loop {
        let mut progressed = false;
        for dir in dirs {
            if finished.contains_key(dir) {
                continue;
            }
            let local = match local_sums.get(dir) {
                Some(v) => *v,
                None => continue,
            };
            let p = *pending.get(dir).unwrap_or(&0);
            if p != 0 {
                continue;
            }
            let total = local.saturating_add(*children_sum.get(dir).unwrap_or(&0));
            finished.insert(dir.clone(), total);
            progressed = true;

            if let Some(Some(parent)) = parent_map.get(dir) {
                if let Some(rem) = pending.get_mut(parent) {
                    *rem = rem.saturating_sub(1);
                }
                if let Some(sum) = children_sum.get_mut(parent) {
                    *sum = sum.saturating_add(total);
                }
            }
        }
        if !progressed {
            break;
        }
    }

    (finished, start.elapsed())
}

/// Cascade simulation — O(n) version (deepest-first topological order).
fn stage_cascade_new(
    dirs: &[PathBuf],
    dir_depths: &HashMap<PathBuf, usize>,
    local_sums: &HashMap<PathBuf, u64>,
    parent_map: &HashMap<PathBuf, Option<PathBuf>>,
) -> (HashMap<PathBuf, u64>, std::time::Duration) {
    let start = Instant::now();

    // Sort deepest-first.
    let mut sorted: Vec<&PathBuf> = dirs.iter().collect();
    sorted.sort_by(|a, b| {
        let da = dir_depths.get(*a).copied().unwrap_or(0);
        let db = dir_depths.get(*b).copied().unwrap_or(0);
        db.cmp(&da)
    });

    let mut children_sum: HashMap<&PathBuf, u64> = HashMap::new();
    let mut finished: HashMap<PathBuf, u64> = HashMap::new();

    for dir in &sorted {
        let local = local_sums.get(*dir).copied().unwrap_or(0);
        let cs = children_sum.get(*dir).copied().unwrap_or(0);
        let total = local.saturating_add(cs);
        finished.insert((*dir).clone(), total);

        if let Some(Some(parent)) = parent_map.get(*dir) {
            *children_sum.entry(parent).or_insert(0) += total;
        }
    }

    (finished, start.elapsed())
}

// ───────────────────────────────────────── main ──────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    });
    let num_threads: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

    let root = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Cannot resolve {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    println!("Benchmarking: {}", root.display());
    println!("Threads:      {num_threads}");
    println!("{}", "=".repeat(60));

    // ── Stage 1: tree walk ───────────────────────────────────
    let max_depth = 3;
    let (entries, dir_count, file_count, symlink_count, t1) =
        stage_tree_walk(&root, max_depth);

    println!("\nStage 1 — Tree walk (WalkBuilder, depth {max_depth})");
    println!("  entries: {entries}  (dirs: {dir_count}, files: {file_count}, symlinks: {symlink_count})");
    println!("  time:    {t1:.2?}");

    let tree_dirs: Vec<PathBuf> = {
        let walker = ignore::WalkBuilder::new(&root)
            .max_depth(Some(max_depth))
            .hidden(false)
            .git_ignore(true)
            .build();
        walker
            .flatten()
            .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
            .map(|e| e.path().to_path_buf())
            .collect()
    };

    // ── Stage 2: shallow stat ────────────────────────────────
    let (files_statted, shallow_bytes, t2) = stage_shallow_stat(&tree_dirs);

    println!("\nStage 2 — Shallow stat (readdir+stat per tree dir)");
    println!("  tree dirs:     {}", tree_dirs.len());
    println!("  files statted: {files_statted}");
    println!("  shallow bytes: {}", human_size(shallow_bytes));
    println!("  time:          {t2:.2?}");

    // ── Stage 3: recursive walk ──────────────────────────────
    println!("\nStage 3 — Full recursive walk + stat");

    let (f1, d1, b1, t3_1t_nodedup) = stage_recursive_walk_single(&root, false);
    println!("\n  [1 thread, no dedup]");
    println!("    dirs: {d1}, files: {f1}, bytes: {} ({b1})", human_size(b1));
    println!("    time: {t3_1t_nodedup:.2?}");

    let (f2, d2, b2, t3_1t_dedup) = stage_recursive_walk_single(&root, true);
    println!("\n  [1 thread, dedup]");
    println!("    dirs: {d2}, files: {f2}, bytes: {} ({b2})", human_size(b2));
    println!("    time: {t3_1t_dedup:.2?}");

    let (f3, d3, b3, t3_nt_nodedup) = stage_recursive_walk_multi(&root, num_threads, false);
    println!("\n  [{num_threads} threads, no dedup]");
    println!("    dirs: {d3}, files: {f3}, bytes: {} ({b3})", human_size(b3));
    println!("    time: {t3_nt_nodedup:.2?}");

    let (f4, d4, b4, t3_nt_dedup) = stage_recursive_walk_multi(&root, num_threads, true);
    println!("\n  [{num_threads} threads, dedup]");
    println!("    dirs: {d4}, files: {f4}, bytes: {} ({b4})", human_size(b4));
    println!("    time: {t3_nt_dedup:.2?}");

    let dedup_saved = b1.saturating_sub(b2);
    println!("\n  Dedup saved: {} ({dedup_saved} bytes)", human_size(dedup_saved));

    println!("\n  Speedups:");
    if t3_nt_nodedup.as_nanos() > 0 {
        let s = t3_1t_nodedup.as_secs_f64() / t3_nt_nodedup.as_secs_f64();
        println!("    no dedup: {s:.2}x  ({t3_1t_nodedup:.2?} → {t3_nt_nodedup:.2?})");
    }
    if t3_nt_dedup.as_nanos() > 0 {
        let s = t3_1t_dedup.as_secs_f64() / t3_nt_dedup.as_secs_f64();
        println!("    dedup:    {s:.2}x  ({t3_1t_dedup:.2?} → {t3_nt_dedup:.2?})");
    }
    if t3_1t_nodedup.as_nanos() > 0 {
        let overhead = t3_1t_dedup.as_secs_f64() / t3_1t_nodedup.as_secs_f64();
        println!("    dedup overhead (1t): {:.1}%", (overhead - 1.0) * 100.0);
    }

    // ── Stage 4: cascade ─────────────────────────────────────
    // Build cascade structures from a fresh walk (page cache is warm).
    let mut dir_local_sums: HashMap<PathBuf, u64> = HashMap::new();
    let mut parent_map: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    let mut child_dir_counts: HashMap<PathBuf, usize> = HashMap::new();
    let mut dir_depths: HashMap<PathBuf, usize> = HashMap::new();
    let mut all_dirs: Vec<PathBuf> = Vec::new();

    {
        // BFS to get depths.
        let mut bfs: VecDeque<(PathBuf, usize)> = VecDeque::new();
        bfs.push_back((root.clone(), 0));
        parent_map.insert(root.clone(), None);

        while let Some((current, depth)) = bfs.pop_front() {
            all_dirs.push(current.clone());
            dir_local_sums.insert(current.clone(), 0);
            child_dir_counts.insert(current.clone(), 0);
            dir_depths.insert(current.clone(), depth);

            let entries = match std::fs::read_dir(&current) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let ft = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                let p = entry.path();
                if ft.is_dir() {
                    parent_map.insert(p.clone(), Some(current.clone()));
                    *child_dir_counts.get_mut(&current).unwrap() += 1;
                    bfs.push_back((p, depth + 1));
                } else if ft.is_file() {
                    if let Ok(meta) = entry.metadata() {
                        *dir_local_sums.get_mut(&current).unwrap() += meta.len();
                    }
                } else if ft.is_symlink() {
                    if let Ok(meta) = std::fs::symlink_metadata(&p) {
                        *dir_local_sums.get_mut(&current).unwrap() += meta.len();
                    }
                }
            }
        }
    }

    println!("\nStage 4 — Cascade finalization ({} dirs)", all_dirs.len());

    let (totals_old, t4_old) = stage_cascade_old(&all_dirs, &dir_local_sums, &parent_map, &child_dir_counts);
    let root_old = totals_old.get(&root).copied().unwrap_or(0);
    println!("\n  [old O(n²)]");
    println!("    root total: {} ({root_old})", human_size(root_old));
    println!("    time:       {t4_old:.2?}");

    let (totals_new, t4_new) = stage_cascade_new(&all_dirs, &dir_depths, &dir_local_sums, &parent_map);
    let root_new = totals_new.get(&root).copied().unwrap_or(0);
    println!("\n  [new O(n) topo-sort]");
    println!("    root total: {} ({root_new})", human_size(root_new));
    println!("    time:       {t4_new:.2?}");

    if t4_new.as_nanos() > 0 {
        let s = t4_old.as_secs_f64() / t4_new.as_secs_f64();
        println!("\n    Speedup: {s:.1}x  ({t4_old:.2?} → {t4_new:.2?})");
    }

    assert_eq!(root_old, root_new, "Cascade mismatch!");

    // ── Summary ──────────────────────────────────────────────
    println!("\n{}", "=".repeat(60));

    let total_old = t1 + t2 + t3_nt_dedup + t4_old;
    let total_new = t1 + t2 + t3_nt_dedup + t4_new;

    println!("End-to-end ({num_threads} threads, dedup, {}-thread walk):", num_threads);
    println!("  old cascade: {total_old:.2?}");
    println!("  new cascade: {total_new:.2?}");

    // Compare with du
    println!("\n--- Reference: `du --apparent-size -s -b` ---");
    let du_start = Instant::now();
    let du_output = std::process::Command::new("du")
        .args(["--apparent-size", "-s", "-b"])
        .arg(&root)
        .output();
    let du_time = du_start.elapsed();
    match du_output {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            let du_bytes: u64 = s
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            println!("  du reports:    {} ({du_bytes})", human_size(du_bytes));
            println!("  du time:       {du_time:.2?}");
            println!("  our (dedup):   {} ({b2})", human_size(b2));
            if b2 > 0 && du_bytes > 0 {
                let diff = (b2 as f64 - du_bytes as f64).abs();
                let pct = diff / du_bytes as f64 * 100.0;
                println!("  difference:    {} ({pct:.2}%)", human_size(diff as u64));
            }
        }
        Err(e) => println!("  (du failed: {e})"),
    }
}
