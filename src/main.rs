//! A tree-based TUI to replace `cd` & `ls`.
//!
//! Run the binary to launch the interactive tree view.
//! Run with `--init-bash` to print the shell function for your `.bashrc`.

mod app;
mod config;
mod core;
mod shell;
mod ui;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, stderr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    widgets::{Block, Borders, Paragraph},
    Terminal,
};

use crate::app::{
    event::{spawn_event_reader, AppEvent},
    handler,
    state::{ActiveView, AppState},
};
use crate::shell::integration;
use crate::ui::{layout::AppLayout, popup, theme::Theme, tree_widget::TreeWidget};

// ───────────────────────────────────────── CLI ───────────────

#[derive(Parser, Debug)]
#[command(name = env!("CARGO_PKG_NAME"), about = "Tree-based directory navigator")]
struct Cli {
    /// Directory to open (defaults to `.`).
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Print the bash shell function and exit.
    #[arg(long = "init-bash")]
    init_bash: bool,

    /// Print the zsh shell function and exit.
    #[arg(long = "init-zsh")]
    init_zsh: bool,

    /// Maximum tree depth.
    #[arg(long, default_value_t = 3)]
    depth: usize,

    /// Show hidden (dot) files.
    #[arg(long)]
    hidden: bool,
}

// ───────────────────────────────────────── size computation ──

/// Map of hard-linked inodes: (dev, ino) → apparent size.
/// Only files with nlink > 1 land here; nlink == 1 files are summed directly.
type InodeMap = HashMap<(u64, u64), u64>;

/// Cached result from a directory's local walk.
#[derive(Clone)]
struct DirLocalResult {
    /// Sum of apparent sizes for files with nlink == 1 (safely additive).
    unique_sum: u64,
    /// Hard-linked files: (dev, ino) → size.  Deduped within this subtree,
    /// but may overlap with sibling directories — the cascade merges these.
    hardlinks: InodeMap,
}

#[derive(Debug)]
enum SizeUpdate {
    File { path: PathBuf, size: u64 },
    DirLocalDone {
        dir: PathBuf,
        unique_sum: u64,
        hardlinks: InodeMap,
    },
    WorkerDone,
}

/// Shared read-only context available to every worker thread.
struct WorkerCtx {
    /// Paths of directories that are nodes in the display tree.
    /// Workers skip these during their walk because the cascade
    /// handles them separately.
    tree_dirs: HashSet<PathBuf>,
    /// Whether hard-link dedup is enabled.
    dedup_hard_links: bool,
}

struct SizeComputeState {
    generation: u64,
    remaining_workers: usize,
    /// Tree directory nodes sorted deepest-first for O(n) cascade.
    dirs: Vec<PathBuf>,
    parent_dir: HashMap<PathBuf, Option<PathBuf>>,
    pending_children: HashMap<PathBuf, usize>,
    /// Per-dir: accumulated unique_sum from tree-children.
    children_unique: HashMap<PathBuf, u64>,
    /// Per-dir: merged hardlink maps from tree-children.
    children_hardlinks: HashMap<PathBuf, InodeMap>,
    /// Per-dir: the local walk result (unique_sum + hardlinks).
    local_done: HashMap<PathBuf, DirLocalResult>,
    finished: HashSet<PathBuf>,
    /// Shared flag used to signal worker threads to stop early.
    cancel: Arc<AtomicBool>,
}

/// Classify a file as unique or hard-linked, returning `(size, is_hardlink, dev, ino)`.
/// Files with nlink == 1 are unique and never need inode tracking.
#[cfg(unix)]
fn classify_file(meta: &std::fs::Metadata, dedup: bool) -> (u64, Option<(u64, u64)>) {
    let size = meta.len();
    if !dedup {
        return (size, None);
    }
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() <= 1 {
        (size, None) // unique — no inode tracking needed
    } else {
        (size, Some((meta.dev(), meta.ino())))
    }
}

