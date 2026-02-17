//! Central application state.
//!
//! All mutable state lives here so that the rest of the app can be pure
//! functions over `&AppState` (rendering) or `&mut AppState` (event handling).

use std::path::PathBuf;

use crate::core::{
    fs::WalkConfig,
    grouping::GroupingConfig,
    tree::DirTree,
};
use crate::ui::tree_widget::TreeWidgetState;

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
}

impl AppState {
    pub fn new(cwd: PathBuf, tree: DirTree) -> Self {
        Self {
            tree,
            tree_state: TreeWidgetState::default(),
            walk_config: WalkConfig::default(),
            grouping_config: GroupingConfig::default(),
            cwd,
            selected_dir: None,
            should_quit: false,
            status_message: None,
        }
    }
}

