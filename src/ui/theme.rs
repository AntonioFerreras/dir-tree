//! Colour palette and text styles used across the UI.

use ratatui::style::{Color, Modifier, Style};

/// Central theme — change colours here and they propagate everywhere.
pub struct Theme;

impl Theme {
    // ── tree view ──────────────────────────────────────────────
    pub fn dir_style() -> Style {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    }

    pub fn file_style() -> Style {
        Style::default().fg(Color::White)
    }

    pub fn group_style() -> Style {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::ITALIC)
    }

    pub fn selected_style() -> Style {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    }

    // ── chrome ─────────────────────────────────────────────────
    pub fn border_style() -> Style {
        Style::default().fg(Color::Gray)
    }

    pub fn title_style() -> Style {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_bar_style() -> Style {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    }

    pub fn command_input_style() -> Style {
        Style::default().fg(Color::Yellow)
    }
}

