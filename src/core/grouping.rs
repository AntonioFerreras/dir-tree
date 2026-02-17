//! Grouping algorithms — collapse many similar entries into a summary line.
//!
//! When a directory contains 40 `.png` files you don't need to see all of them
//! individually.  The grouping layer inspects the children of a node and
//! produces [`GroupedEntry`] values that the UI renders instead of raw nodes.

use std::collections::HashMap;
use std::path::PathBuf;

use super::tree::{DirTree, NodeId};

// ───────────────────────────────────────── types ─────────────

/// How a set of children should be presented.
#[derive(Debug, Clone)]
pub enum GroupedEntry {
    /// Show the node as-is (a single file or directory).
    Single(NodeId),
    /// A collapsed group: "12 .png files (340 KB)".
    Group {
        /// Representative label, e.g. `"*.png"`.
        label: String,
        /// Number of files in the group.
        count: usize,
        /// Combined size in bytes.
        total_size: u64,
        /// The underlying node ids (so the user can expand the group).
        members: Vec<NodeId>,
    },
}

/// Configuration for the grouping heuristics.
#[derive(Debug, Clone)]
pub struct GroupingConfig {
    /// Minimum number of files sharing the same extension before we collapse
    /// them into a group.
    pub min_group_size: usize,
}

impl Default for GroupingConfig {
    fn default() -> Self {
        Self { min_group_size: 5 }
    }
}

// ───────────────────────────────────────── algorithm ─────────

/// Given a parent node, return the grouped view of its **direct children**.
///
/// Strategy:
/// 1. Directories are always shown individually.
/// 2. Files are bucketed by extension.
/// 3. If a bucket has ≥ `min_group_size` entries it becomes a [`GroupedEntry::Group`].
/// 4. Otherwise each file stays as [`GroupedEntry::Single`].
pub fn group_children(
    tree: &DirTree,
    parent_id: NodeId,
    config: &GroupingConfig,
    file_sizes: Option<&HashMap<PathBuf, u64>>,
) -> Vec<GroupedEntry> {
    let parent = tree.get(parent_id);
    let mut result: Vec<GroupedEntry> = Vec::new();

    // Bucket files by extension.
    let mut ext_buckets: HashMap<Option<String>, Vec<NodeId>> = HashMap::new();

    for &child_id in &parent.children {
        let child = tree.get(child_id);
        if child.meta.is_dir {
            // Directories always show individually.
            result.push(GroupedEntry::Single(child_id));
        } else {
            ext_buckets
                .entry(child.meta.extension.clone())
                .or_default()
                .push(child_id);
        }
    }

    // Convert buckets to grouped entries.
    let mut ext_keys: Vec<_> = ext_buckets.keys().cloned().collect();
    ext_keys.sort_by(|a, b| {
        let a_str = a.as_deref().unwrap_or("");
        let b_str = b.as_deref().unwrap_or("");
        a_str.cmp(b_str)
    });

    for ext in ext_keys {
        let members = ext_buckets.remove(&ext).unwrap();
        if members.len() >= config.min_group_size {
            let total_size: u64 = members
                .iter()
                .map(|&id| {
                    let node = tree.get(id);
                    // Prefer the async-computed size; fall back to meta.size.
                    file_sizes
                        .and_then(|fs| fs.get(&node.meta.path).copied())
                        .unwrap_or(node.meta.size)
                })
                .sum();
            let label = match &ext {
                Some(e) => format!("*.{e}"),
                None => "(no extension)".to_string(),
            };
            result.push(GroupedEntry::Group {
                label,
                count: members.len(),
                total_size,
                members,
            });
        } else {
            for id in members {
                result.push(GroupedEntry::Single(id));
            }
        }
    }

    result
}

/// Human-readable size string.
pub fn human_size(bytes: u64) -> String {
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

