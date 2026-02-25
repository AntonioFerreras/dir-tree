//! Search tab widget (query input, options, and ranked results).

use std::path::{Component, Path};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use crate::core::search::SearchResult;
use crate::ui::theme::Theme;

pub struct SearchWidget<'a> {
    pub block: Block<'a>,
    pub root: &'a Path,
    pub query: &'a str,
    pub case_sensitive: bool,
    pub results: &'a [SearchResult],
    pub selected: Option<usize>,
    pub scroll: usize,
    pub has_focus: bool,
    pub pin_hint: &'a str,
}

impl<'a> Widget for SearchWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = self.block.inner(area);
        self.block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let mut y = inner.y;
        let bottom = inner.y + inner.height;

        let search_prompt = if self.has_focus { "Search [focused]" } else { "Search" };
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{search_prompt}: "),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                self.query.to_string(),
                Style::default().add_modifier(Modifier::UNDERLINED),
            ),
        ]))
        .render(Rect::new(inner.x, y, inner.width, 1), buf);
        y = y.saturating_add(1);
        if y >= bottom {
            return;
        }

        let case_text = if self.case_sensitive {
            "[x] case-sensitive (Alt+c)"
        } else {
            "[ ] case-sensitive (Alt+c)"
        };
        Paragraph::new(Line::from(vec![Span::styled(case_text, Theme::size_style())]))
        .render(Rect::new(inner.x, y, inner.width, 1), buf);
        y = y.saturating_add(1);
        if y >= bottom {
            return;
        }

        let root_text = format!("searching within {}/", self.root.display());
        Paragraph::new(Line::from(Span::styled(root_text, Theme::size_style())))
            .render(Rect::new(inner.x, y, inner.width, 1), buf);
        y = y.saturating_add(1);
        if y >= bottom {
            return;
        }

        Paragraph::new(Line::from(Span::styled(
            "Results",
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .render(Rect::new(inner.x, y, inner.width, 1), buf);
        y = y.saturating_add(1);
        if y >= bottom {
            return;
        }

        let max_rows = search_results_capacity(inner);
        if self.results.is_empty() {
            let empty = if self.query.trim().is_empty() {
                "Type to search."
            } else {
                "No matches."
            };
            Paragraph::new(Line::from(Span::styled(empty, Theme::size_style())))
                .render(Rect::new(inner.x, y, inner.width, 1), buf);
            return;
        }

        let scroll = self.scroll.min(self.results.len().saturating_sub(1));
        for (row_idx, result) in self
            .results
            .iter()
            .enumerate()
            .skip(scroll)
            .take(max_rows)
            .map(|(idx, r)| (idx - scroll, r))
        {
            let absolute_idx = scroll + row_idx;
            let selected = self.selected == Some(absolute_idx);
            let style = if selected {
                Theme::selected_style()
            } else if result.is_dir {
                Theme::dir_style()
            } else {
                Theme::file_style()
            };
            let marker = if selected { "> " } else { "  " };
            let parent = result.path.parent().unwrap_or(self.root);
            let avail_for_parent = inner.width.saturating_sub(20) as usize;
            let compact_parent = truncate_parent_path(parent, avail_for_parent.max(8));
            let mut spans = vec![Span::styled(
                format!("{marker}{}  {}", result.name, compact_parent),
                style,
            )];
            if selected && !result.is_dir {
                spans.push(Span::styled(
                    format!("  {} to pin file on inspector", self.pin_hint),
                    Theme::root_hint_style(),
                ));
            }
            Paragraph::new(Line::from(spans))
                .render(Rect::new(inner.x, y + row_idx as u16, inner.width, 1), buf);
        }

        render_scrollbar(
            Rect::new(inner.x, y, inner.width, max_rows as u16),
            self.results.len(),
            scroll,
            max_rows,
            buf,
        );
    }
}

/// Number of rows available for search results (below the header fields).
pub fn search_results_capacity(inner: Rect) -> usize {
    inner.height.saturating_sub(4) as usize
}

fn truncate_parent_path(path: &Path, max_chars: usize) -> String {
    let as_text = path.display().to_string();
    let full_len = as_text.chars().count();
    if full_len <= max_chars {
        return as_text;
    }
    if max_chars <= 6 {
        return "...".to_string();
    }

    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            Component::RootDir => Some("/".to_string()),
            _ => None,
        })
        .collect();

    if parts.len() <= 2 {
        return middle_ellipsis(&as_text, max_chars);
    }

    let last_dir = parts.last().cloned().unwrap_or_default();
    let mut prefix = String::new();
    if as_text.starts_with('/') {
        prefix.push('/');
    }
    let candidate = format!("{prefix}.../{last_dir}");
    if candidate.chars().count() <= max_chars {
        return candidate;
    }

    middle_ellipsis(&as_text, max_chars)
}

fn middle_ellipsis(s: &str, max_chars: usize) -> String {
    let len = s.chars().count();
    if len <= max_chars {
        return s.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let left = (max_chars - 3) / 2;
    let right = max_chars - 3 - left;
    let mut left_part = String::new();
    let mut right_part = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i < left {
            left_part.push(ch);
        }
        if i >= len.saturating_sub(right) {
            right_part.push(ch);
        }
    }
    format!("{left_part}...{right_part}")
}

fn render_scrollbar(
    area: Rect,
    total: usize,
    offset: usize,
    visible: usize,
    buf: &mut Buffer,
) {
    use ratatui::layout::Position;

    if total <= visible || area.height < 2 || area.width == 0 {
        return;
    }

    let x = area.x + area.width.saturating_sub(1);
    let h = area.height as f64;
    let thumb_sz = ((visible as f64 / total as f64) * h).ceil().max(1.0) as u16;
    let max_off = total.saturating_sub(visible) as f64;
    let thumb_pos = if max_off > 0.0 {
        ((offset as f64 / max_off) * (h - thumb_sz as f64)).round() as u16
    } else {
        0
    };

    for row in 0..area.height {
        let y = area.y + row;
        let is_thumb = row >= thumb_pos && row < thumb_pos + thumb_sz;
        let (ch, fg) = if is_thumb {
            ('█', ratatui::style::Color::LightBlue)
        } else {
            ('│', ratatui::style::Color::DarkGray)
        };
        if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
            cell.set_char(ch).set_fg(fg);
        }
    }
}


