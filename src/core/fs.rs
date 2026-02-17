//! Filesystem traversal — walk directories and populate a [`DirTree`].
//!
//! The walker respects `.gitignore` rules via the [`ignore`] crate and caps
//! the depth to keep things snappy.

use std::path::Path;

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

/// Build a [`DirTree`] rooted at `root` using the given config.
///
/// Directories are sorted before files; within each group entries are sorted
/// alphabetically (case-insensitive).
pub fn build_tree(root: &Path, config: &WalkConfig) -> anyhow::Result<DirTree> {
    let root_meta = EntryMeta::from_path(root)?;
    let mut tree = DirTree::new(root_meta);

    // Populate children of the root.
    let root_id = tree.root;
    populate_children(&mut tree, root_id, root, config, 0)?;

    Ok(tree)
}

/// Recursively populate children for `parent_id`.
fn populate_children(
    tree: &mut DirTree,
    parent_id: NodeId,
    dir: &Path,
    config: &WalkConfig,
    current_depth: usize,
) -> anyhow::Result<()> {
    if current_depth >= config.max_depth {
        return Ok(());
    }

    let walker = WalkBuilder::new(dir)
        .max_depth(Some(1)) // only immediate children
        .hidden(!config.show_hidden)
        .git_ignore(config.respect_gitignore)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    // Collect entries, split into dirs and files, sort each group.
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in walker.flatten() {
        let path = entry.path();
        // Skip the directory itself (WalkBuilder yields the root as first entry).
        if path == dir {
            continue;
        }

        if let Ok(meta) = EntryMeta::from_path(path) {
            if meta.is_dir {
                dirs.push(meta);
            } else {
                files.push(meta);
            }
        }
    }

    // Sort: dirs first (alphabetical), then files (alphabetical).
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Insert dirs first, then files.
    for meta in dirs {
        let child_path = meta.path.clone();
        let child_id = tree.add_child(parent_id, meta);
        // Recurse into subdirectories.
        populate_children(tree, child_id, &child_path, config, current_depth + 1)?;
    }
    for meta in files {
        tree.add_child(parent_id, meta);
    }

    Ok(())
}

/// Lazily expand a single directory that hasn't been populated yet.
/// Useful when the user expands a previously-collapsed node.
pub fn expand_node(
    tree: &mut DirTree,
    node_id: NodeId,
    config: &WalkConfig,
) -> anyhow::Result<()> {
    let node = tree.get(node_id);
    if !node.meta.is_dir || !node.children.is_empty() {
        // Already populated or not a directory.
        return Ok(());
    }
    let dir = node.meta.path.clone();
    let depth = node.depth;
    populate_children(tree, node_id, &dir, config, depth)?;
    Ok(())
}

