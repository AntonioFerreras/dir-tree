//! Layout helpers — split the terminal area into regions.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Primary screen layout with tree pane and a bottom status/command bar.
pub struct AppLayout {
    pub tree_area: Rect,
    pub status_area: Rect,
}

impl AppLayout {
    /// Compute the layout from the full terminal area.
    pub fn from_area(area: Rect) -> Self {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // tree pane (takes all remaining space)
                Constraint::Length(1), // status / command bar
            ])
            .split(area);

        Self {
            tree_area: chunks[0],
            status_area: chunks[1],
        }
    }
}

