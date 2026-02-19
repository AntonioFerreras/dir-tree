//! Central application state.
//!
//! All mutable state lives here so that the rest of the app can be pure
//! functions over `&AppState` (rendering) or `&mut AppState` (event handling).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::AppConfig;
use crate::core::{
    fs::WalkConfig,
    grouping::GroupingConfig,
    inspector::InspectorInfo,
    tree::{DirTree, NodeId},
};
use crate::ui::tree_widget::TreeWidgetState;
use ratatui::layout::Rect;

/// Which view / overlay is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveView {
    #[default]
    Tree,
    SettingsMenu,
    ControlsSubmenu,
    /// Full-screen image lightbox overlay.
    Lightbox,
}

/// Which main pane currently owns keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Tree,
    Inspector,
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
    /// Last terminal area used to render the frame (for mouse hit-testing).
    pub terminal_area: Rect,
    /// True while dragging the tree/inspector splitter with mouse.
    pub dragging_splitter: bool,
    /// Path currently shown in the inspector cache.
    pub inspector_path: Option<PathBuf>,
    /// Cached inspector payload for the selected row.
    pub inspector_info: Option<InspectorInfo>,
    /// Which pane receives keyboard navigation in main tree view.
    pub pane_focus: PaneFocus,
    /// Keys of file-groups that the user has expanded in the tree.
    pub expanded_groups: HashSet<String>,
    /// Pinned inspector cards, created from tree entries.
    pub pinned_inspector: Vec<InspectorInfo>,
    /// Selected pinned card index.
    pub inspector_selected_pin: usize,
    /// Vertical scroll offset into pinned cards (logical target).
    pub inspector_pin_scroll: usize,
    /// Smooth-scroll animator for the pinned cards list.
    /// Smooth-scroll row-offset animator for the pinned cards list.
    pub pin_scroll_anim: crate::ui::smooth_scroll::SmoothScroll,
    /// Pre-resized image thumbnails for the inspector preview.
    /// Images are decoded + resized on background threads and stored here
    /// as small RGBA bitmaps so rendering is essentially free.
    pub image_cache: HashMap<PathBuf, Arc<image::RgbaImage>>,
    /// Paths currently being decoded on background threads.
    pub image_decoding: HashSet<PathBuf>,
    /// Index of the image currently shown in the lightbox (into `pinned_inspector`).
    pub lightbox_index: usize,
    /// Hit zones from the last lightbox render (for mouse click dispatch).
    pub lightbox_hit_zones: Option<crate::ui::lightbox::LightboxHitZones>,
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
            terminal_area: Rect::default(),
            dragging_splitter: false,
            inspector_path: None,
            inspector_info: None,
            pane_focus: PaneFocus::Tree,
            expanded_groups: HashSet::new(),
            pinned_inspector: Vec::new(),
            inspector_selected_pin: 0,
            inspector_pin_scroll: 0,
            pin_scroll_anim: crate::ui::smooth_scroll::SmoothScroll::new(0.35),
            image_cache: HashMap::new(),
            image_decoding: HashSet::new(),
            lightbox_index: 0,
            lightbox_hit_zones: None,
        }
    }
}

