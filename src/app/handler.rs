//! Input handling — maps key/mouse events to state mutations.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::config::{Action, KeyBind};
use crate::core::fs;
use crate::core::tree::NodeId;

use super::state::{ActiveView, AppState};
use crate::ui::tree_widget::{TreeRow, TreeWidget};

/// Menu items shown in the settings popup.
pub const SETTINGS_ITEMS: &[&str] = &["Controls"];

/// Total selectable rows in the controls submenu (actions + "Reset").
pub fn controls_item_count() -> usize {
    Action::ALL.len() + 1
}

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
        ActiveView::ControlsSubmenu => {
            if state.awaiting_rebind {
                handle_rebind_key(state, key);
            } else {
                handle_controls_key(state, key);
            }
        }
    }
}

// ── Tree view (configurable bindings) ───────────────────────────

fn handle_tree_key(state: &mut AppState, key: KeyEvent) {
    let Some(action) = state.config.match_key(key) else {
        return;
    };

    match action {
        Action::Quit => {
            state.should_quit = true;
        }
        Action::OpenSettings => {
            state.active_view = ActiveView::SettingsMenu;
            state.settings_selected = 0;
        }
        Action::MoveUp => {
            state.tree_state.select_prev();
        }
        Action::MoveDown => {
            let visible_count = state.tree.visible_nodes().len();
            state.tree_state.select_next(visible_count);
        }
        Action::Expand => {
            if let Some(node_id) = selected_node_id(state) {
                let t0 = std::time::Instant::now();
                let _ = fs::expand_node(&mut state.tree, node_id, &state.walk_config);
                state.tree.get_mut(node_id).expanded = true;
                // Invalidate only this dir's cached local_sum — its children
                // moved from non-tree to tree, changing how bytes are counted.
                // All other dirs' caches remain valid.
                let path = state.tree.get(node_id).meta.path.clone();
                state.dir_local_sums.remove(&path);
                state.needs_size_recompute = true;
                tracing::debug!("expand_node: {:.2?} path={}", t0.elapsed(), path.display());
            }
        }
        Action::Collapse => {
            handle_collapse(state);
        }
        Action::JumpSiblingUp => {
            jump_to_sibling_dir(state, Direction::Up);
        }
        Action::JumpSiblingDown => {
            jump_to_sibling_dir(state, Direction::Down);
        }
        Action::CdIntoDir => {
            if let Some(node_id) = selected_node_id(state) {
                let node = state.tree.get(node_id);
                if node.meta.is_dir {
                    state.selected_dir = Some(node.meta.path.clone());
                    state.should_quit = true;
                }
            }
        }
        Action::ToggleHidden => {
            state.walk_config.show_hidden = !state.walk_config.show_hidden;
            rebuild_tree(state);
        }
    }
}

/// Handle collapse: collapse expanded dir, or go to parent for files/collapsed dirs.
fn handle_collapse(state: &mut AppState) {
    let Some(node_id) = selected_node_id(state) else {
        return;
    };

    let node = state.tree.get(node_id);

    if node.meta.is_dir && node.expanded {
        state.tree.get_mut(node_id).expanded = false;
    } else if let Some(parent_id) = state.tree.get(node_id).parent {
        state.tree.get_mut(parent_id).expanded = false;
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

// ── Settings menu (hardcoded keys) ──────────────────────────────

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
                state.controls_selected = 0;
            }
        }
        _ => {}
    }
}

// ── Controls submenu (hardcoded navigation, interactive rebinding) ──

fn handle_controls_key(state: &mut AppState, key: KeyEvent) {
    let item_count = controls_item_count();

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.active_view = ActiveView::Tree;
        }
        KeyCode::Left | KeyCode::Char('h') => {
            state.active_view = ActiveView::SettingsMenu;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.controls_selected = state.controls_selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.controls_selected < item_count - 1 {
                state.controls_selected += 1;
            }
        }
        KeyCode::Enter => {
            if state.controls_selected < Action::ALL.len() {
                // Start rebinding the selected action.
                state.awaiting_rebind = true;
            } else {
                // "Reset to defaults" item.
                state.config.reset_defaults();
                let _ = state.config.save();
            }
        }
        KeyCode::Delete | KeyCode::Backspace => {
            // Clear all bindings for the selected action.
            if state.controls_selected < Action::ALL.len() {
                let action = Action::ALL[state.controls_selected];
                state.config.bindings.insert(action, Vec::new());
                let _ = state.config.save();
            }
        }
        _ => {}
    }
}

/// Capture the next key press as a new binding.
fn handle_rebind_key(state: &mut AppState, key: KeyEvent) {
    // Only process Press events (ignore Release/Repeat on supported terminals).
    if key.kind != KeyEventKind::Press {
        return;
    }

    // Esc cancels rebinding.
    if key.code == KeyCode::Esc {
        state.awaiting_rebind = false;
        return;
    }

    // Don't allow rebinding Ctrl+C (reserved for emergency quit).
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return;
    }

    let action = Action::ALL[state.controls_selected];
    let bind = KeyBind::from_key_event(key);
    state.config.add_binding(action, bind);
    let _ = state.config.save();
    state.awaiting_rebind = false;
}

// ── Mouse ───────────────────────────────────────────────────────

/// Process a mouse event.
pub fn handle_mouse(state: &mut AppState, mouse: MouseEvent) {
    if state.active_view != ActiveView::Tree {
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
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

fn build_rows(state: &AppState) -> Vec<TreeRow> {
    TreeWidget::new(&state.tree, &state.grouping_config).build_rows()
}

fn selected_node_id(state: &AppState) -> Option<NodeId> {
    let rows = build_rows(state);
    rows.get(state.tree_state.selected).and_then(|row| match row {
        TreeRow::Node { node_id, .. } => Some(*node_id),
        TreeRow::Group { .. } => None,
    })
}

fn rebuild_tree(state: &mut AppState) {
    if let Ok(tree) = fs::build_tree(&state.cwd, &state.walk_config) {
        state.tree = tree;
        state.tree_state.selected = 0;
        state.tree_state.offset = 0;
        state.file_sizes.clear();
        state.dir_local_sums.clear();
        state.needs_size_recompute = true;
    }
}