#[cfg(not(unix))]
fn classify_file(meta: &std::fs::Metadata, _dedup: bool) -> (u64, Option<(u64, u64)>) {
    (meta.len(), None)
}

fn start_size_computation(
    state: &mut AppState,
    tx: &tokio::sync::mpsc::UnboundedSender<(u64, SizeUpdate)>,
) -> SizeComputeState {
    state.size_compute_generation = state.size_compute_generation.wrapping_add(1);
    let generation = state.size_compute_generation;

    // Don't clear dir_sizes — stale values are shown briefly until the
    // cascade overwrites them.  This avoids a visible flicker where sizes
    // disappear for one frame before being repopulated.

    let cancel = Arc::new(AtomicBool::new(false));

    // Build a set of all directory paths that are nodes in the display tree.
    let mut tree_dirs = HashSet::new();
    for node in &state.tree.nodes {
        if node.meta.is_dir {
            tree_dirs.insert(node.meta.path.clone());
        }
    }

    let mut dirs = Vec::new();
    let mut dir_depth: HashMap<PathBuf, usize> = HashMap::new();
    let mut parent_dir: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    let mut pending_children: HashMap<PathBuf, usize> = HashMap::new();
    let mut children_unique: HashMap<PathBuf, u64> = HashMap::new();
    let mut children_hardlinks: HashMap<PathBuf, InodeMap> = HashMap::new();
    let mut local_done: HashMap<PathBuf, DirLocalResult> = HashMap::new();
    let mut jobs: VecDeque<PathBuf> = VecDeque::new();

    for node in &state.tree.nodes {
        if !node.meta.is_dir {
            continue;
        }

        let dir_path = node.meta.path.clone();
        dirs.push(dir_path.clone());
        dir_depth.insert(dir_path.clone(), node.depth);

        let parent_path = node.parent.and_then(|pid| {
            let p = &state.tree.nodes[pid];
            if p.meta.is_dir {
                Some(p.meta.path.clone())
            } else {
                None
            }
        });
        parent_dir.insert(dir_path.clone(), parent_path);

        let child_dir_count = node
            .children
            .iter()
            .filter(|&&cid| state.tree.nodes[cid].meta.is_dir)
            .count();

        pending_children.insert(dir_path.clone(), child_dir_count);
        children_unique.insert(dir_path.clone(), 0);
        children_hardlinks.insert(dir_path.clone(), InodeMap::new());

        // Reuse cached local result if available.
        if let Some(cached) = state.dir_local_sums.get(&dir_path) {
            local_done.insert(dir_path, cached.clone());
        } else {
            jobs.push_back(dir_path);
        }
    }

    // Sort dirs deepest-first for O(n) cascade finalization.
    dirs.sort_by(|a, b| {
        let da = dir_depth.get(a).copied().unwrap_or(0);
        let db = dir_depth.get(b).copied().unwrap_or(0);
        db.cmp(&da)
    });

    let queue = Arc::new(Mutex::new(jobs));
    let dedup_hard_links = state.walk_config.dedup_hard_links;
    let ctx = Arc::new(WorkerCtx {
        tree_dirs,
        dedup_hard_links,
    });

    let max_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);

    let job_count = queue.lock().ok().map_or(0, |q| q.len());
    let worker_count = max_threads.min(job_count.max(1));

    if job_count > 0 {
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let cancel = Arc::clone(&cancel);
            let ctx = Arc::clone(&ctx);
            std::thread::spawn(move || {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }

                    let dir = {
                        let mut q = match queue.lock() {
                            Ok(guard) => guard,
                            Err(_) => break,
                        };
                        match q.pop_front() {
                            Some(d) => d,
                            None => break,
                        }
                    };

                    let entries = match std::fs::read_dir(&dir) {
                        Ok(e) => e,
                        Err(_) => {
                            let _ = tx.send((
                                generation,
                                SizeUpdate::DirLocalDone {
                                    dir,
                                    unique_sum: 0,
                                    hardlinks: InodeMap::new(),
                                },
                            ));
                            continue;
                        }
                    };

                    let mut unique_sum: u64 = 0;
                    let mut hardlinks = InodeMap::new();

                    for entry in entries.flatten() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let ft = match entry.file_type() {
                            Ok(ft) => ft,
                            Err(_) => continue,
                        };
                        let path = entry.path();

                        if ft.is_file() {
                            if let Ok(meta) = entry.metadata() {
                                let s = meta.len();
                                let _ = tx.send((
                                    generation,
                                    SizeUpdate::File {
                                        path: path.clone(),
                                        size: s,
                                    },
                                ));
                                let (size, inode_key) = classify_file(&meta, ctx.dedup_hard_links);
                                match inode_key {
                                    None => unique_sum = unique_sum.saturating_add(size),
                                    Some(key) => { hardlinks.entry(key).or_insert(size); }
                                }
                            }
                        } else if ft.is_dir() {
                            if ctx.tree_dirs.contains(&path) {
                                // Tree child dir — cascade handles it.
                            } else {
                                // Non-tree child dir — recursively walk it.
                                let (sub_unique, sub_hardlinks) =
                                    recursive_dir_size(&path, &cancel, ctx.dedup_hard_links);
                                unique_sum = unique_sum.saturating_add(sub_unique);
                                for (k, v) in sub_hardlinks {
                                    hardlinks.entry(k).or_insert(v);
                                }
                            }
                        } else if ft.is_symlink() {
                            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                                let s = meta.len();
                                let _ = tx.send((
                                    generation,
                                    SizeUpdate::File {
                                        path: path.clone(),
                                        size: s,
                                    },
                                ));
                                unique_sum = unique_sum.saturating_add(s);
                            }
                        }
                    }

                    let _ = tx.send((
                        generation,
                        SizeUpdate::DirLocalDone { dir, unique_sum, hardlinks },
                    ));
                }

                let _ = tx.send((generation, SizeUpdate::WorkerDone));
            });
        }
    }

    SizeComputeState {
        generation,
        remaining_workers: if job_count > 0 { worker_count } else { 0 },
        dirs,
        parent_dir,
        pending_children,
        children_unique,
        children_hardlinks,
        local_done,
        finished: HashSet::new(),
        cancel,
    }
}

