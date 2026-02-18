//! Central application state.
//!
//! All mutable state lives here so that the rest of the app can be pure
//! functions over `&AppState` (rendering) or `&mut AppState` (event handling).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::AppConfig;
use crate::core::{
    fs::WalkConfig,
    grouping::GroupingConfig,
    tree::{DirTree, NodeId},
};
use crate::ui::tree_widget::TreeWidgetState;

/// Which view / overlay is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveView {
    #[default]
    Tree,
    SettingsMenu,
    ControlsSubmenu,
}

/// Top-level application state.
pub struct AppState {
    /// The directory tree data.
    pub tree: DirTree,
    /// Widget-level state (selection, scroll).
    pub tree_state: TreeWidgetState,
    /// Walk configuration (depth, hidden files, etc.).
    pub walk_config: WalkConfig,
    /// Grouping configuration.
    pub grouping_config: GroupingConfig,
    /// Current working directory (the tree root).
    pub cwd: PathBuf,
    /// When the user selects a directory and confirms, we store it here so
    /// the shell wrapper can `cd` to it.
    pub selected_dir: Option<PathBuf>,
    /// Controls the main event loop.
    pub should_quit: bool,
    /// An optional status message shown in the bottom bar.
    pub status_message: Option<String>,
    /// Which view / overlay is currently shown.
    pub active_view: ActiveView,
    /// User-configurable keybindings.
    pub config: AppConfig,
    /// Currently highlighted item in the settings menu.
    pub settings_selected: usize,
    /// Currently highlighted item in the controls submenu.
    pub controls_selected: usize,
    /// When `true`, the controls submenu is waiting for the user to press
    /// a key to rebind the action at `controls_selected`.
    pub awaiting_rebind: bool,
    /// Computed directory sizes (path → total bytes).  Populated asynchronously
    /// by a background thread.
    pub dir_sizes: HashMap<PathBuf, u64>,
    /// Computed file sizes (path → bytes). Populated asynchronously.
    pub file_sizes: HashMap<PathBuf, u64>,
    /// Cached per-directory local walk results from workers.  On expand, only
    /// the expanded dir's entry is invalidated — all others survive so we
    /// skip redundant I/O.
    pub dir_local_sums: HashMap<PathBuf, crate::core::size::DirLocalResult>,
    /// Flag set by event handlers to trigger a background size recomputation.
    pub needs_size_recompute: bool,
    /// Monotonic generation id used to ignore stale background size updates.
    pub size_compute_generation: u64,
    /// `true` while background size workers are still running.
    pub scanning: bool,
    /// Last left-clicked directory node and click time, for double-click.
    pub last_left_click: Option<(NodeId, std::time::Instant)>,
}

impl AppState {
    pub fn new(cwd: PathBuf, tree: DirTree, config: AppConfig) -> Self {
        Self {
            tree,
            tree_state: TreeWidgetState::default(),
            walk_config: WalkConfig::default(),
            grouping_config: GroupingConfig::default(),
            cwd,
            selected_dir: None,
            should_quit: false,
            status_message: None,
            active_view: ActiveView::default(),
            config,
            settings_selected: 0,
            controls_selected: 0,
            awaiting_rebind: false,
            dir_sizes: HashMap::new(),
            file_sizes: HashMap::new(),
            dir_local_sums: HashMap::new(),
            needs_size_recompute: false,
            size_compute_generation: 0,
            scanning: false,
            last_left_click: None,
        }
    }
}

