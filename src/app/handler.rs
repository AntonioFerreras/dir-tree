//! Input handling — maps key/mouse events to state mutations.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::path::Path;
use std::time::Instant;

use crate::config::{Action, KeyBind};
use crate::core::fs;
use crate::core::tree::NodeId;
use crate::ui::inspector::pinned_cards_geometry;
use crate::ui::layout::AppLayout;

use super::settings::{SettingsItem, SETTINGS_ITEMS};
use super::state::{ActiveView, AppState, PaneFocus, RightPaneTab};
use crate::ui::tree_widget::{TreeRow, TreeWidget};

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
        ActiveView::Lightbox => handle_lightbox_key(state, key),
    }
}

// ── Tree view (configurable bindings) ───────────────────────────

fn handle_tree_key(state: &mut AppState, key: KeyEvent) {
    if is_search_shortcut(key) {
        toggle_search_tab(state);
        return;
    }

    if state.right_pane_tab == RightPaneTab::Search {
        if handle_search_key(state, key) {
            return;
        }
    }

    if key.code == KeyCode::Tab {
        state.pane_focus = match state.pane_focus {
            PaneFocus::Tree => PaneFocus::Inspector,
            PaneFocus::Inspector => PaneFocus::Tree,
        };
        // When tabbing into inspector, reveal selected item in tree.
        if state.pane_focus == PaneFocus::Inspector && state.right_pane_tab == RightPaneTab::Inspector {
            reveal_selected_pin_in_tree(state);
        } else if state.pane_focus == PaneFocus::Inspector && state.right_pane_tab == RightPaneTab::Search {
            reveal_selected_search_in_tree(state);
        }
        return;
    }

    if state.pane_focus == PaneFocus::Inspector {
        // While inspector is focused, tree navigation/actions are disabled.
        // Allow inspector-local keys plus global quit/settings actions.
        if handle_inspector_focus_key(state, key) {
            return;
        }
        if let Some(action) = state.config.match_key(key) {
            match action {
                Action::Quit => state.should_quit = true,
                Action::OpenSettings => {
                    state.active_view = ActiveView::SettingsMenu;
                    state.settings_selected = 0;
                }
                _ => {}
            }
        }
        return;
    }

    // Navigation keys that should always work in tree view.
    match key.code {
        KeyCode::Home => {
            // Root is always the first visible row.
            state.tree_state.selected = 0;
            state.tree_state.offset = 0;
            return;
        }
        KeyCode::End => {
            let rows = build_rows(state);
            if !rows.is_empty() {
                state.tree_state.selected = rows.len() - 1;
            }
            return;
        }
        _ => {}
    }

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
            let visible_count = build_rows(state).len();
            state.tree_state.select_next(visible_count);
        }
        Action::Expand => {
            // Groups: toggle expand/collapse.
            if let Some((key, _)) = selected_group_key(state) {
                toggle_group(state, &key);
            } else {
                // Files: toggle pin. Dirs: expand tree node.
                maybe_pin_selected_non_dir(state);
                if let Some(node_id) = selected_node_id(state) {
                    let t0 = std::time::Instant::now();
                    let _ = fs::expand_node(
                        &mut state.tree,
                        node_id,
                        &state.walk_config,
                        state.config.one_file_system,
                    );
                    state.tree.get_mut(node_id).expanded = true;
                    let path = state.tree.get(node_id).meta.path.clone();
                    state.dir_local_sums.remove(&path);
                    state.needs_size_recompute = true;
                    tracing::debug!("expand_node: {:.2?} path={}", t0.elapsed(), path.display());
                }
            }
        }
        Action::Collapse => {
            // Groups: collapse if expanded, else fall through to normal collapse.
            if let Some((key, expanded)) = selected_group_key(state) {
                if expanded {
                    toggle_group(state, &key);
                    return;
                }
            }
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

    // On the current tree root, "collapse/parent" means move the whole
    // browser root up one level so users can navigate above the launch dir.
    if node_id == state.tree.root {
        move_root_to_parent(state);
        return;
    }

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
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => {
            if let Some(item) = SETTINGS_ITEMS.get(state.settings_selected) {
                match item {
                    SettingsItem::Submenu { view, .. } => {
                        state.active_view = *view;
                        state.controls_selected = 0;
                    }
                    SettingsItem::Toggle { get, set, .. } => {
                        let current = get(state);
                        set(state, !current);
                    }
                    SettingsItem::Cycle { cycle, .. } => {
                        cycle(state);
                    }
                }
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
    if state.active_view == ActiveView::Lightbox {
        handle_lightbox_mouse(state, mouse);
        return;
    }
    if state.active_view != ActiveView::Tree {
        return;
    }

    let layout = AppLayout::from_area(
        state.terminal_area,
        state.config.panel_layout,
        state.config.panel_split_pct,
    );

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if layout.is_on_splitter(mouse.column, mouse.row) {
                state.dragging_splitter = true;
                return;
            }
            state.dragging_splitter = false;

            if point_in_rect(layout.inspector_area, mouse.column, mouse.row) {
                state.pane_focus = PaneFocus::Inspector;
                handle_inspector_click(state, layout.inspector_area, mouse.column, mouse.row);
                return;
            }

            if !point_in_rect(layout.tree_area, mouse.column, mouse.row) {
                return;
            }
            state.pane_focus = PaneFocus::Tree;
            let tree_content_top = layout.tree_area.y.saturating_add(1);
            let tree_content_bottom = layout
                .tree_area
                .y
                .saturating_add(layout.tree_area.height.saturating_sub(1));
            if mouse.row < tree_content_top || mouse.row >= tree_content_bottom {
                return;
            }

            let clicked_row = mouse.row.saturating_sub(tree_content_top) as usize + state.tree_state.offset;
            let rows = build_rows(state);
            if clicked_row < rows.len() {
                state.tree_state.selected = clicked_row;

                let now = Instant::now();
                let is_repeat_click = |state: &AppState, nid: NodeId| -> bool {
                    state
                        .last_left_click
                        .as_ref()
                        .map(|(last_id, at)| {
                            *last_id == nid
                                && now.duration_since(*at)
                                    <= std::time::Duration::from_millis(
                                        state.config.double_click_ms,
                                    )
                        })
                        .unwrap_or(false)
                };

                if let Some(TreeRow::Node {
                    node_id, is_dir, ..
                }) = rows.get(clicked_row)
                {
                    if *is_dir {
                        if is_repeat_click(state, *node_id) {
                            let node = state.tree.get(*node_id);
                            state.selected_dir = Some(node.meta.path.clone());
                            state.should_quit = true;
                            state.last_left_click = None;
                            return;
                        }

                        toggle_dir_with_click(state, *node_id);
                        state.last_left_click = Some((*node_id, now));
                    } else {
                        // Second click on the same file toggles its pin.
                        if is_repeat_click(state, *node_id) {
                            toggle_pin_for_node(state, *node_id);
                            state.last_left_click = None;
                        } else {
                            state.last_left_click = Some((*node_id, now));
                        }
                    }
                } else if let Some(TreeRow::Group { group_key, .. }) =
                    rows.get(clicked_row)
                {
                    // Clicking a group row toggles its expand state.
                    let key = group_key.clone();
                    toggle_group(state, &key);
                    state.last_left_click = None;
                } else {
                    state.last_left_click = None;
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if state.dragging_splitter {
                if let Some(pct) = layout.split_pct_from_pointer(mouse.column, mouse.row) {
                    state.config.panel_split_pct = pct;
                    let _ = state.config.save();
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            state.dragging_splitter = false;
        }
        MouseEventKind::ScrollUp => {
            if point_in_rect(layout.inspector_area, mouse.column, mouse.row)
                || state.pane_focus == PaneFocus::Inspector
            {
                if state.right_pane_tab == RightPaneTab::Search {
                    if state.search_selected > 0 {
                        state.search_selected -= 1;
                        reveal_selected_search_in_tree(state);
                    }
                    return;
                }
                if state.inspector_pin_scroll > 0 {
                    state.inspector_pin_scroll -= 1;
                }
                return;
            }
            state.tree_state.select_prev();
        }
        MouseEventKind::ScrollDown => {
            if point_in_rect(layout.inspector_area, mouse.column, mouse.row)
                || state.pane_focus == PaneFocus::Inspector
            {
                if state.right_pane_tab == RightPaneTab::Search {
                    if state.search_selected + 1 < state.search_results.len() {
                        state.search_selected += 1;
                        reveal_selected_search_in_tree(state);
                    }
                    return;
                }
                let geom = inspector_geom(state);
                state.inspector_pin_scroll =
                    (state.inspector_pin_scroll + 1).min(geom.max_scroll);
                return;
            }
            let visible_count = build_rows(state).len();
            state.tree_state.select_next(visible_count);
        }
        _ => {}
    }
}

// ── Lightbox ────────────────────────────────────────────────────

fn handle_lightbox_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('x') => {
            state.active_view = ActiveView::Tree;
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Up | KeyCode::Char('k') => {
            lightbox_prev(state);
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Down | KeyCode::Char('j') => {
            lightbox_next(state);
        }
        KeyCode::Enter => {
            state.active_view = ActiveView::Tree;
        }
        _ => {}
    }
}

fn handle_lightbox_mouse(state: &mut AppState, mouse: MouseEvent) {
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        if let Some(zones) = state.lightbox_hit_zones {
            if point_in_rect(zones.close_rect, mouse.column, mouse.row) {
                state.active_view = ActiveView::Tree;
                return;
            }
            if point_in_rect(zones.prev_rect, mouse.column, mouse.row) {
                lightbox_prev(state);
                return;
            }
            if point_in_rect(zones.next_rect, mouse.column, mouse.row) {
                lightbox_next(state);
                return;
            }
        }
    }
}

/// Navigate to the previous pinned image in the lightbox.
fn lightbox_prev(state: &mut AppState) {
    let image_indices: Vec<usize> = state
        .pinned_inspector
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_image())
        .map(|(i, _)| i)
        .collect();
    if let Some(pos) = image_indices.iter().position(|&i| i == state.lightbox_index) {
        if pos > 0 {
            state.lightbox_index = image_indices[pos - 1];
        }
    }
}

/// Navigate to the next pinned image in the lightbox.
fn lightbox_next(state: &mut AppState) {
    let image_indices: Vec<usize> = state
        .pinned_inspector
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_image())
        .map(|(i, _)| i)
        .collect();
    if let Some(pos) = image_indices.iter().position(|&i| i == state.lightbox_index) {
        if pos + 1 < image_indices.len() {
            state.lightbox_index = image_indices[pos + 1];
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────

/// Reveal the currently selected pinned file in the tree panel.
fn reveal_selected_pin_in_tree(state: &mut AppState) {
    if state.pinned_inspector.is_empty() {
        return;
    }
    let idx = state.inspector_selected_pin.min(state.pinned_inspector.len().saturating_sub(1));
    let target_path = state.pinned_inspector[idx].path.clone();
    reveal_path_in_tree(state, &target_path);
}

/// Ensure `target` is visible in the tree by:
/// 1. Moving the root upward if the target is outside the current tree.
/// 2. Expanding each ancestor directory along the path.
/// 3. Selecting the target's row in the tree widget.
fn reveal_path_in_tree(state: &mut AppState, target: &std::path::Path) {
    // Step 1: If the file is not under the current cwd, move root up.
    if !target.starts_with(&state.cwd) {
        // Find the deepest common ancestor.
        let mut ancestor = target.to_path_buf();
        loop {
            if let Some(parent) = ancestor.parent() {
                ancestor = parent.to_path_buf();
                if state.cwd.starts_with(&ancestor) && target.starts_with(&ancestor) {
                    break;
                }
            } else {
                break;
            }
        }
        // Rebuild tree from the common ancestor.
        if let Ok(tree) = fs::build_tree(&ancestor, &state.walk_config, state.config.one_file_system) {
            state.cwd = ancestor;
            state.tree = tree;
            state.tree_state.selected = 0;
            state.tree_state.offset = 0;
            state.dir_sizes.clear();
            state.file_sizes.clear();
            state.dir_local_sums.clear();
            state.needs_size_recompute = true;
            state.search_root = state.cwd.clone();
            state.search_index.clear();
        } else {
            return;
        }
    }

    // Step 2: Walk from cwd to the target, expanding each directory.
    // Collect the chain of ancestor paths between cwd and the target's parent.
    let mut dirs_to_expand = Vec::new();
    {
        let mut p = target.parent();
        while let Some(dir) = p {
            if dir == state.cwd.as_path() {
                break;
            }
            dirs_to_expand.push(dir.to_path_buf());
            p = dir.parent();
        }
        dirs_to_expand.reverse(); // from shallowest to deepest
    }

    for dir_path in &dirs_to_expand {
        // Find the node with this path.
        let node_id = state
            .tree
            .nodes
            .iter()
            .enumerate()
            .find(|(_, n)| n.meta.path == *dir_path)
            .map(|(i, _)| i);

        if let Some(nid) = node_id {
            // Expand it (lazy-load children if needed).
            let _ = fs::expand_node(
                &mut state.tree,
                nid,
                &state.walk_config,
                state.config.one_file_system,
            );
            state.tree.get_mut(nid).expanded = true;
            let path = state.tree.get(nid).meta.path.clone();
            state.dir_local_sums.remove(&path);
        }
    }
    state.needs_size_recompute = true;

    // Step 3: If the file is inside a collapsed group, expand that group.
    // Build rows and check: if the target isn't found as a Node row, look
    // for a Group whose members include it and expand that group.
    loop {
        let rows = build_rows(state);
        // Try to find the target as a visible Node row.
        let found = rows.iter().enumerate().find(|(_, row)| {
            if let TreeRow::Node { node_id, .. } = row {
                state.tree.get(*node_id).meta.path == *target
            } else {
                false
            }
        });
        if let Some((i, _)) = found {
            state.tree_state.selected = i;
            break;
        }

        // Not found — look for a Group that contains it as a member.
        let group_to_expand = rows.iter().find_map(|row| {
            if let TreeRow::Group {
                group_key,
                members,
                expanded,
                ..
            } = row
            {
                if !expanded {
                    let has_member = members.iter().any(|&mid| {
                        state.tree.get(mid).meta.path == *target
                    });
                    if has_member {
                        return Some(group_key.clone());
                    }
                }
                None
            } else {
                None
            }
        });

        if let Some(key) = group_to_expand {
            state.expanded_groups.insert(key);
            // Loop again — now the group is expanded and the file should be visible.
        } else {
            // Neither a visible node nor inside any group — give up.
            break;
        }
    }
}

fn build_rows(state: &AppState) -> Vec<TreeRow> {
    TreeWidget::new(&state.tree, &state.grouping_config)
        .expanded_groups(&state.expanded_groups)
        .build_rows()
}

fn selected_node_id(state: &AppState) -> Option<NodeId> {
    let rows = build_rows(state);
    rows.get(state.tree_state.selected).and_then(|row| match row {
        TreeRow::Node { node_id, .. } => Some(*node_id),
        TreeRow::Group { .. } => None,
    })
}

/// If the selected row is a Group, return its key and expanded state.
fn selected_group_key(state: &AppState) -> Option<(String, bool)> {
    let rows = build_rows(state);
    rows.get(state.tree_state.selected).and_then(|row| match row {
        TreeRow::Group {
            group_key,
            expanded,
            ..
        } => Some((group_key.clone(), *expanded)),
        _ => None,
    })
}

/// Toggle expand state of a group by key.
fn toggle_group(state: &mut AppState, key: &str) {
    if state.expanded_groups.contains(key) {
        state.expanded_groups.remove(key);
    } else {
        state.expanded_groups.insert(key.to_string());
    }
}

/// Selected tree entry path, if the currently selected row is a node.
pub fn selected_node_path(state: &AppState) -> Option<std::path::PathBuf> {
    selected_node_id(state).map(|id| state.tree.get(id).meta.path.clone())
}

fn toggle_dir_with_click(state: &mut AppState, node_id: NodeId) {
    if !state.tree.get(node_id).meta.is_dir {
        return;
    }

    // Keep mouse behavior consistent with keyboard collapse:
    // collapsing the current root should navigate to its parent.
    if node_id == state.tree.root {
        move_root_to_parent(state);
        return;
    }

    if state.tree.get(node_id).expanded {
        state.tree.get_mut(node_id).expanded = false;
        return;
    }

    let t0 = std::time::Instant::now();
    let _ = fs::expand_node(
        &mut state.tree,
        node_id,
        &state.walk_config,
        state.config.one_file_system,
    );
    state.tree.get_mut(node_id).expanded = true;

    // Invalidate only this dir's cached local_sum — its children moved
    // from non-tree to tree, changing how bytes are counted.
    let path = state.tree.get(node_id).meta.path.clone();
    state.dir_local_sums.remove(&path);
    state.needs_size_recompute = true;
    tracing::debug!("expand_node(click): {:.2?} path={}", t0.elapsed(), path.display());
}

fn handle_inspector_focus_key(state: &mut AppState, key: KeyEvent) -> bool {
    if state.right_pane_tab == RightPaneTab::Search {
        return handle_search_key(state, key);
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if state.inspector_selected_pin > 0 {
                state.inspector_selected_pin -= 1;
                clamp_inspector_selection_and_scroll(state);
                reveal_selected_pin_in_tree(state);
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !state.pinned_inspector.is_empty()
                && state.inspector_selected_pin + 1 < state.pinned_inspector.len()
            {
                state.inspector_selected_pin += 1;
                clamp_inspector_selection_and_scroll(state);
                reveal_selected_pin_in_tree(state);
            }
            true
        }
        KeyCode::Home => {
            if !state.pinned_inspector.is_empty() {
                state.inspector_selected_pin = 0;
                clamp_inspector_selection_and_scroll(state);
                reveal_selected_pin_in_tree(state);
            }
            true
        }
        KeyCode::End => {
            if !state.pinned_inspector.is_empty() {
                state.inspector_selected_pin = state.pinned_inspector.len() - 1;
                clamp_inspector_selection_and_scroll(state);
                reveal_selected_pin_in_tree(state);
            }
            true
        }
        KeyCode::Enter => {
            // Open lightbox if the selected pinned card is an image.
            if !state.pinned_inspector.is_empty() {
                let idx = state.inspector_selected_pin;
                if idx < state.pinned_inspector.len()
                    && state.pinned_inspector[idx].is_image()
                {
                    state.lightbox_index = idx;
                    state.active_view = ActiveView::Lightbox;
                }
            }
            true
        }
        KeyCode::Delete | KeyCode::Backspace => {
            remove_selected_pin(state);
            true
        }
        _ => false,
    }
}

fn handle_inspector_click(state: &mut AppState, inspector_area: ratatui::layout::Rect, col: u16, row: u16) {
    if state.right_pane_tab == RightPaneTab::Search {
        handle_search_click(state, inspector_area, row);
        return;
    }

    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(inspector_area);
    let geom = pinned_cards_geometry(
        inner,
        state.inspector_info.as_ref(),
        &state.pinned_inspector,
        state.inspector_pin_scroll,
    );

    for card in geom.cards {
        if point_in_rect(card.unpin_rect, col, row) {
            remove_pin_at(state, card.pin_index);
            return;
        }
        if point_in_rect(card.card_rect, col, row) {
            state.inspector_selected_pin = card.pin_index;
            clamp_inspector_selection_and_scroll(state);
            reveal_selected_pin_in_tree(state);
            return;
        }
    }
}

/// Sync pinned paths from `pinned_inspector` to `config.pinned_paths` and persist.
fn persist_pins(state: &mut AppState) {
    state.config.pinned_paths = state
        .pinned_inspector
        .iter()
        .map(|info| info.path.display().to_string())
        .collect();
    let _ = state.config.save();
}

/// Toggle pin state for a given node: unpin if already pinned, pin if not.
fn toggle_pin_for_node(state: &mut AppState, node_id: NodeId) {
    if state.tree.get(node_id).meta.is_dir {
        return;
    }
    let path = state.tree.get(node_id).meta.path.clone();
    toggle_pin_for_path(state, &path);
}

fn maybe_pin_selected_non_dir(state: &mut AppState) {
    let Some(node_id) = selected_node_id(state) else {
        return;
    };
    toggle_pin_for_node(state, node_id);
}

fn remove_selected_pin(state: &mut AppState) {
    if state.pinned_inspector.is_empty() {
        return;
    }
    remove_pin_at(state, state.inspector_selected_pin);
}

fn remove_pin_at(state: &mut AppState, index: usize) {
    if index >= state.pinned_inspector.len() {
        return;
    }
    state.pinned_inspector.remove(index);
    if state.inspector_selected_pin >= state.pinned_inspector.len() && !state.pinned_inspector.is_empty() {
        state.inspector_selected_pin = state.pinned_inspector.len() - 1;
    }
    if state.pinned_inspector.is_empty() {
        state.inspector_selected_pin = 0;
        state.inspector_pin_scroll = 0;
    } else {
        clamp_inspector_selection_and_scroll(state);
    }
    persist_pins(state);
}

fn inspector_geom(state: &AppState) -> crate::ui::inspector::PinnedCardsGeometry {
    let layout = AppLayout::from_area(
        state.terminal_area,
        state.config.panel_layout,
        state.config.panel_split_pct,
    );
    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(layout.inspector_area);
    pinned_cards_geometry(
        inner,
        state.inspector_info.as_ref(),
        &state.pinned_inspector,
        state.inspector_pin_scroll,
    )
}

fn clamp_inspector_selection_and_scroll(state: &mut AppState) {
    if state.pinned_inspector.is_empty() {
        state.inspector_selected_pin = 0;
        state.inspector_pin_scroll = 0;
        return;
    }

    if state.inspector_selected_pin >= state.pinned_inspector.len() {
        state.inspector_selected_pin = state.pinned_inspector.len() - 1;
    }

    let geom = inspector_geom(state);
    state.inspector_pin_scroll = state.inspector_pin_scroll.min(geom.max_scroll);

    if geom.visible_cards == 0 {
        state.inspector_pin_scroll = 0;
        return;
    }

    if state.inspector_selected_pin < state.inspector_pin_scroll {
        state.inspector_pin_scroll = state.inspector_selected_pin;
    } else {
        let last_visible = state.inspector_pin_scroll + geom.visible_cards - 1;
        if state.inspector_selected_pin > last_visible {
            state.inspector_pin_scroll = state
                .inspector_selected_pin
                .saturating_sub(geom.visible_cards.saturating_sub(1));
        }
    }
    state.inspector_pin_scroll = state.inspector_pin_scroll.min(geom.max_scroll);
}

fn rebuild_tree(state: &mut AppState) {
    if let Ok(tree) = fs::build_tree(&state.cwd, &state.walk_config, state.config.one_file_system) {
        state.tree = tree;
        state.tree_state.selected = 0;
        state.tree_state.offset = 0;
        state.dir_sizes.clear();
        state.file_sizes.clear();
        state.dir_local_sums.clear();
        state.needs_size_recompute = true;
        state.search_root = state.cwd.clone();
        state.search_index.clear();
        refresh_search_results(state);
    }
}

fn move_root_to_parent(state: &mut AppState) {
    let Some(parent) = state.cwd.parent().map(|p| p.to_path_buf()) else {
        state.status_message = Some("Already at filesystem root".to_string());
        return;
    };

    match fs::build_tree(&parent, &state.walk_config, state.config.one_file_system) {
        Ok(tree) => {
            state.cwd = parent;
            state.tree = tree;
            state.tree_state.selected = 0;
            state.tree_state.offset = 0;
            state.dir_sizes.clear();
            state.file_sizes.clear();
            state.dir_local_sums.clear();
            state.needs_size_recompute = true;
            state.status_message = Some(format!("Moved to parent: {}", state.cwd.display()));
            state.search_root = state.cwd.clone();
            state.search_index.clear();
            refresh_search_results(state);
        }
        Err(_) => {
            state.status_message = Some("Cannot open parent directory".to_string());
        }
    }
}

fn point_in_rect(area: ratatui::layout::Rect, col: u16, row: u16) -> bool {
    col >= area.x
        && col < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn is_search_shortcut(key: KeyEvent) -> bool {
    (key.code == KeyCode::Char('/') && key.modifiers.is_empty())
        || (key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL))
}

fn toggle_search_tab(state: &mut AppState) {
    if state.right_pane_tab == RightPaneTab::Search {
        state.right_pane_tab = state.right_pane_prev_tab;
        return;
    }

    state.right_pane_prev_tab = state.right_pane_tab;
    state.right_pane_tab = RightPaneTab::Search;
    state.pane_focus = PaneFocus::Inspector;
    ensure_search_index(state);
    refresh_search_results(state);
    reveal_selected_search_in_tree(state);
}

fn handle_search_key(state: &mut AppState, key: KeyEvent) -> bool {
    if state.right_pane_tab != RightPaneTab::Search {
        return false;
    }

    match key.code {
        KeyCode::Esc => {
            state.right_pane_tab = state.right_pane_prev_tab;
            true
        }
        KeyCode::Up => {
            if state.search_selected > 0 {
                state.search_selected -= 1;
                reveal_selected_search_in_tree(state);
            }
            true
        }
        KeyCode::Down => {
            if state.search_selected + 1 < state.search_results.len() {
                state.search_selected += 1;
                reveal_selected_search_in_tree(state);
            }
            true
        }
        KeyCode::Home => {
            if !state.search_results.is_empty() {
                state.search_selected = 0;
                reveal_selected_search_in_tree(state);
            }
            true
        }
        KeyCode::End => {
            if !state.search_results.is_empty() {
                state.search_selected = state.search_results.len() - 1;
                reveal_selected_search_in_tree(state);
            }
            true
        }
        KeyCode::Backspace => {
            state.search_query.pop();
            refresh_search_results(state);
            true
        }
        KeyCode::Char('c') if key.modifiers == KeyModifiers::ALT => {
            state.search_case_sensitive = !state.search_case_sensitive;
            refresh_search_results(state);
            true
        }
        KeyCode::Char(ch) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            state.search_query.push(ch);
            refresh_search_results(state);
            true
        }
        _ => {
            if let Some(action) = state.config.match_key(key) {
                if action == Action::Expand {
                    let selected_path = state
                        .search_results
                        .get(state.search_selected)
                        .map(|r| r.path.clone());
                    if let Some(path) = selected_path {
                        toggle_pin_for_path(state, &path);
                    }
                    return true;
                }
            }
            false
        }
    }
}

fn ensure_search_index(state: &mut AppState) {
    if state.search_root != state.cwd || state.search_index.is_empty() {
        state.search_root = state.cwd.clone();
        state.search_index = crate::core::search::build_index(
            &state.search_root,
            state.walk_config.show_hidden,
            state.walk_config.respect_gitignore,
            state.config.one_file_system,
        );
    }
}

fn refresh_search_results(state: &mut AppState) {
    ensure_search_index(state);
    state.search_results = crate::core::search::search_entries(
        &state.search_index,
        &state.search_query,
        state.search_case_sensitive,
        300,
    );
    if state.search_results.is_empty() {
        state.search_selected = 0;
    } else {
        state.search_selected = state.search_selected.min(state.search_results.len() - 1);
    }
}

fn reveal_selected_search_in_tree(state: &mut AppState) {
    if state.search_results.is_empty() {
        return;
    }
    if let Some(path) = state.search_results.get(state.search_selected).map(|r| r.path.clone()) {
        reveal_path_in_tree(state, &path);
    }
}

fn toggle_pin_for_path(state: &mut AppState, path: &Path) {
    // Already pinned -> unpin.
    if let Some((idx, _)) = state
        .pinned_inspector
        .iter()
        .enumerate()
        .find(|(_, info)| info.path == path)
    {
        remove_pin_at(state, idx);
        return;
    }

    // Only files are pinnable.
    if std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(true) {
        return;
    }

    let mut info = crate::core::inspector::inspect_path(path);
    if let Some(sz) = state.dir_sizes.get(path).copied() {
        info.size_bytes = Some(sz);
    } else if let Some(sz) = state.file_sizes.get(path).copied() {
        info.size_bytes = Some(sz);
    }
    state.pinned_inspector.push(info);
    state.inspector_selected_pin = state.pinned_inspector.len().saturating_sub(1);
    clamp_inspector_selection_and_scroll(state);
    persist_pins(state);
}

fn handle_search_click(state: &mut AppState, inspector_area: ratatui::layout::Rect, row: u16) {
    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(inspector_area);
    let results_start = inner.y.saturating_add(4);
    if row < results_start {
        return;
    }
    let idx = row.saturating_sub(results_start) as usize;
    if idx < state.search_results.len() {
        state.search_selected = idx;
        reveal_selected_search_in_tree(state);
    }
}