/// Recursively compute the total size of all files under `dir`.
/// Returns (unique_sum, hardlinks) — split by nlink for cascade dedup.
fn recursive_dir_size(dir: &Path, cancel: &AtomicBool, dedup: bool) -> (u64, InodeMap) {
    let mut unique_sum: u64 = 0;
    let mut hardlinks = InodeMap::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let entries = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    let (size, inode_key) = classify_file(&meta, dedup);
                    match inode_key {
                        None => unique_sum = unique_sum.saturating_add(size),
                        Some(key) => { hardlinks.entry(key).or_insert(size); }
                    }
                }
            } else if ft.is_symlink() {
                if let Ok(meta) = std::fs::symlink_metadata(&entry.path()) {
                    unique_sum = unique_sum.saturating_add(meta.len());
                }
            }
        }
    }

    (unique_sum, hardlinks)
}

/// Process a single size update message.  Returns `true` if a `DirLocalDone`
/// was applied (meaning `finalize_ready_dirs` should be called afterward).
fn apply_size_update(
    state: &mut AppState,
    size_compute: &mut Option<SizeComputeState>,
    generation: u64,
    update: SizeUpdate,
) -> bool {
    if generation != state.size_compute_generation {
        return false;
    }
    let Some(ref mut compute) = size_compute else {
        return false;
    };
    if compute.generation != generation {
        return false;
    }
    match update {
        SizeUpdate::File { path, size } => {
            state.file_sizes.insert(path, size);
            false
        }
        SizeUpdate::DirLocalDone { dir, unique_sum, hardlinks } => {
            let result = DirLocalResult { unique_sum, hardlinks };
            // Cache for future recomputes.
            state.dir_local_sums.insert(dir.clone(), result.clone());
            compute.local_done.insert(dir, result);
            true
        }
        SizeUpdate::WorkerDone => {
            compute.remaining_workers = compute.remaining_workers.saturating_sub(1);
            false
        }
    }
}

