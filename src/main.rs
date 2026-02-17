//! A tree-based TUI to replace `cd` & `ls`.
//!
//! Run the binary to launch the interactive tree view.
//! Run with `--init-bash` to print the shell function for your `.bashrc`.

mod app;
mod core;
mod shell;
mod ui;

use std::io::{self, stderr};
use std::path::PathBuf;
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
use crate::core::fs;
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
    let mut walk_config = fs::WalkConfig::default();
    walk_config.max_depth = cli.depth;
    walk_config.show_hidden = cli.hidden;

    let tree = fs::build_tree(&root, &walk_config)?;
    let mut state = AppState::new(root, tree);
    state.walk_config = walk_config;

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

    // ── size computation channel ──────────────────────────────
    let (size_tx, mut size_rx) =
        tokio::sync::mpsc::unbounded_channel::<(PathBuf, u64)>();

    // Kick off the initial background size computation.
    spawn_size_computation(&state, &size_tx);

    // ── event loop ────────────────────────────────────────────
    let mut events = spawn_event_reader(Duration::from_millis(100));

    loop {
        // If the tree changed (expand / rebuild), compute sizes for new dirs.
        if state.needs_size_recompute {
            state.needs_size_recompute = false;
            spawn_size_computation(&state, &size_tx);
        }

        // ── draw ──────────────────────────────────────────────
        terminal.draw(|frame| {
            let layout = AppLayout::from_area(frame.area());

            // Tree pane.
            let tree_block = Block::default()
                .title(format!(" {} ", state.cwd.display()))
                .title_style(Theme::title_style())
                .borders(Borders::ALL)
                .border_style(Theme::border_style());

            let tree_widget = TreeWidget::new(&state.tree, &state.grouping_config)
                .dir_sizes(&state.dir_sizes)
                .block(tree_block);

            frame.render_stateful_widget(tree_widget, layout.tree_area, &mut state.tree_state);

            // Status bar.
            let status_text = match state.active_view {
                ActiveView::Tree => state
                    .status_message
                    .as_deref()
                    .unwrap_or("↑↓: navigate | ←→: collapse/expand | Alt+↑↓: jump dirs | Enter: cd | ?: settings"),
                ActiveView::SettingsMenu | ActiveView::ControlsSubmenu => "",
            };
            let status = Paragraph::new(status_text).style(Theme::status_bar_style());
            frame.render_widget(status, layout.status_area);

            // Settings / controls overlay.
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

        // ── handle events ─────────────────────────────────────
        // Use biased select so user input is always prioritised over
        // background size updates.
        tokio::select! {
            biased;

            Some(event) = events.recv() => {
                match event {
                    AppEvent::Key(k) => handler::handle_key(&mut state, k),
                    AppEvent::Mouse(m) => handler::handle_mouse(&mut state, m),
                    AppEvent::Resize(_, _) => { /* terminal auto-redraws */ }
                    AppEvent::Tick => { /* just redraw */ }
                }
            }

            Some((path, size)) = size_rx.recv() => {
                state.dir_sizes.insert(path, size);
                // Drain any additional ready updates to batch redraws.
                while let Ok((p, s)) = size_rx.try_recv() {
                    state.dir_sizes.insert(p, s);
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

    // If the user selected a directory, print it to stdout for the shell wrapper.
    if let Some(ref dir) = state.selected_dir {
        integration::print_selected_dir(dir);
    }

    Ok(())
}

/// Spawn a background OS thread that computes total sizes for every directory
/// in the tree that isn't already cached.  Results are streamed back through
/// `size_tx` and picked up by the main event loop.
fn spawn_size_computation(
    state: &AppState,
    size_tx: &tokio::sync::mpsc::UnboundedSender<(PathBuf, u64)>,
) {
    let needed: Vec<PathBuf> = state
        .tree
        .nodes
        .iter()
        .filter(|n| n.meta.is_dir && !state.dir_sizes.contains_key(&n.meta.path))
        .map(|n| n.meta.path.clone())
        .collect();

    if needed.is_empty() {
        return;
    }

    let tx = size_tx.clone();
    std::thread::spawn(move || {
        for dir in needed {
            let size = fs::dir_size(&dir);
            // If the receiver is dropped (app quit), stop early.
            if tx.send((dir, size)).is_err() {
                break;
            }
        }
    });
}
