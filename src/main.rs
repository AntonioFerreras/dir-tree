//! A tree-based TUI to replace `cd` & `ls`.
//!
//! Run the binary to launch the interactive tree view.
//! Run with `--init-bash` to print the shell function for your `.bashrc`.

mod app;
mod config;
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
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
    Terminal,
};

use crate::app::{
    event::{spawn_event_reader, AppEvent},
    handler,
    state::{ActiveView, AppState, PaneFocus},
};
use crate::shell::integration;
use crate::ui::{
    inspector::InspectorWidget, layout::AppLayout, popup, spinner::ScanIndicator, theme::Theme,
    tree_widget::TreeWidget,
};

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

    /// Stay on the same filesystem (don't cross mount points).
    #[arg(long = "one-file-system", short = 'x')]
    one_file_system: bool,
}

// ───────────────────────────────────────── size computation ──

use crate::app::size_runtime::{
    apply_size_update, finalize_ready_dirs, start_size_computation, SizeComputeState, SizeUpdate,
};

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

    let mut user_config = config::AppConfig::load();

    // Apply persisted settings; CLI flags override.
    user_config.one_file_system = if cli.one_file_system {
        true // CLI -x forces it on
    } else {
        user_config.one_file_system
    };

    let tree = core::fs::build_tree(&root, &walk_config, user_config.one_file_system)?;
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
    let mut events = spawn_event_reader(Duration::from_millis(50));
    let (size_tx, mut size_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, SizeUpdate)>();
    let mut size_compute: Option<SizeComputeState> = None;
    let mut tick_count: u64 = 0;

    // ── event loop ────────────────────────────────────────────
    loop {
        refresh_inspector(&mut state);

        // ── draw first ─────────────────────────────────────────
        // Always render before doing any expensive work so the UI
        // stays responsive.  Sizes fill in asynchronously.
        terminal.draw(|frame| {
            state.terminal_area = frame.area();
            let layout =
                AppLayout::from_area(frame.area(), state.config.panel_layout, state.config.panel_split_pct);

            let tree_focused = state.pane_focus == PaneFocus::Tree;
            let inspector_focused = state.pane_focus == PaneFocus::Inspector;

            let tree_block = Block::default()
                .title(format!(
                    " Tree{} · Tab: switch pane ",
                    if tree_focused { " [focused]" } else { "" }
                ))
                .title_style(if tree_focused {
                    Theme::title_style()
                } else {
                    Theme::size_style()
                })
                .title_bottom(format!(" {} ", state.cwd.display()))
                .borders(Borders::ALL)
                .border_style(if tree_focused {
                    ratatui::style::Style::default().fg(ratatui::style::Color::LightBlue)
                } else {
                    Theme::border_style()
                });

            let tree_widget = TreeWidget::new(&state.tree, &state.grouping_config)
                .dir_sizes(&state.dir_sizes)
                .file_sizes(&state.file_sizes)
                .block(tree_block);

            frame.render_stateful_widget(tree_widget, layout.tree_area, &mut state.tree_state);

            let inspector_block = Block::default()
                .title(format!(
                    " Inspector{} · Tab: switch pane ",
                    if inspector_focused { " [focused]" } else { "" }
                ))
                .title_style(if inspector_focused {
                    Theme::title_style()
                } else {
                    Theme::size_style()
                })
                .borders(Borders::ALL)
                .border_style(if inspector_focused {
                    ratatui::style::Style::default().fg(ratatui::style::Color::LightBlue)
                } else {
                    Theme::border_style()
                });
            frame.render_widget(
                InspectorWidget {
                    block: inspector_block,
                    info: state.inspector_info.as_ref(),
                    pinned: &state.pinned_inspector,
                    pin_scroll: state.inspector_pin_scroll,
                    scroll_row_offset: state.pin_scroll_anim.row_offset(),
                    selected_pin: if state.pane_focus != PaneFocus::Inspector
                        || state.pinned_inspector.is_empty()
                    {
                        None
                    } else {
                        Some(state.inspector_selected_pin)
                    },
                    has_focus: state.pane_focus == PaneFocus::Inspector,
                    image_cache: &state.image_cache,
                },
                layout.inspector_area,
            );

            // Scanning indicator (top-right of tree area, overlays the border).
            frame.render_widget(
                ScanIndicator {
                    visible: state.scanning,
                    tick: tick_count,
                },
                layout.tree_area,
            );

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
                            state: &state,
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
                old.request_cancel();
            }
            size_compute = Some(start_size_computation(&mut state, &size_tx));
            if let Some(ref mut compute) = size_compute {
                state.scanning = compute.is_scanning();
                finalize_ready_dirs(&mut state, compute);
            }
        }

        tokio::select! {
            biased;

            Some(event) = events.recv() => {
                match event {
                    AppEvent::Key(k) => handler::handle_key(&mut state, k),
                    AppEvent::Mouse(m) => handler::handle_mouse(&mut state, m),
                    AppEvent::Resize(w, h) => {
                        state.terminal_area = Rect::new(0, 0, w, h);
                    }
                    AppEvent::Tick => {
                        tick_count = tick_count.wrapping_add(1);
                        // Drive smooth-scroll animation (detect target change → inject row offset → decay).
                        state.pin_scroll_anim.set_target(state.inspector_pin_scroll, 12.0);
                        state.pin_scroll_anim.tick();
                    }
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

                // Update scanning flag.
                if let Some(ref compute) = size_compute {
                    state.scanning = compute.is_scanning();
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

fn refresh_inspector(state: &mut AppState) {
    let selected = handler::selected_node_path(state);
    if selected == state.inspector_path {
        // Keep directory/file apparent size fresh even when selection stays put
        // and async size workers are still updating maps.
        if let (Some(path), Some(info)) = (selected.as_ref(), state.inspector_info.as_mut()) {
            if let Some(sz) = state.dir_sizes.get(path).copied() {
                info.size_bytes = Some(sz);
            } else if let Some(sz) = state.file_sizes.get(path).copied() {
                info.size_bytes = Some(sz);
            }
        }
        for pin in &mut state.pinned_inspector {
            if let Some(sz) = state.dir_sizes.get(&pin.path).copied() {
                pin.size_bytes = Some(sz);
            } else if let Some(sz) = state.file_sizes.get(&pin.path).copied() {
                pin.size_bytes = Some(sz);
            }
        }
        // Ensure images are cached for current + pinned.
        cache_images_if_needed(state);
        return;
    }
    state.inspector_path = selected.clone();
    state.inspector_info = selected.as_ref().map(|path| {
        let mut info = crate::core::inspector::inspect_path(path);
        if let Some(sz) = state.dir_sizes.get(path).copied() {
            info.size_bytes = Some(sz);
        } else if let Some(sz) = state.file_sizes.get(path).copied() {
            info.size_bytes = Some(sz);
        }
        info
    });

    for pin in &mut state.pinned_inspector {
        if let Some(sz) = state.dir_sizes.get(&pin.path).copied() {
            pin.size_bytes = Some(sz);
        } else if let Some(sz) = state.file_sizes.get(&pin.path).copied() {
            pin.size_bytes = Some(sz);
        }
    }
    cache_images_if_needed(state);
}

/// Lazily decode images for the current selection and all pinned cards.
/// Results are cached so each image is only decoded once.
fn cache_images_if_needed(state: &mut AppState) {
    // Collect paths that need decoding to avoid borrow issues.
    let mut to_decode: Vec<std::path::PathBuf> = Vec::new();

    if let Some(info) = &state.inspector_info {
        if info.is_image() && !state.image_cache.contains_key(&info.path) {
            to_decode.push(info.path.clone());
        }
    }
    for pin in &state.pinned_inspector {
        if pin.is_image() && !state.image_cache.contains_key(&pin.path) {
            to_decode.push(pin.path.clone());
        }
    }

    for path in to_decode {
        if let Ok(img) = image::open(&path) {
            state
                .image_cache
                .insert(path, std::sync::Arc::new(img));
        }
    }
}
