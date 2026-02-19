//! Full-screen image lightbox overlay.
//!
//! Renders a large image preview centred on the terminal with navigation
//! arrows, a close button, and a position indicator (e.g. "3 / 7").

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use crate::core::inspector::InspectorInfo;

/// The lightbox overlay widget.
pub struct LightboxWidget<'a> {
    /// All pinned items (we show images from this list).
    pub pinned: &'a [InspectorInfo],
    /// Index into `pinned` of the currently displayed image.
    pub current: usize,
    /// Pre-resized thumbnail cache.
    pub image_cache: &'a HashMap<PathBuf, Arc<image::RgbaImage>>,
}

/// Clickable regions returned after rendering, for mouse hit-testing.
#[derive(Debug, Clone, Copy)]
pub struct LightboxHitZones {
    pub close_rect: Rect,
    pub prev_rect: Rect,
    pub next_rect: Rect,
}

impl<'a> LightboxWidget<'a> {
    /// Compute the overlay area (centred, 80% of terminal).
    fn overlay_area(terminal: Rect) -> Rect {
        let margin_x = (terminal.width as f32 * 0.1).round() as u16;
        let margin_y = (terminal.height as f32 * 0.1).round() as u16;
        Rect::new(
            terminal.x + margin_x,
            terminal.y + margin_y,
            terminal.width.saturating_sub(margin_x * 2).max(20),
            terminal.height.saturating_sub(margin_y * 2).max(8),
        )
    }

    /// Render and return hit zones for mouse interaction.
    pub fn render_and_hit(self, terminal_area: Rect, buf: &mut Buffer) -> LightboxHitZones {
        let area = Self::overlay_area(terminal_area);

        // Clear the background.
        Clear.render(area, buf);

        // Collect only pinned images.
        let image_pins: Vec<(usize, &InspectorInfo)> = self
            .pinned
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_image())
            .collect();

        let total_images = image_pins.len();
        let display_index = image_pins
            .iter()
            .position(|&(i, _)| i == self.current)
            .unwrap_or(0);

        let info = image_pins
            .get(display_index)
            .map(|&(_, info)| info);

        // Title bar: file name + index.
        let title = if let Some(info) = info {
            format!(
                " {} — {}/{} ",
                info.name,
                display_index + 1,
                total_images,
            )
        } else {
            " No images pinned ".to_string()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightBlue))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));

        let inner = block.inner(area);
        block.render(area, buf);

        // Close button [X] on the top-right corner of the border.
        let close_rect = Rect::new(
            area.x + area.width.saturating_sub(5),
            area.y,
            3,
            1,
        );
        Paragraph::new(Line::from(Span::styled(
            "[X]",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        )))
        .render(close_rect, buf);

        // Navigation arrows on the left/right edges (vertically centred).
        let arrow_y = area.y + area.height / 2;
        let prev_rect = Rect::new(area.x, arrow_y, 3, 1);
        let next_rect = Rect::new(area.x + area.width.saturating_sub(3), arrow_y, 3, 1);

        if display_index > 0 {
            Paragraph::new(Line::from(Span::styled(
                " ◀",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )))
            .render(prev_rect, buf);
        }
        if display_index + 1 < total_images {
            Paragraph::new(Line::from(Span::styled(
                "▶ ",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )))
            .render(next_rect, buf);
        }

        // Render the image in the inner area (with some padding for arrows).
        if let Some(info) = info {
            if let Some(thumb) = self.image_cache.get(&info.path) {
                let img_area = Rect::new(
                    inner.x.saturating_add(2),
                    inner.y,
                    inner.width.saturating_sub(4),
                    inner.height.saturating_sub(1), // leave 1 row for footer
                );
                if img_area.width > 2 && img_area.height > 1 {
                    super::inspector::render_image_halfblocks_pub(thumb, img_area, buf);
                }
            } else {
                // Image not yet decoded.
                let msg = Paragraph::new(Line::from(Span::styled(
                    "Loading…",
                    Style::default().fg(Color::DarkGray),
                )));
                msg.render(
                    Rect::new(
                        inner.x + inner.width / 2 - 4,
                        inner.y + inner.height / 2,
                        10,
                        1,
                    ),
                    buf,
                );
            }
        }

        // Footer hint.
        let footer = Line::from(vec![
            Span::styled(
                " ←/→ navigate   Esc close ",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        let footer_y = inner.y + inner.height.saturating_sub(1);
        Paragraph::new(vec![footer]).render(
            Rect::new(inner.x, footer_y, inner.width, 1),
            buf,
        );

        LightboxHitZones {
            close_rect,
            prev_rect,
            next_rect,
        }
    }
}

