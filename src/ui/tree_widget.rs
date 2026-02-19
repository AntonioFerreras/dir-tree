//! Custom Ratatui widget that renders a [`DirTree`] as an indented,
//! collapsible tree with grouping support.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, StatefulWidget, Widget},
};

use crate::core::{
    grouping::{self, GroupedEntry, GroupingConfig},
    tree::{DirTree, NodeId},
};

use super::theme::Theme;

// ───────────────────────────────────────── state ─────────────

/// Persistent state for the tree widget (selected index, scroll offset).
#[derive(Debug, Default)]
pub struct TreeWidgetState {
    /// Index into the *visible* flat list that is currently highlighted.
    pub selected: usize,
    /// Vertical scroll offset (first visible row).
    pub offset: usize,
}

impl TreeWidgetState {
    pub fn select_next(&mut self, max: usize) {
        if max > 0 && self.selected < max - 1 {
            self.selected += 1;
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Ensure the selected row is visible within the viewport of `height` rows.
    pub fn clamp_scroll(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + height {
            self.offset = self.selected - height + 1;
        }
    }
}

// ───────────────────────────────────────── row model ─────────

/// One rendered row in the tree view.
#[derive(Debug)]
pub enum TreeRow {
    Node {
        node_id: NodeId,
        depth: usize,
        is_dir: bool,
        is_symlink: bool,
        expanded: bool,
        label: String,
        /// For symlinks: the target path (displayed as `→ target`).
        symlink_target: Option<String>,
    },
    Group {
        depth: usize,
        label: String,
        /// Stable key used to track expanded state (e.g. "/path/to/parent:*.png").
        group_key: String,
        /// Whether this group is currently expanded to show its members.
        expanded: bool,
        /// Member node IDs (for expanding).
        members: Vec<NodeId>,
    },
}

// ───────────────────────────────────────── widget ────────────

/// The tree widget itself — created fresh each frame.
pub struct TreeWidget<'a> {
    tree: &'a DirTree,
    grouping_config: &'a GroupingConfig,
    dir_sizes: Option<&'a HashMap<PathBuf, u64>>,
    file_sizes: Option<&'a HashMap<PathBuf, u64>>,
    block: Option<Block<'a>>,
    /// Optional hint shown on the selected non-dir row (e.g. "→ to pin").
    pin_hint: Option<String>,
    /// Keys of groups that are currently expanded.
    expanded_groups: Option<&'a HashSet<String>>,
}

impl<'a> TreeWidget<'a> {
    pub fn new(tree: &'a DirTree, grouping_config: &'a GroupingConfig) -> Self {
        Self {
            tree,
            grouping_config,
            dir_sizes: None,
            file_sizes: None,
            block: None,
            pin_hint: None,
            expanded_groups: None,
        }
    }

    pub fn dir_sizes(mut self, sizes: &'a HashMap<PathBuf, u64>) -> Self {
        self.dir_sizes = Some(sizes);
        self
    }

    pub fn file_sizes(mut self, sizes: &'a HashMap<PathBuf, u64>) -> Self {
        self.file_sizes = Some(sizes);
        self
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Set a hint string shown beside the selected non-directory file
    /// (e.g. `"→ to pin file on inspector"`).
    pub fn pin_hint(mut self, hint: Option<String>) -> Self {
        self.pin_hint = hint;
        self
    }

    /// Provide the set of currently expanded group keys.
    pub fn expanded_groups(mut self, groups: &'a HashSet<String>) -> Self {
        self.expanded_groups = Some(groups);
        self
    }

    /// Build the flat list of rows (with grouping applied).
    pub fn build_rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.collect_rows(self.tree.root, &mut rows);
        rows
    }

