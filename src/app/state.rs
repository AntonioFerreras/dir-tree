//! Central application state.
//!
//! All mutable state lives here so that the rest of the app can be pure
//! functions over `&AppState` (rendering) or `&mut AppState` (event handling).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::AppConfig;
use crate::core::{
    fs::WalkConfig,
    grouping::GroupingConfig,
    inspector::InspectorInfo,
    search::{SearchEntry, SearchResult},
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

/// Active tab inside the right pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightPaneTab {
    #[default]
    Inspector,
    Search,
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
    /// Active tab inside the right pane.
    pub right_pane_tab: RightPaneTab,
    /// Previous tab used before entering search.
    pub right_pane_prev_tab: RightPaneTab,
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
    /// Search root directory.
    pub search_root: PathBuf,
    /// Flat search index for `search_root`.
    pub search_index: Vec<SearchEntry>,
    /// Current search query.
    pub search_query: String,
    /// Search option: case-sensitive matching.
    pub search_case_sensitive: bool,
    /// Ranked matches for the current query.
    pub search_results: Vec<SearchResult>,
    /// Selected row in `search_results`.
    pub search_selected: usize,
    /// Scroll offset for search results.
    pub search_scroll: usize,
    /// Path copied to clipboard before exit (if any).
    pub copied_path: Option<PathBuf>,
    /// Root-change request processed by the background fs runtime.
    pub pending_tree_rebuild: Option<PathBuf>,
    /// Current tree rebuild generation in flight.
    pub tree_rebuild_in_flight: Option<u64>,
    /// Monotonic generation id for tree rebuild requests.
    pub tree_rebuild_generation: u64,
    /// Queue of directory paths to lazily expand in background.
    pub pending_expand_paths: VecDeque<PathBuf>,
    /// Paths currently expanding in background.
    pub expand_in_flight: HashSet<PathBuf>,
    /// Pending reveal target path that should be retried after async scans.
    pub pending_reveal_path: Option<PathBuf>,
    /// Whether search index should be rebuilt for the current root.
    pub search_reindex_requested: bool,
    /// Search index generation currently in flight.
    pub search_reindex_in_flight: Option<u64>,
    /// Monotonic generation id for search reindex requests.
    pub search_reindex_generation: u64,
    /// Non-size background scanning in progress (tree/search/expand jobs).
    pub fs_scanning: bool,
}

impl AppState {
    pub fn new(cwd: PathBuf, tree: DirTree, config: AppConfig) -> Self {
        Self {
            tree,
            tree_state: TreeWidgetState::default(),
            walk_config: WalkConfig::default(),
            grouping_config: GroupingConfig::default(),
            cwd: cwd.clone(),
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
            right_pane_tab: RightPaneTab::Inspector,
            right_pane_prev_tab: RightPaneTab::Inspector,
            expanded_groups: HashSet::new(),
            pinned_inspector: Vec::new(),
            inspector_selected_pin: 0,
            inspector_pin_scroll: 0,
            pin_scroll_anim: crate::ui::smooth_scroll::SmoothScroll::new(0.35),
            image_cache: HashMap::new(),
            image_decoding: HashSet::new(),
            lightbox_index: 0,
            lightbox_hit_zones: None,
            search_root: cwd.clone(),
            search_index: Vec::new(),
            search_query: String::new(),
            search_case_sensitive: false,
            search_results: Vec::new(),
            search_selected: 0,
            search_scroll: 0,
            copied_path: None,
            pending_tree_rebuild: None,
            tree_rebuild_in_flight: None,
            tree_rebuild_generation: 0,
            pending_expand_paths: VecDeque::new(),
            expand_in_flight: HashSet::new(),
            pending_reveal_path: None,
            search_reindex_requested: true,
            search_reindex_in_flight: None,
            search_reindex_generation: 0,
            fs_scanning: false,
        }
    }
}

