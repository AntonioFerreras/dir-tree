//! Input handling — maps key/mouse events to state mutations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::core::fs;
use crate::core::tree::NodeId;

use super::state::{ActiveView, AppState};
use crate::ui::tree_widget::{TreeRow, TreeWidget};

/// Menu items shown in the settings popup.
pub const SETTINGS_ITEMS: &[&str] = &["Controls"];

/// Process a key event, dispatching based on the active view.
pub fn handle_key(state: &mut AppState, key: KeyEvent) {
    // Ctrl+c always quits, regardless of view.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        state.should_quit = true;
        return;
    }

    match state.active_view {
        ActiveView::Tree => handle_tree_key(state, key),
        ActiveView::SettingsMenu => handle_settings_key(state, key),
        ActiveView::ControlsSubmenu => handle_controls_key(state, key),
    }
}

// ── Tree view ────────────────────────────────────────────────────

fn handle_tree_key(state: &mut AppState, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // ── Quit ───────────────────────────────────────────────
        (_, KeyCode::Char('q')) => {
            state.should_quit = true;
        }

        // ── Settings ──────────────────────────────────────────
        (_, KeyCode::Char('?')) => {
            state.active_view = ActiveView::SettingsMenu;
            state.settings_selected = 0;
        }

        // ── Alt+navigation: jump between sibling dirs ─────────
        // (must come before plain navigation to avoid being caught by `_`)
        (m, KeyCode::Down) if m.contains(KeyModifiers::ALT) => {
            jump_to_sibling_dir(state, Direction::Down);
        }
        (m, KeyCode::Up) if m.contains(KeyModifiers::ALT) => {
            jump_to_sibling_dir(state, Direction::Up);
        }

        // ── Navigation ────────────────────────────────────────
        (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
            state.tree_state.select_prev();
        }
        (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
            let visible_count = state.tree.visible_nodes().len();
            state.tree_state.select_next(visible_count);
        }

        // ── Expand / collapse ─────────────────────────────────
        (_, KeyCode::Right) | (_, KeyCode::Char('l')) => {
            if let Some(node_id) = selected_node_id(state) {
                // Lazy-load children if needed, then expand.
                let _ = fs::expand_node(&mut state.tree, node_id, &state.walk_config);
                state.tree.get_mut(node_id).expanded = true;
                // Invalidate this dir's cached local_sum — its set of
                // tree-children just changed, so it needs re-walking.
                let path = state.tree.get(node_id).meta.path.clone();
                state.dir_local_sums.remove(&path);
                state.needs_size_recompute = true;
            }
        }
        (_, KeyCode::Left) | (_, KeyCode::Char('h')) => {
            handle_left(state);
        }

        // ── Enter: cd into directory ──────────────────────────
        (_, KeyCode::Enter) => {
            if let Some(node_id) = selected_node_id(state) {
                let node = state.tree.get(node_id);
                if node.meta.is_dir {
                    state.selected_dir = Some(node.meta.path.clone());
                    state.should_quit = true;
                }
            }
        }

        // ── Toggle hidden files ───────────────────────────────
        (_, KeyCode::Char('.')) => {
            state.walk_config.show_hidden = !state.walk_config.show_hidden;
            rebuild_tree(state);
        }

        _ => {}
    }
}

/// Handle Left key: collapse expanded dir, or go to parent for files/collapsed dirs.
fn handle_left(state: &mut AppState) {
    let Some(node_id) = selected_node_id(state) else {
        return;
    };

    let node = state.tree.get(node_id);

    if node.meta.is_dir && node.expanded {
        // Expanded dir → just collapse it.
        state.tree.get_mut(node_id).expanded = false;
    } else if let Some(parent_id) = state.tree.get(node_id).parent {
        // File or collapsed dir → collapse parent and select it.
        state.tree.get_mut(parent_id).expanded = false;
        // Find parent's row index in the updated visible rows.
        let rows = build_rows(state);
        for (i, row) in rows.iter().enumerate() {
            if let TreeRow::Node { node_id: nid, .. } = row {
                if *nid == parent_id {
                    state.tree_state.selected = i;
                    break;
                }
            }
        }
    }
}

