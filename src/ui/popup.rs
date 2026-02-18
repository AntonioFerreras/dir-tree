//! Popup overlay widgets for the settings menu and controls submenu.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};

use crate::app::handler::SETTINGS_ITEMS;
use crate::config::{Action, AppConfig};
use crate::core::fs::WalkConfig;

// ───────────────────────────────────────── settings popup ────

/// Settings menu popup overlay.
pub struct SettingsPopup<'a> {
    pub selected: usize,
    pub walk_config: &'a WalkConfig,
}

impl<'a> Widget for SettingsPopup<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let height = (SETTINGS_ITEMS.len() as u16) + 6;
        let popup = centered_fixed(40, height, area);
        Clear.render(popup, buf);

        let block = Block::default()
            .title(" Settings ")
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(popup);
        block.render(popup, buf);

        let mut lines = Vec::new();
        lines.push(Line::raw(""));
        for (i, item) in SETTINGS_ITEMS.iter().enumerate() {
            let (prefix, style) = if i == self.selected {
                (
                    " ▸ ",
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("   ", Style::default().fg(Color::White))
            };

            // For toggle items, show the current state.
            let suffix = match i {
                1 => {
                    if self.walk_config.dedup_hard_links { "  [ON]" } else { "  [OFF]" }
                }
                2 => {
                    if self.walk_config.one_file_system { "  [ON]" } else { "  [OFF]" }
                }
                _ => "",
            };
            let toggle_style = if suffix.contains("ON") {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            if suffix.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{item}"),
                    style,
                )));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("{prefix}{item}"), style),
                    Span::styled(suffix, toggle_style),
                ]));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Enter/Space: toggle  Esc: close",
            Style::default().fg(Color::DarkGray),
        )));

        Paragraph::new(lines).render(inner, buf);
    }
}

// ───────────────────────────────────────── controls popup ────

/// Interactive controls / keybinding popup overlay.
pub struct ControlsPopup<'a> {
    pub config: &'a AppConfig,
    pub selected: usize,
    pub awaiting_rebind: bool,
}

impl<'a> Widget for ControlsPopup<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Action::ALL.len() actions + 2 blanks + 1 reset + 1 hint + 2 border = ~17
        let height = (Action::ALL.len() as u16) + 7;
        let popup = centered_fixed(52, height, area);
        Clear.render(popup, buf);

        let block = Block::default()
            .title(" Controls ")
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(popup);
        block.render(popup, buf);

        let dim = Style::default().fg(Color::DarkGray);
        let mut lines = Vec::new();

        lines.push(Line::raw(""));

        // ── Action rows ─────────────────────────────────────────
        for (i, &action) in Action::ALL.iter().enumerate() {
            let is_selected = i == self.selected;

            let prefix = if is_selected { " ▸ " } else { "   " };
            let label = action.label();

            let keys_display = if is_selected && self.awaiting_rebind {
                "Press a key…".to_string()
            } else {
                self.config.display_bindings(action)
            };

            let base_style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let key_style = if is_selected && self.awaiting_rebind {
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            };

            // Fixed-width columns: label left-aligned, keys right-aligned.
            let label_col = format!("{prefix}{label:<22}");
            let inner_width = inner.width as usize;
            let keys_width = inner_width.saturating_sub(label_col.len()).max(1);
            let keys_col = format!("{keys_display:>keys_width$}");

            lines.push(Line::from(vec![
                Span::styled(label_col, base_style),
                Span::styled(keys_col, key_style),
            ]));
        }

        // ── Reset option ────────────────────────────────────────
        let reset_idx = Action::ALL.len();
        let is_reset_selected = self.selected == reset_idx;

        lines.push(Line::raw(""));
        let reset_style = if is_reset_selected {
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let reset_prefix = if is_reset_selected { " ▸ " } else { "   " };
        lines.push(Line::from(Span::styled(
            format!("{reset_prefix}⟳ Reset to defaults"),
            reset_style,
        )));

        // ── Hint bar ────────────────────────────────────────────
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Enter: add key  Del: clear  Esc: back",
            dim,
        )));

        Paragraph::new(lines).render(inner, buf);
    }
}

// ───────────────────────────────────────── helpers ───────────

/// Create a centered rectangle with fixed dimensions, clamped to the available area.
fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}
