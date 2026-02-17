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

#[derive(Debug)]
enum SizeUpdate {
    File { path: PathBuf, size: u64 },
    DirLocalDone { dir: PathBuf, local_sum: u64 },
    WorkerDone,
}

/// Shared read-only context available to every worker thread.
struct WorkerCtx {
    /// Paths of directories that are nodes in the display tree.
    /// Workers skip these during their walk because the cascade
    /// handles them separately.
    tree_dirs: HashSet<PathBuf>,
    /// Snapshot of already-known file sizes (avoids redundant `stat` calls
    /// on recompute after expand).
    known_file_sizes: HashMap<PathBuf, u64>,
}

struct SizeComputeState {
    generation: u64,
    remaining_workers: usize,
    dirs: Vec<PathBuf>,
    parent_dir: HashMap<PathBuf, Option<PathBuf>>,
    pending_children: HashMap<PathBuf, usize>,
    children_sum: HashMap<PathBuf, u64>,
    local_done: HashMap<PathBuf, u64>,
    finished: HashSet<PathBuf>,
    /// Shared flag used to signal worker threads to stop early.
    cancel: Arc<AtomicBool>,
}

fn start_size_computation(
    state: &mut AppState,
    tx: &tokio::sync::mpsc::UnboundedSender<(u64, SizeUpdate)>,
) -> SizeComputeState {
    state.size_compute_generation = state.size_compute_generation.wrapping_add(1);
    let generation = state.size_compute_generation;

    // Clear dir sizes (tree structure may have changed, so totals need
    // recomputation), but KEEP file sizes — individual file sizes remain
    // valid across expand/collapse and avoid expensive re-stat calls.
    state.dir_sizes.clear();

    let cancel = Arc::new(AtomicBool::new(false));

    // Build a set of all directory paths that are nodes in the display tree.
    // Workers use this to know which child dirs the cascade already handles
    // vs which ones need an inline recursive walk.
    let mut tree_dirs = HashSet::new();
    for node in &state.tree.nodes {
        if node.meta.is_dir {
            tree_dirs.insert(node.meta.path.clone());
        }
    }

    let mut dirs = Vec::new();
    let mut parent_dir: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    let mut pending_children: HashMap<PathBuf, usize> = HashMap::new();
    let mut children_sum: HashMap<PathBuf, u64> = HashMap::new();
    let mut local_done: HashMap<PathBuf, u64> = HashMap::new();
    let mut jobs: VecDeque<PathBuf> = VecDeque::new();

    for node in &state.tree.nodes {
        if !node.meta.is_dir {
            continue;
        }

        let dir_path = node.meta.path.clone();
        dirs.push(dir_path.clone());

        let parent_path = node.parent.and_then(|pid| {
            let p = &state.tree.nodes[pid];
            if p.meta.is_dir {
                Some(p.meta.path.clone())
            } else {
                None
            }
        });
        parent_dir.insert(dir_path.clone(), parent_path);

        // Count only tree-loaded child directories for the cascade;
        // non-tree child dirs are walked inline by the worker and
        // included in local_sum.
        let child_dir_count = node
            .children
            .iter()
            .filter(|&&cid| state.tree.nodes[cid].meta.is_dir)
            .count();

        pending_children.insert(dir_path.clone(), child_dir_count);
        children_sum.insert(dir_path.clone(), 0);

        // If we already have a cached local_sum for this directory, reuse it
        // instead of spawning a worker job.  The cache is invalidated only for
        // directories whose tree-children changed (i.e. the expanded dir).
        if let Some(&cached) = state.dir_local_sums.get(&dir_path) {
            local_done.insert(dir_path, cached);
        } else {
            jobs.push_back(dir_path);
        }
    }

    let queue = Arc::new(Mutex::new(jobs));
    let ctx = Arc::new(WorkerCtx {
        tree_dirs,
        known_file_sizes: state.file_sizes.clone(),
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

                    // Walk the REAL filesystem at depth 1 for this directory.
                    let entries = match std::fs::read_dir(&dir) {
                        Ok(e) => e,
                        Err(_) => {
                            let _ = tx.send((
                                generation,
                                SizeUpdate::DirLocalDone {
                                    dir,
                                    local_sum: 0,
                                },
                            ));
                            continue;
                        }
                    };

                    let mut local_sum: u64 = 0;

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
                            // Use cached size if available, otherwise stat.
                            let size = if let Some(&s) = ctx.known_file_sizes.get(&path) {
                                s
                            } else if let Ok(meta) = entry.metadata() {
                                let s = meta.len();
                                let _ = tx.send((
                                    generation,
                                    SizeUpdate::File {
                                        path: path.clone(),
                                        size: s,
                                    },
                                ));
                                s
                            } else {
                                continue;
                            };
                            local_sum = local_sum.saturating_add(size);
                        } else if ft.is_dir() {
                            if ctx.tree_dirs.contains(&path) {
                                // Tree child dir — handled by the cascade,
                                // don't count it here.
                            } else {
                                // Non-tree child dir (gitignored, hidden, or
                                // beyond display depth) — recursively walk it.
                                local_sum = local_sum
                                    .saturating_add(recursive_dir_size(&path, &cancel));
                            }
                        }
                        // Symlinks and other special files are intentionally
                        // skipped to avoid double-counting.
                    }

                    let _ = tx.send((
                        generation,
                        SizeUpdate::DirLocalDone { dir, local_sum },
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
        children_sum,
        local_done,
        finished: HashSet::new(),
        cancel,
    }
}

/// Recursively compute the total size of all files under `dir`.
fn recursive_dir_size(dir: &Path, cancel: &AtomicBool) -> u64 {
    let mut total: u64 = 0;
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
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }

    total
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
        SizeUpdate::DirLocalDone { dir, local_sum } => {
            compute.local_done.insert(dir.clone(), local_sum);
            // Cache for future recomputes so this dir won't need a worker.
            state.dir_local_sums.insert(dir, local_sum);
            true
        }
        SizeUpdate::WorkerDone => {
            compute.remaining_workers = compute.remaining_workers.saturating_sub(1);
            false
        }
    }
}

fn finalize_ready_dirs(state: &mut AppState, compute: &mut SizeComputeState) {
    loop {
        let mut progressed = false;

        for dir in &compute.dirs {
            if compute.finished.contains(dir) {
                continue;
            }

            let local = match compute.local_done.get(dir) {
                Some(v) => *v,
                None => continue,
            };
            let pending = *compute.pending_children.get(dir).unwrap_or(&0);
            if pending != 0 {
                continue;
            }

            let total = local.saturating_add(*compute.children_sum.get(dir).unwrap_or(&0));
            state.dir_sizes.insert(dir.clone(), total);
            compute.finished.insert(dir.clone());
            progressed = true;

            if let Some(Some(parent)) = compute.parent_dir.get(dir) {
                if let Some(remaining) = compute.pending_children.get_mut(parent) {
                    *remaining = remaining.saturating_sub(1);
                }
                if let Some(sum) = compute.children_sum.get_mut(parent) {
                    *sum = sum.saturating_add(total);
                }
            }
        }

        if !progressed {
            break;
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
        if state.needs_size_recompute {
            state.needs_size_recompute = false;
            // Signal old workers to stop before starting new ones.
            if let Some(ref old) = size_compute {
                old.cancel.store(true, Ordering::Relaxed);
            }
            size_compute = Some(start_size_computation(&mut state, &size_tx));
            if let Some(ref mut compute) = size_compute {
                finalize_ready_dirs(&mut state, compute);
            }
        }

        // ── draw ──────────────────────────────────────────────
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