enum Direction {
    Up,
    Down,
}

/// Jump to the next/previous sibling directory.
///
/// From a file, jumps to the next dir at the parent's depth (i.e. a sibling of
/// the containing folder).  From a directory, jumps to the next dir at the same
/// depth.
fn jump_to_sibling_dir(state: &mut AppState, direction: Direction) {
    let rows = build_rows(state);
    let current = state.tree_state.selected;

    let target_depth = match rows.get(current) {
        Some(TreeRow::Node { depth, is_dir, .. }) => {
            if *is_dir {
                *depth
            } else {
                depth.saturating_sub(1)
            }
        }
        Some(TreeRow::Group { depth, .. }) => depth.saturating_sub(1),
        None => return,
    };

    match direction {
        Direction::Down => {
            for i in (current + 1)..rows.len() {
                if let TreeRow::Node {
                    depth, is_dir, ..
                } = &rows[i]
                {
                    if *is_dir && *depth <= target_depth {
                        state.tree_state.selected = i;
                        return;
                    }
                }
            }
        }
        Direction::Up => {
            for i in (0..current).rev() {
                if let TreeRow::Node {
                    depth, is_dir, ..
                } = &rows[i]
                {
                    if *is_dir && *depth <= target_depth {
                        state.tree_state.selected = i;
                        return;
                    }
                }
            }
        }
    }
}

// ── Settings menu ────────────────────────────────────────────────

fn handle_settings_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
            state.active_view = ActiveView::Tree;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.settings_selected = state.settings_selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.settings_selected < SETTINGS_ITEMS.len() - 1 {
                state.settings_selected += 1;
            }
        }
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
            if state.settings_selected == 0 {
                state.active_view = ActiveView::ControlsSubmenu;
            }
        }
        _ => {}
    }
}

// ── Controls submenu ─────────────────────────────────────────────

fn handle_controls_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
            state.active_view = ActiveView::SettingsMenu;
        }
        KeyCode::Char('q') => {
            state.active_view = ActiveView::Tree;
        }
        _ => {}
    }
}

/// Process a mouse event.
pub fn handle_mouse(state: &mut AppState, mouse: MouseEvent) {
    // Ignore mouse events when a menu overlay is open.
    if state.active_view != ActiveView::Tree {
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Click on a row → select it.
            let clicked_row = mouse.row.saturating_sub(1) as usize + state.tree_state.offset;
            let visible = state.tree.visible_nodes();
            if clicked_row < visible.len() {
                state.tree_state.selected = clicked_row;
            }
        }
        MouseEventKind::ScrollUp => {
            state.tree_state.select_prev();
        }
        MouseEventKind::ScrollDown => {
            let visible_count = state.tree.visible_nodes().len();
            state.tree_state.select_next(visible_count);
        }
        _ => {}
    }
}

// ── helpers ─────────────────────────────────────────────────────

/// Build the flat list of visible rows.
fn build_rows(state: &AppState) -> Vec<TreeRow> {
    TreeWidget::new(&state.tree, &state.grouping_config).build_rows()
}

/// Map the current selection index back to a [`NodeId`].
fn selected_node_id(state: &AppState) -> Option<NodeId> {
    let rows = build_rows(state);
    rows.get(state.tree_state.selected).and_then(|row| match row {
        TreeRow::Node { node_id, .. } => Some(*node_id),
        TreeRow::Group { .. } => None,
    })
}

/// Rebuild the tree from the current cwd (e.g. after toggling hidden files).
fn rebuild_tree(state: &mut AppState) {
    if let Ok(tree) = fs::build_tree(&state.cwd, &state.walk_config) {
        state.tree = tree;
        state.tree_state.selected = 0;
        state.tree_state.offset = 0;
        // Full rebuild changes which files/dirs are in the tree, so clear
        // all cached sizes to avoid stale data.
        state.file_sizes.clear();
        state.dir_local_sums.clear();
        state.needs_size_recompute = true;
    }
}
