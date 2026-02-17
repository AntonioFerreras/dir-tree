//! Popup overlay widgets for the settings menu and controls submenu.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};

use crate::app::handler::SETTINGS_ITEMS;

// ───────────────────────────────────────── settings popup ────

/// Settings menu popup overlay.
pub struct SettingsPopup {
    pub selected: usize,
}

impl Widget for SettingsPopup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = centered_fixed(35, 7, area);
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
            lines.push(Line::from(Span::styled(
                format!("{prefix}{item}"),
                style,
            )));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Esc close",
            Style::default().fg(Color::DarkGray),
        )));

        Paragraph::new(lines).render(inner, buf);
    }
}

// ───────────────────────────────────────── controls popup ────

/// Controls / keybinding reference popup overlay.
pub struct ControlsPopup;

impl Widget for ControlsPopup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup = centered_fixed(44, 22, area);
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

        let section = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let key = Style::default().fg(Color::Yellow);
        let desc = Style::default().fg(Color::White);
        let dim = Style::default().fg(Color::DarkGray);

        let lines = vec![
            Line::raw(""),
            Line::from(Span::styled("  Navigation", section)),
            control_line("    ↑ / k", "Move up", key, desc),
            control_line("    ↓ / j", "Move down", key, desc),
            control_line("    ← / h", "Collapse / parent", key, desc),
            control_line("    → / l", "Expand directory", key, desc),
            control_line("    Alt+↑", "Prev sibling dir", key, desc),
            control_line("    Alt+↓", "Next sibling dir", key, desc),
            control_line("    Enter", "cd into directory", key, desc),
            Line::raw(""),
            Line::from(Span::styled("  View", section)),
            control_line("    .", "Toggle hidden files", key, desc),
            Line::raw(""),
            Line::from(Span::styled("  General", section)),
            control_line("    ?", "Settings menu", key, desc),
            control_line("    q / Ctrl+c", "Quit", key, desc),
            Line::raw(""),
            Line::from(Span::styled("  Esc back  q close", dim)),
        ];

        Paragraph::new(lines).render(inner, buf);
    }
}

// ───────────────────────────────────────── helpers ───────────

fn control_line<'a>(
    key_text: &'a str,
    desc_text: &'a str,
    key_style: Style,
    desc_style: Style,
) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key_text:<16}"), key_style),
        Span::styled(desc_text, desc_style),
    ])
}

/// Create a centered rectangle with fixed dimensions, clamped to the available area.
fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

