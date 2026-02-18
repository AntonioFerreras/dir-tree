//! Scanning indicator — a small spinner + label rendered in the top-right
//! corner of a given area.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

/// Braille-dot spinner frames.  Cycles through these on each tick.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A small "scanning…" indicator with a spinning icon.
///
/// Render this on top of the tree area's border.  It picks its own
/// position (top-right of `area`) and is invisible when `visible` is false.
pub struct ScanIndicator {
    /// Whether to show the indicator at all.
    pub visible: bool,
    /// Monotonically increasing tick counter (drives the spinner frame).
    pub tick: u64,
}

impl Widget for ScanIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.visible || area.width < 16 || area.height == 0 {
            return;
        }

        let frame = SPINNER_FRAMES[(self.tick as usize) % SPINNER_FRAMES.len()];
        let label = format!(" {frame} scanning ");

        let label_width = label.len() as u16;
        // Position: top-right, inside the border (leave 1 col for the border char).
        let x = area.x + area.width.saturating_sub(label_width + 2);
        let y = area.y; // top border row

        let line = Line::from(Span::styled(
            label,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));

        buf.set_line(x, y, &line, label_width);
    }
}

