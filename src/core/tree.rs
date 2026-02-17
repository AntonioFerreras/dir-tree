//! In-memory tree data-structure that mirrors the local directory layout.
//!
//! The [`TreeNode`] is the fundamental unit – it holds metadata about a single
//! filesystem entry and links to its children via indices into an arena
//! (the [`DirTree`] struct).  Using an arena avoids recursive `Box` allocations,
//! is cache-friendly, and makes borrowing trivial.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ───────────────────────────────────────── node metadata ─────

/// Lightweight metadata we keep per filesystem entry.
#[derive(Debug, Clone)]
pub struct EntryMeta {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// File extension (lower-cased), e.g. `"rs"`, `"toml"`. `None` for dirs
    /// or extensionless files.
    pub extension: Option<String>,
}

impl EntryMeta {
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.to_path_buf(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: meta.modified().ok(),
            extension: path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase()),
        })
    }
}

// ───────────────────────────────────────── tree node ─────────

/// Index into [`DirTree::nodes`].
pub type NodeId = usize;

/// A single node in the arena-allocated tree.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub meta: EntryMeta,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// Whether this node is expanded in the UI (only meaningful for dirs).
    pub expanded: bool,
    /// Depth from the root (0 = root).
    pub depth: usize,
}

// ───────────────────────────────────────── arena tree ────────

/// Arena-backed directory tree.
///
/// Nodes are stored in a flat `Vec` and reference each other by index, which
/// avoids recursive ownership and makes traversal cheap.
#[derive(Debug, Clone)]
pub struct DirTree {
    pub nodes: Vec<TreeNode>,
    pub root: NodeId,
}

impl DirTree {
    /// Create a new tree with a single root node.
    pub fn new(root_meta: EntryMeta) -> Self {
        let root = TreeNode {
            meta: root_meta,
            parent: None,
            children: Vec::new(),
            expanded: true,
            depth: 0,
        };
        Self {
            nodes: vec![root],
            root: 0,
        }
    }

    /// Add a child under `parent_id` and return its [`NodeId`].
    pub fn add_child(&mut self, parent_id: NodeId, meta: EntryMeta) -> NodeId {
        let depth = self.nodes[parent_id].depth + 1;
        let id = self.nodes.len();
        self.nodes.push(TreeNode {
            meta,
            parent: Some(parent_id),
            children: Vec::new(),
            expanded: false,
            depth,
        });
        self.nodes[parent_id].children.push(id);
        id
    }

    /// Iterate node ids that are currently visible (expanded ancestors).
    /// This is the flattened list the UI will render.
    pub fn visible_nodes(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect_visible(self.root, &mut out);
        out
    }

    fn collect_visible(&self, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        let node = &self.nodes[id];
        if node.expanded {
            for &child in &node.children {
                self.collect_visible(child, out);
            }
        }
    }

    /// Toggle the expanded state of a node (only if it is a directory).
    pub fn toggle_expand(&mut self, id: NodeId) {
        if self.nodes[id].meta.is_dir {
            self.nodes[id].expanded = !self.nodes[id].expanded;
        }
    }

    /// Return a reference to a node.
    pub fn get(&self, id: NodeId) -> &TreeNode {
        &self.nodes[id]
    }

    /// Return a mutable reference to a node.
    pub fn get_mut(&mut self, id: NodeId) -> &mut TreeNode {
        &mut self.nodes[id]
    }
}