/// O(n) cascade: process dirs deepest-first, merging hardlink maps bottom-up.
///
/// Each directory's total = unique_bytes + sum(hardlink_map.values()), where
/// hardlink_map is the union of the dir's own hardlinks and all children's
/// hardlink maps.  This means a hard-linked file counts independently in
/// each leaf directory, but is deduped in any common ancestor.
fn finalize_ready_dirs(state: &mut AppState, compute: &mut SizeComputeState) {
    for i in 0..compute.dirs.len() {
        let dir = compute.dirs[i].clone();
        if compute.finished.contains(&dir) {
            continue;
        }

        // Check readiness without removing yet.
        if !compute.local_done.contains_key(&dir) {
            continue;
        }
        let pending = *compute.pending_children.get(&dir).unwrap_or(&0);
        if pending != 0 {
            continue;
        }

        // Take ownership — no cloning.
        let local = compute.local_done.remove(&dir).unwrap();
        let children_unique = compute.children_unique.remove(&dir).unwrap_or(0);
        let children_hl = compute.children_hardlinks.remove(&dir).unwrap_or_default();

        let total_unique = local.unique_sum.saturating_add(children_unique);

        // Merge hardlink maps: pick the larger map as the base to minimise
        // insertions, then extend from the smaller one.
        let mut merged_hardlinks;
        if local.hardlinks.len() >= children_hl.len() {
            merged_hardlinks = local.hardlinks;
            for (k, v) in children_hl {
                merged_hardlinks.entry(k).or_insert(v);
            }
        } else {
            merged_hardlinks = children_hl;
            for (k, v) in local.hardlinks {
                merged_hardlinks.entry(k).or_insert(v);
            }
        }

        let hardlink_bytes: u64 = merged_hardlinks.values().sum();
        let total = total_unique.saturating_add(hardlink_bytes);

        state.dir_sizes.insert(dir.clone(), total);
        compute.finished.insert(dir.clone());

        // Propagate to parent — move the merged map, don't copy.
        if let Some(Some(parent)) = compute.parent_dir.get(&dir) {
            if let Some(remaining) = compute.pending_children.get_mut(parent) {
                *remaining = remaining.saturating_sub(1);
            }
            if let Some(sum) = compute.children_unique.get_mut(parent) {
                *sum = sum.saturating_add(total_unique);
            }
            // Merge into parent's children_hardlinks.  If the parent has
            // no accumulated map yet, just move ours in wholesale.
            let parent_hl = compute
                .children_hardlinks
                .entry(parent.clone())
                .or_default();
            if parent_hl.is_empty() {
                *parent_hl = merged_hardlinks;
            } else {
                for (k, v) in merged_hardlinks {
                    parent_hl.entry(k).or_insert(v);
                }
            }
        }
    }
}

