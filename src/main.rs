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
    state::AppState,
};
use crate::core::fs;
use crate::shell::integration;
use crate::ui::{layout::AppLayout, theme::Theme, tree_widget::TreeWidget};

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

    // ── event loop ────────────────────────────────────────────
    let mut events = spawn_event_reader(Duration::from_millis(100));

    loop {
        // ── draw ──────────────────────────────────────────────
        terminal.draw(|frame| {
            let layout = AppLayout::from_area(frame.area());

            // Tree pane.
            let tree_block = Block::default()
                .title(format!(" {} ", state.cwd.display()))
                .title_style(Theme::title_style())
                .borders(Borders::ALL)
                .border_style(Theme::border_style());

            let tree_widget =
                TreeWidget::new(&state.tree, &state.grouping_config).block(tree_block);

            frame.render_stateful_widget(tree_widget, layout.tree_area, &mut state.tree_state);

            // Status bar.
            let status_text = state
                .status_message
                .as_deref()
                .unwrap_or("q: quit | ↑↓/jk: navigate | ←→/hl: collapse/expand | Enter: cd | .: toggle hidden");
            let status = Paragraph::new(status_text).style(Theme::status_bar_style());
            frame.render_widget(status, layout.status_area);
        })?;

        // ── handle events ─────────────────────────────────────
        if let Some(event) = events.recv().await {
            match event {
                AppEvent::Key(k) => handler::handle_key(&mut state, k),
                AppEvent::Mouse(m) => handler::handle_mouse(&mut state, m),
                AppEvent::Resize(_, _) => { /* terminal auto-redraws */ }
                AppEvent::Tick => { /* just redraw */ }
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
