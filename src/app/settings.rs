//! Settings menu model (data only).
//!
//! Keeping these definitions outside the input handler lets both the handler
//! and UI renderers consume the same source of truth without cross-importing.

use super::state::{ActiveView, AppState};
use crate::config::PanelLayoutMode;

/// A single item in the settings menu.
pub enum SettingsItem {
    /// Opens a submenu.
    Submenu {
        label: &'static str,
        view: ActiveView,
    },
    /// Boolean toggle — reads/writes via accessors on `AppState`.
    Toggle {
        label: &'static str,
        get: fn(&AppState) -> bool,
        set: fn(&mut AppState, bool),
    },
    /// Cycles through a finite set of values.
    Cycle {
        label: &'static str,
        value: fn(&AppState) -> String,
        cycle: fn(&mut AppState),
    },
}

impl SettingsItem {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Submenu { label, .. }
            | Self::Toggle { label, .. }
            | Self::Cycle { label, .. } => label,
        }
    }
}

/// All items shown in the settings popup, in display order.
pub static SETTINGS_ITEMS: &[SettingsItem] = &[
    SettingsItem::Submenu {
        label: "Controls",
        view: ActiveView::ControlsSubmenu,
    },
    SettingsItem::Toggle {
        label: "Dedup Hard Links",
        get: |s| s.config.dedup_hard_links,
        set: |s, v| {
            s.config.dedup_hard_links = v;
            let _ = s.config.save();
            s.dir_local_sums.clear();
            s.needs_size_recompute = true;
        },
    },
    SettingsItem::Toggle {
        label: "One File System",
        get: |s| s.config.one_file_system,
        set: |s, v| {
            s.config.one_file_system = v;
            let _ = s.config.save();
            // Full rebuild needed since directory visibility changes.
            if let Ok(tree) = crate::core::fs::build_tree(
                &s.cwd,
                &s.walk_config,
                s.config.one_file_system,
            ) {
                s.tree = tree;
                s.tree_state.selected = 0;
                s.tree_state.offset = 0;
                s.file_sizes.clear();
                s.dir_local_sums.clear();
                s.needs_size_recompute = true;
            }
        },
    },
    SettingsItem::Cycle {
        label: "Double-click Window",
        value: |s| format!("{}ms", s.config.double_click_ms),
        cycle: |s| {
            const WINDOWS: &[u64] = &[150, 200, 250, 300, 400, 500];
            let current = s.config.double_click_ms;
            let idx = WINDOWS.iter().position(|&w| w == current).unwrap_or(2);
            let next = WINDOWS[(idx + 1) % WINDOWS.len()];
            s.config.double_click_ms = next;
            let _ = s.config.save();
            s.status_message = Some(format!("Double-click window: {}ms", next));
        },
    },
    SettingsItem::Cycle {
        label: "Panel Layout",
        value: |s| s.config.panel_layout.label().to_string(),
        cycle: |s| {
            let idx = PanelLayoutMode::ALL
                .iter()
                .position(|m| *m == s.config.panel_layout)
                .unwrap_or(0);
            s.config.panel_layout = PanelLayoutMode::ALL[(idx + 1) % PanelLayoutMode::ALL.len()];
            let _ = s.config.save();
            s.status_message = Some(format!("Layout: {}", s.config.panel_layout.label()));
        },
    },
    SettingsItem::Cycle {
        label: "Panel Split",
        value: |s| format!("{}%", s.config.panel_split_pct),
        cycle: |s| {
            const SPLITS: &[u16] = &[30, 40, 50, 60, 70];
            let idx = SPLITS
                .iter()
                .position(|p| *p == s.config.panel_split_pct)
                .unwrap_or(3);
            s.config.panel_split_pct = SPLITS[(idx + 1) % SPLITS.len()];
            let _ = s.config.save();
            s.status_message = Some(format!("Panel split: {}%", s.config.panel_split_pct));
        },
    },
];

