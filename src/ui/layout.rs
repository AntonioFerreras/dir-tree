//! Layout helpers — split the terminal area into regions.

use crate::config::PanelLayoutMode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Primary screen layout with tree + inspector panes and a status bar.
pub struct AppLayout {
    pub tree_area: Rect,
    pub inspector_area: Rect,
    pub splitter_area: Rect,
    pub status_area: Rect,
    main_area: Rect,
    mode: PanelLayoutMode,
}

impl AppLayout {
    /// Compute the layout from the full terminal area.
    pub fn from_area(area: Rect, mode: PanelLayoutMode, split_pct: u16) -> Self {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // main panes (tree + inspector)
                Constraint::Length(1), // status / command bar
            ])
            .split(area);

        let main_area = chunks[0];
        let status_area = chunks[1];
        let split_pct = split_pct.clamp(10, 90);

        let (tree_area, inspector_area, splitter_area) = match mode {
            PanelLayoutMode::TreeLeft | PanelLayoutMode::TreeRight => {
                let panes = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(split_pct),
                        Constraint::Length(1),
                        Constraint::Min(10),
                    ])
                    .split(main_area);

                if mode == PanelLayoutMode::TreeLeft {
                    (panes[0], panes[2], panes[1])
                } else {
                    (panes[2], panes[0], panes[1])
                }
            }
            PanelLayoutMode::TreeTop | PanelLayoutMode::TreeBottom => {
                let panes = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage(split_pct),
                        Constraint::Length(1),
                        Constraint::Min(4),
                    ])
                    .split(main_area);

                if mode == PanelLayoutMode::TreeTop {
                    (panes[0], panes[2], panes[1])
                } else {
                    (panes[2], panes[0], panes[1])
                }
            }
        };

        Self {
            tree_area,
            inspector_area,
            splitter_area,
            status_area,
            main_area,
            mode,
        }
    }

    pub fn is_on_splitter(&self, col: u16, row: u16) -> bool {
        Self::contains(self.splitter_area, col, row)
    }

    /// Convert a pointer position to a split percentage for the current mode.
    pub fn split_pct_from_pointer(&self, col: u16, row: u16) -> Option<u16> {
        if !Self::contains(self.main_area, col, row) {
            return None;
        }

        let pct = match self.mode {
            PanelLayoutMode::TreeLeft => Self::pct_from_x(self.main_area, col),
            PanelLayoutMode::TreeRight => 100u16.saturating_sub(Self::pct_from_x(self.main_area, col)),
            PanelLayoutMode::TreeTop => Self::pct_from_y(self.main_area, row),
            PanelLayoutMode::TreeBottom => {
                100u16.saturating_sub(Self::pct_from_y(self.main_area, row))
            }
        };

        Some(pct.clamp(10, 90))
    }

    fn contains(r: Rect, col: u16, row: u16) -> bool {
        col >= r.x
            && col < r.x.saturating_add(r.width)
            && row >= r.y
            && row < r.y.saturating_add(r.height)
    }

    fn pct_from_x(area: Rect, col: u16) -> u16 {
        if area.width <= 1 {
            return 50;
        }
        let rel = col.saturating_sub(area.x) as u32;
        ((rel * 100) / (area.width as u32)).clamp(10, 90) as u16
    }

    fn pct_from_y(area: Rect, row: u16) -> u16 {
        if area.height <= 1 {
            return 50;
        }
        let rel = row.saturating_sub(area.y) as u32;
        ((rel * 100) / (area.height as u32)).clamp(10, 90) as u16
    }
}

