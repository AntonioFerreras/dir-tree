//! Filesystem traversal — walk directories and populate a [`DirTree`].
//!
//! The walker respects `.gitignore` rules via the [`ignore`] crate and caps
//! the depth to keep things snappy.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::tree::{DirTree, EntryMeta, NodeId};

/// Configuration knobs for the traversal.
#[derive(Debug, Clone)]
pub struct WalkConfig {
    /// Maximum depth to descend (0 = root only).
    pub max_depth: usize,
    /// Respect `.gitignore` files.
    pub respect_gitignore: bool,
    /// Show hidden (dot-prefixed) entries.
    pub show_hidden: bool,
}

impl Default for WalkConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            respect_gitignore: true,
            show_hidden: false,
        }
    }
}

/// Build an [`EntryMeta`] from an [`ignore::DirEntry`] without an extra `stat`
/// call.  File type comes from `readdir` for free on Unix.
fn meta_from_dir_entry(entry: &ignore::DirEntry) -> EntryMeta {
    let path = entry.path().to_path_buf();
    let is_dir = entry.file_type().map_or(false, |ft| ft.is_dir());
    EntryMeta {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        extension: path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase()),
        is_dir,
        size: 0,
        modified: None,
        path,
    }
}

/// Sort helper — case-insensitive by name.
fn sort_by_name(entries: &mut [EntryMeta]) {
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
}

/// Build a [`DirTree`] rooted at `root` using the given config.
///
/// Uses a **single** `WalkBuilder` pass (one `.gitignore` parse, no redundant
/// `stat` calls) and assembles the tree in BFS order afterward.
pub fn build_tree(root: &Path, config: &WalkConfig) -> anyhow::Result<DirTree> {
    let root_meta = EntryMeta::from_path(root)?;
    let mut tree = DirTree::new(root_meta);

    // Single walk at full depth — avoids re-creating a WalkBuilder per dir.
    let walker = WalkBuilder::new(root)
        .max_depth(Some(config.max_depth))
        .hidden(!config.show_hidden)
        .git_ignore(config.respect_gitignore)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    // Group entries by parent directory.
    let mut children: HashMap<PathBuf, (Vec<EntryMeta>, Vec<EntryMeta>)> = HashMap::new();

    for entry in walker.flatten() {
        let path = entry.path();
        if path == root {
            continue;
        }
        let parent = match path.parent() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };

        let meta = meta_from_dir_entry(&entry);
        let (dirs, files) = children.entry(parent).or_default();
        if meta.is_dir {
            dirs.push(meta);
        } else {
            files.push(meta);
        }
    }

    // Sort each group: dirs first (alphabetical), then files (alphabetical).
    for (dirs, files) in children.values_mut() {
        sort_by_name(dirs);
        sort_by_name(files);
    }

    // Assemble the tree in BFS order so parent nodes exist before children.
    let mut queue = VecDeque::new();
    queue.push_back((tree.root, root.to_path_buf()));

    while let Some((parent_id, parent_path)) = queue.pop_front() {
        if let Some((dirs, files)) = children.remove(&parent_path) {
            for meta in dirs {
                let child_path = meta.path.clone();
                let child_id = tree.add_child(parent_id, meta);
                queue.push_back((child_id, child_path));
            }
            for meta in files {
                tree.add_child(parent_id, meta);
            }
        }
    }

    Ok(tree)
}

/// Lazily expand a single directory that hasn't been populated yet.
/// Useful when the user expands a previously-collapsed node beyond the
/// initial `max_depth`.
pub fn expand_node(
    tree: &mut DirTree,
    node_id: NodeId,
    config: &WalkConfig,
) -> anyhow::Result<()> {
    let node = tree.get(node_id);
    if !node.meta.is_dir || !node.children.is_empty() {
        return Ok(());
    }
    let dir = node.meta.path.clone();

    // Walk immediate children only (single level); deeper expansion
    // happens lazily when the user expands those children.
    let walker = WalkBuilder::new(&dir)
        .max_depth(Some(1))
        .hidden(!config.show_hidden)
        .git_ignore(config.respect_gitignore)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in walker.flatten() {
        if entry.path() == dir.as_path() {
            continue;
        }
        let meta = meta_from_dir_entry(&entry);
        if meta.is_dir {
            dirs.push(meta);
        } else {
            files.push(meta);
        }
    }

    sort_by_name(&mut dirs);
    sort_by_name(&mut files);

    for meta in dirs {
        tree.add_child(node_id, meta);
    }
    for meta in files {
        tree.add_child(node_id, meta);
    }

    Ok(())
}

/// Compute the total size in bytes of all regular files under `dir` (recursive).
///
/// Symlinks are not followed.  Permission errors are silently skipped.
/// Designed to be called from a background thread.
pub fn dir_size(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