    fn collect_rows(&self, node_id: NodeId, rows: &mut Vec<TreeRow>) {
        let node = self.tree.get(node_id);

        // Push the node itself.
        rows.push(TreeRow::Node {
            node_id,
            depth: node.depth,
            is_dir: node.meta.is_dir,
            is_symlink: node.meta.is_symlink,
            expanded: node.expanded,
            label: node.meta.name.clone(),
            symlink_target: node.meta.symlink_target.clone(),
        });

        if !node.expanded || !node.meta.is_dir {
            return;
        }

        // Apply grouping to this node's children.
        let grouped = grouping::group_children(self.tree, node_id, self.grouping_config, self.file_sizes);
        let parent_path = node.meta.path.display().to_string();

        for entry in grouped {
            match entry {
                GroupedEntry::Single(child_id) => {
                    self.collect_rows(child_id, rows);
                }
                GroupedEntry::Group {
                    label,
                    count,
                    total_size,
                    members,
                } => {
                    let depth = node.depth + 1;
                    let group_key = format!("{parent_path}:{label}");
                    let expanded = self
                        .expanded_groups
                        .map_or(false, |g| g.contains(&group_key));

                    rows.push(TreeRow::Group {
                        depth,
                        label: format!("{count} {label} files {}", grouping::human_size(total_size)),
                        group_key,
                        expanded,
                        members: members.clone(),
                    });

                    // When expanded, show each member indented one level deeper.
                    if expanded {
                        for &member_id in &members {
                            let member = self.tree.get(member_id);
                            rows.push(TreeRow::Node {
                                node_id: member_id,
                                depth: depth + 1,
                                is_dir: false,
                                is_symlink: member.meta.is_symlink,
                                expanded: false,
                                label: member.meta.name.clone(),
                                symlink_target: member.meta.symlink_target.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
}

impl<'a> StatefulWidget for TreeWidget<'a> {
    type State = TreeWidgetState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Resolve the inner area (inside the optional block border).
        let inner = if let Some(ref block) = self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        let rows = self.build_rows();
        state.clamp_scroll(inner.height as usize);

        let visible_rows = rows
            .iter()
            .enumerate()
            .skip(state.offset)
            .take(inner.height as usize);

        for (i, (row_idx, row)) in visible_rows.enumerate() {
            let y = inner.y + i as u16;
            let is_selected = row_idx == state.selected;

            let line = match row {
                TreeRow::Node {
                    node_id,
                    depth,
                    is_dir,
                    is_symlink,
                    expanded,
                    label,
                    symlink_target,
                } => {
                    let indent = "  ".repeat(*depth);
                    let icon = if *is_symlink {
                        "~ "
                    } else if *is_dir {
                        if *expanded {
                            "▼ "
                        } else {
                            "▶ "
                        }
                    } else {
                        "  "
                    };
                    let style = if is_selected {
                        Theme::selected_style()
                    } else if *is_symlink {
                        Theme::symlink_style()
                    } else if *is_dir {
                        Theme::dir_style()
                    } else {
                        Theme::file_style()
                    };

                    let mut spans = vec![
                        Span::raw(indent),
                        Span::styled(format!("{icon}{label}"), style),
                    ];

                    // Show symlink target as `→ target`.
                    if let Some(target) = symlink_target {
                        let target_style = if is_selected {
                            Theme::selected_style()
                        } else {
                            Theme::size_style()
                        };
                        spans.push(Span::styled(format!(" → {target}"), target_style));
                    }

                    let path = &self.tree.get(*node_id).meta.path;
                    let maybe_size = if *is_dir {
                        self.dir_sizes.and_then(|sizes| sizes.get(path).copied())
                    } else {
                        self.file_sizes.and_then(|sizes| sizes.get(path).copied())
                    };

                    if let Some(size) = maybe_size {
                        let size_style = if is_selected {
                            Theme::selected_style()
                        } else {
                            Theme::size_style()
                        };
                        spans.push(Span::styled(
                            format!(" {}", grouping::human_size(size)),
                            size_style,
                        ));
                    }

                    // Hint on selected root: explain how to navigate above
                    // the launch directory.
                    if is_selected && *node_id == self.tree.root {
                        spans.push(Span::styled(
                            "  Collapse to see parent directory",
                            Theme::root_hint_style(),
                        ));
                    }

                    // Hint on selected non-dir file: explain pin action.
                    if is_selected && !*is_dir {
                        if let Some(ref hint) = self.pin_hint {
                            spans.push(Span::styled(
                                format!("  {hint}"),
                                Theme::root_hint_style(),
                            ));
                        }
                    }

                    Line::from(spans)
                }
                TreeRow::Group {
                    depth,
                    label,
                    expanded,
                    ..
                } => {
                    let indent = "  ".repeat(*depth);
                    let icon = if *expanded { "− " } else { "+ " };
                    let style = if is_selected {
                        Theme::selected_style()
                    } else {
                        Theme::group_style()
                    };
                    Line::from(vec![
                        Span::raw(indent),
                        Span::styled(format!("{icon}{label}"), style),
                    ])
                }
            };

            // Render the line into the buffer.
            let line_width = inner.width as usize;
            buf.set_line(inner.x, y, &line, line_width as u16);
        }
    }
}