// ───────────────────────────────────────── main ─────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Initialise tracing (only in debug builds / when RUST_LOG is set).
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr) // never pollute stdout
        .init();

    let cli = Cli::parse();

    // ── shell-integration mode ────────────────────────────────
    if cli.init_bash {
        print!("{}", integration::bash_function());
        return Ok(());
    }
    if cli.init_zsh {
        print!("{}", integration::zsh_function());
        return Ok(());
    }

    // ── build initial tree ────────────────────────────────────
    let root = cli.path.canonicalize()?;
    let mut walk_config = core::fs::WalkConfig::default();
    walk_config.max_depth = cli.depth;
    walk_config.show_hidden = cli.hidden;

    let tree = core::fs::build_tree(&root, &walk_config)?;
    let user_config = config::AppConfig::load();
    let mut state = AppState::new(root, tree, user_config);
    state.walk_config = walk_config;
    state.needs_size_recompute = true;

    // ── terminal setup ────────────────────────────────────────
    enable_raw_mode()?;
    let mut stderr_handle = stderr();
    execute!(
        stderr_handle,
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stderr());
    let mut terminal = Terminal::new(backend)?;

    // ── async channels ────────────────────────────────────────
    let mut events = spawn_event_reader(Duration::from_millis(100));
    let (size_tx, mut size_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, SizeUpdate)>();
    let mut size_compute: Option<SizeComputeState> = None;

    // ── event loop ────────────────────────────────────────────
    loop {
        // ── draw first ─────────────────────────────────────────
        // Always render before doing any expensive work so the UI
        // stays responsive.  Sizes fill in asynchronously.
        terminal.draw(|frame| {
            let layout = AppLayout::from_area(frame.area());

            let tree_block = Block::default()
                .title(format!(" {} ", state.cwd.display()))
                .title_style(Theme::title_style())
                .borders(Borders::ALL)
                .border_style(Theme::border_style());

            let tree_widget = TreeWidget::new(&state.tree, &state.grouping_config)
                .dir_sizes(&state.dir_sizes)
                .file_sizes(&state.file_sizes)
                .block(tree_block);

            frame.render_stateful_widget(tree_widget, layout.tree_area, &mut state.tree_state);

            let hint = state.config.status_bar_hint();
            let status_text = match state.active_view {
                ActiveView::Tree => state.status_message.as_deref().unwrap_or(&hint),
                ActiveView::SettingsMenu | ActiveView::ControlsSubmenu => "",
            };
            let status = Paragraph::new(status_text).style(Theme::status_bar_style());
            frame.render_widget(status, layout.status_area);

            match state.active_view {
                ActiveView::SettingsMenu => {
                    frame.render_widget(
                        popup::SettingsPopup {
                            selected: state.settings_selected,
                        },
                        frame.area(),
                    );
                }
                ActiveView::ControlsSubmenu => {
                    frame.render_widget(
                        popup::ControlsPopup {
                            config: &state.config,
                            selected: state.controls_selected,
                            awaiting_rebind: state.awaiting_rebind,
                        },
                        frame.area(),
                    );
                }
                ActiveView::Tree => {}
            }
        })?;

        // ── kick off size recompute AFTER draw ───────────────────
        // The draw above already rendered the updated tree structure
        // (expanded dirs, new entries).  Now we compute sizes — cached
        // dirs finalize immediately, uncached ones arrive via workers.
        // Sizes appear on the next frame; the expand itself is instant.
        if state.needs_size_recompute {
            state.needs_size_recompute = false;
            if let Some(ref old) = size_compute {
                old.cancel.store(true, Ordering::Relaxed);
            }
            size_compute = Some(start_size_computation(&mut state, &size_tx));
            if let Some(ref mut compute) = size_compute {
                finalize_ready_dirs(&mut state, compute);
            }
        }

        tokio::select! {
            biased;

            Some(event) = events.recv() => {
                match event {
                    AppEvent::Key(k) => handler::handle_key(&mut state, k),
                    AppEvent::Mouse(m) => handler::handle_mouse(&mut state, m),
                    AppEvent::Resize(_, _) => {}
                    AppEvent::Tick => {}
                }
            }

            Some((generation, update)) = size_rx.recv() => {
                // Process the first message, then batch-drain all remaining
                // available messages before redrawing.  This prevents stale
                // messages from old (cancelled) workers from causing
                // per-message redraws that stall visible progress.
                let mut need_finalize = false;
                need_finalize |= apply_size_update(
                    &mut state, &mut size_compute, generation, update,
                );

                // Drain everything currently queued without blocking.
                while let Ok((gen, upd)) = size_rx.try_recv() {
                    need_finalize |= apply_size_update(
                        &mut state, &mut size_compute, gen, upd,
                    );
                }

                if need_finalize {
                    if let Some(ref mut compute) = size_compute {
                        finalize_ready_dirs(&mut state, compute);
                    }
                }
            }
        }

        if state.should_quit {
            break;
        }
    }

    // ── teardown ──────────────────────────────────────────────
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Some(ref dir) = state.selected_dir {
        integration::print_selected_dir(dir);
    }

    Ok(())
}
