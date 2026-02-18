//! Size computation types and platform helpers.
//!
//! This module contains the data types shared between the size computation
//! workers and the rest of the application, plus platform-specific helpers
//! for inode classification and device checks.  The orchestration (spawning
//! workers, cascade finalization) lives in `main.rs`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Map of hard-linked inodes: (dev, ino) → apparent size.
/// Only files with nlink > 1 land here; nlink == 1 files are summed directly.
pub type InodeMap = HashMap<(u64, u64), u64>;

/// Cached result from a directory's local walk.
#[derive(Clone, Default)]
pub struct DirLocalResult {
    /// Sum of apparent sizes for files with nlink == 1 (safely additive).
    pub unique_sum: u64,
    /// Hard-linked files: (dev, ino) → size.  Deduped within this subtree,
    /// but may overlap with sibling directories — the cascade merges these.
    pub hardlinks: InodeMap,
}

// ───────────────────────────────────────── platform helpers ──

/// Classify a file as unique or hard-linked.
///
/// Returns `(apparent_size, Some((dev, ino)))` for hard-linked files,
/// or `(apparent_size, None)` for unique files (nlink ≤ 1).
#[cfg(unix)]
pub fn classify_file(meta: &std::fs::Metadata, dedup: bool) -> (u64, Option<(u64, u64)>) {
    let size = meta.len();
    if !dedup {
        return (size, None);
    }
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() <= 1 {
        (size, None)
    } else {
        (size, Some((meta.dev(), meta.ino())))
    }
}

#[cfg(not(unix))]
pub fn classify_file(meta: &std::fs::Metadata, _dedup: bool) -> (u64, Option<(u64, u64)>) {
    (meta.len(), None)
}

/// Check whether a path resides on the same device as the root.
#[cfg(unix)]
pub fn is_same_device(meta: &std::fs::Metadata, root_dev: u64) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.dev() == root_dev
}

#[cfg(not(unix))]
pub fn is_same_device(_meta: &std::fs::Metadata, _root_dev: u64) -> bool {
    true
}

/// Get the device ID of a path (0 on non-Unix).
#[cfg(unix)]
pub fn get_dev(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map(|m| m.dev()).unwrap_or(0)
}

#[cfg(not(unix))]
pub fn get_dev(_path: &Path) -> u64 {
    0
}

// ───────────────────────────────────────── recursive walk ────

/// Recursively compute the total apparent size of all files under `dir`.
///
/// Returns `(unique_sum, hardlinks)` — split by nlink so the cascade can
/// merge hardlink maps bottom-up for per-subtree dedup.
pub fn recursive_dir_size(
    dir: &Path,
    cancel: &AtomicBool,
    dedup: bool,
    one_file_system: bool,
    root_dev: u64,
) -> (u64, InodeMap) {
    let mut unique_sum: u64 = 0;
    let mut hardlinks = InodeMap::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
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
                if one_file_system {
                    if let Ok(meta) = std::fs::metadata(&entry.path()) {
                        if is_same_device(&meta, root_dev) {
                            stack.push(entry.path());
                        }
                    }
                } else {
                    stack.push(entry.path());
                }
            } else if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    let (size, inode_key) = classify_file(&meta, dedup);
                    match inode_key {
                        None => unique_sum = unique_sum.saturating_add(size),
                        Some(key) => {
                            hardlinks.entry(key).or_insert(size);
                        }
                    }
                }
            } else if ft.is_symlink() {
                if let Ok(meta) = std::fs::symlink_metadata(&entry.path()) {
                    unique_sum = unique_sum.saturating_add(meta.len());
                }
            }
        }
    }

    (unique_sum, hardlinks)
}

