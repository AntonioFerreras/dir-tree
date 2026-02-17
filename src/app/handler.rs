//! Input handling — maps key/mouse events to state mutations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::core::fs;
use crate::core::tree::NodeId;

use super::state::AppState;
use crate::ui::tree_widget::TreeWidget;

/// Process a key event, mutating app state as needed.
pub fn handle_key(state: &mut AppState, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // ── Quit ───────────────────────────────────────────────
        (_, KeyCode::Char('q')) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            state.should_quit = true;
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
            }
        }
        (_, KeyCode::Left) | (_, KeyCode::Char('h')) => {
            if let Some(node_id) = selected_node_id(state) {
                state.tree.get_mut(node_id).expanded = false;
            }
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

/// Process a mouse event.
pub fn handle_mouse(state: &mut AppState, mouse: MouseEvent) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Click on a row → select it.  The y coordinate is relative to the
            // terminal, so we subtract the tree area's top (which we approximate
            // as row 1 — inside the border).  A more precise mapping will come
            // once layout info is threaded through.
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

/// Map the current selection index back to a [`NodeId`].
fn selected_node_id(state: &AppState) -> Option<NodeId> {
    let rows = TreeWidget::new(&state.tree, &state.grouping_config).build_rows();
    use crate::ui::tree_widget::TreeRow;
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
    }
}

