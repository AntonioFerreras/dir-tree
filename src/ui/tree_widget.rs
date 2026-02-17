//! Custom Ratatui widget that renders a [`DirTree`] as an indented,
//! collapsible tree with grouping support.

use std::collections::HashMap;
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
        expanded: bool,
        label: String,
    },
    Group {
        depth: usize,
        label: String, // e.g. "12 *.png files (340 KB)"
    },
}

// ───────────────────────────────────────── widget ────────────

/// The tree widget itself — created fresh each frame.
pub struct TreeWidget<'a> {
    tree: &'a DirTree,
    grouping_config: &'a GroupingConfig,
    dir_sizes: Option<&'a HashMap<PathBuf, u64>>,
    block: Option<Block<'a>>,
}

impl<'a> TreeWidget<'a> {
    pub fn new(tree: &'a DirTree, grouping_config: &'a GroupingConfig) -> Self {
        Self {
            tree,
            grouping_config,
            dir_sizes: None,
            block: None,
        }
    }

    pub fn dir_sizes(mut self, sizes: &'a HashMap<PathBuf, u64>) -> Self {
        self.dir_sizes = Some(sizes);
        self
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
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
            expanded: node.expanded,
            label: node.meta.name.clone(),
        });

        if !node.expanded || !node.meta.is_dir {
            return;
        }

        // Apply grouping to this node's children.
        let grouped = grouping::group_children(self.tree, node_id, self.grouping_config);

        for entry in grouped {
            match entry {
                GroupedEntry::Single(child_id) => {
                    self.collect_rows(child_id, rows);
                }
                GroupedEntry::Group {
                    label,
                    count,
                    total_size,
                    ..
                } => {
                    let depth = node.depth + 1;
                    rows.push(TreeRow::Group {
                        depth,
                        label: format!(
                            "{count} {label} files ({})",
                            grouping::human_size(total_size)
                        ),
                    });
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
                    expanded,
                    label,
                } => {
                    let indent = "  ".repeat(*depth);
                    let icon = if *is_dir {
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
                    } else if *is_dir {
                        Theme::dir_style()
                    } else {
                        Theme::file_style()
                    };

                    let mut spans = vec![
                        Span::raw(indent),
                        Span::styled(format!("{icon}{label}"), style),
                    ];

                    // Show directory size when available.
                    if *is_dir {
                        if let Some(sizes) = self.dir_sizes {
                            let path = &self.tree.get(*node_id).meta.path;
                            if let Some(&size) = sizes.get(path) {
                                let size_style = if is_selected {
                                    Theme::selected_style()
                                } else {
                                    Theme::size_style()
                                };
                                spans.push(Span::styled(
                                    format!(" ({})", grouping::human_size(size)),
                                    size_style,
                                ));
                            }
                        }
                    }

                    Line::from(spans)
                }
                TreeRow::Group { depth, label } => {
                    let indent = "  ".repeat(*depth);
                    let style = if is_selected {
                        Theme::selected_style()
                    } else {
                        Theme::group_style()
                    };
                    Line::from(vec![
                        Span::raw(indent),
                        Span::styled(format!("  {label}"), style),
                    ])
                }
            };

            // Render the line into the buffer.
            let line_width = inner.width as usize;
            buf.set_line(inner.x, y, &line, line_width as u16);
        }
    }
}

