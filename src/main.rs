//! A tree-based TUI to replace `cd` & `ls`.
//!
//! Run the binary to launch the interactive tree view.
//! Run with `--init-bash` to print the shell function for your `.bashrc`.

mod app;
mod core;
mod shell;
mod ui;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, stderr};
use std::path::PathBuf;
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

const STAT_CHUNK_SIZE: usize = 256;

#[derive(Debug)]
enum SizeUpdate {
    File { path: PathBuf, size: u64 },
    DirLocalDone { dir: PathBuf, local_sum: u64 },
    WorkerDone,
}

#[derive(Clone)]
struct DirStatJob {
    dir: PathBuf,
    files: Arc<Vec<PathBuf>>,
    cursor: usize,
    local_sum: u64,
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
}

fn start_size_computation(
    state: &mut AppState,
    tx: &tokio::sync::mpsc::UnboundedSender<(u64, SizeUpdate)>,
) -> SizeComputeState {
    state.size_compute_generation = state.size_compute_generation.wrapping_add(1);
    let generation = state.size_compute_generation;

    // Recompute from scratch for the currently-loaded tree snapshot.
    state.dir_sizes.clear();
    state.file_sizes.clear();

    let mut dirs = Vec::new();
    let mut parent_dir: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    let mut pending_children: HashMap<PathBuf, usize> = HashMap::new();
    let mut children_sum: HashMap<PathBuf, u64> = HashMap::new();
    let mut local_done: HashMap<PathBuf, u64> = HashMap::new();
    let mut jobs = VecDeque::new();

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

        let mut child_dir_count = 0usize;
        let mut files = Vec::new();
        for &child_id in &node.children {
            let child = &state.tree.nodes[child_id];
            if child.meta.is_dir {
                child_dir_count += 1;
            } else {
                files.push(child.meta.path.clone());
            }
        }

        pending_children.insert(dir_path.clone(), child_dir_count);
        children_sum.insert(dir_path.clone(), 0);

        if files.is_empty() {
            local_done.insert(dir_path.clone(), 0);
        } else {
            jobs.push_back(DirStatJob {
                dir: dir_path,
                files: Arc::new(files),
                cursor: 0,
                local_sum: 0,
            });
        }
    }

    let queue = Arc::new(Mutex::new(jobs));
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
            std::thread::spawn(move || {
                loop {
                    let mut job = {
                        let mut q = match queue.lock() {
                            Ok(guard) => guard,
                            Err(_) => break,
                        };
                        match q.pop_front() {
                            Some(job) => job,
                            None => break,
                        }
                    };

                    let mut processed = 0usize;
                    while processed < STAT_CHUNK_SIZE && job.cursor < job.files.len() {
                        let path = job.files[job.cursor].clone();
                        job.cursor += 1;
                        processed += 1;

                        if let Ok(meta) = std::fs::metadata(&path) {
                            if meta.is_file() {
                                let size = meta.len();
                                job.local_sum = job.local_sum.saturating_add(size);
                                let _ = tx.send((generation, SizeUpdate::File { path, size }));
                            }
                        }
                    }

                    if job.cursor < job.files.len() {
                        if let Ok(mut q) = queue.lock() {
                            q.push_back(job);
                        } else {
                            break;
                        }
                    } else {
                        let _ = tx.send((
                            generation,
                            SizeUpdate::DirLocalDone {
                                dir: job.dir,
                                local_sum: job.local_sum,
                            },
                        ));
                    }
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
    let mut state = AppState::new(root, tree);
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

            let status_text = match state.active_view {
                ActiveView::Tree => state.status_message.as_deref().unwrap_or(
                    "↑↓: navigate | ←→: collapse/expand | Alt+↑↓: jump dirs | Enter: cd | ?: settings",
                ),
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
                    frame.render_widget(popup::ControlsPopup, frame.area());
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
                // Drop stale updates from older computations.
                if generation == state.size_compute_generation {
                    if let Some(ref mut compute) = size_compute {
                        if compute.generation == generation {
                            match update {
                                SizeUpdate::File { path, size } => {
                                    state.file_sizes.insert(path, size);
                                }
                                SizeUpdate::DirLocalDone { dir, local_sum } => {
                                    compute.local_done.insert(dir, local_sum);
                                    finalize_ready_dirs(&mut state, compute);
                                }
                                SizeUpdate::WorkerDone => {
                                    compute.remaining_workers = compute.remaining_workers.saturating_sub(1);
                                }
                            }
                        }
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
