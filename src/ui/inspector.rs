//! Inspector panel widget — current-selection detail and pinned-card list.
//!
//! ## Architecture
//!
//! * **Geometry** (`PinCardGeometry`, `PinnedCardsGeometry`,
//!   `pinned_cards_geometry`) — pure layout math shared between the widget
//!   (rendering) and the handler (hit-testing).
//! * **Text helpers** (`current_section_lines`, `info_detail_lines`, etc.)
//!   — build `Line` vectors from `InspectorInfo`.  Pure, no side-effects.
//! * **Render helpers** (`render_current_section`, `render_card`,
//!   `render_scrollbar`, `render_image_halfblocks`) — each draws one
//!   self-contained piece into a `Buffer`.
//! * **Widget** (`InspectorWidget`) — thin orchestrator that calls the
//!   helpers above.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::core::{grouping, inspector::InspectorInfo};
use crate::ui::theme::Theme;

// ─── constants ──────────────────────────────────────────────────

const CARD_MIN_HEIGHT: u16 = 6;
const CARD_MAX_HEIGHT: u16 = 26;
const CARD_GAP: u16 = 1;
const CURRENT_PREVIEW_MAX: u16 = 12;
const CARD_PREVIEW_ROWS: u16 = 6;
const TEXT_COL_MAX: u16 = 42;
const SIDE_BY_SIDE_MIN_WIDTH: u16 = 55;

// ─── geometry ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct PinCardGeometry {
    pub pin_index: usize,
    pub card_rect: Rect,
    pub unpin_rect: Rect,
    pub is_partial: bool,
}

#[derive(Debug, Clone)]
pub struct PinnedCardsGeometry {
    pub cards: Vec<PinCardGeometry>,
    pub visible_cards: usize,
    pub max_scroll: usize,
    pub total_pins: usize,
    pub cards_area: Rect,
}

/// Height of the "Current Selection" section (text + optional image preview).
pub fn current_section_total_height(info: Option<&InspectorInfo>, panel_width: u16) -> u16 {
    let text_lines = current_section_lines(info).len() as u16;
    let is_image = info.map_or(false, |i| i.is_image());
    if is_image {
        if panel_width >= SIDE_BY_SIDE_MIN_WIDTH {
            text_lines
        } else {
            text_lines + CURRENT_PREVIEW_MAX
        }
    } else {
        text_lines
    }
}

pub fn pinned_cards_geometry(
    inner: Rect,
    info: Option<&InspectorInfo>,
    pinned: &[InspectorInfo],
    requested_scroll: usize,
) -> PinnedCardsGeometry {
    let current_height = current_section_total_height(info, inner.width);
    let cards_start_y = inner.y.saturating_add(current_height.saturating_add(2));
    let bottom = inner.y.saturating_add(inner.height);
    let available_height = bottom.saturating_sub(cards_start_y);
    let pin_count = pinned.len();
    let cards_area = Rect::new(inner.x, cards_start_y, inner.width, available_height);

    if available_height < 2 || pin_count == 0 {
        return PinnedCardsGeometry {
            cards: Vec::new(),
            visible_cards: 0,
            max_scroll: 0,
            total_pins: pin_count,
            cards_area,
        };
    }

    // Compute max_scroll: the largest scroll index where all cards from
    // that index onward fit entirely within the available height.
    let max_scroll = {
        let mut cumulative = 0u16;
        let mut first_fitting = pin_count; // will walk backwards
        for i in (0..pin_count).rev() {
            let h = card_height_for(&pinned[i]);
            let needed = if cumulative == 0 { h } else { cumulative + CARD_GAP + h };
            if needed > available_height {
                break;
            }
            cumulative = needed;
            first_fitting = i;
        }
        first_fitting
    };
    let scroll = requested_scroll.min(max_scroll);

    let mut cards = Vec::new();
    let mut y = cards_start_y;
    for pin_index in scroll..pin_count {
        let full_height = card_height_for(&pinned[pin_index]);
        let remaining = bottom.saturating_sub(y);
        if remaining < 2 {
            break;
        }
        let is_partial = full_height > remaining;
        let clamped_height = if is_partial { remaining } else { full_height };
        let card_rect = Rect::new(inner.x, y, inner.width, clamped_height);
        let unpin_rect = Rect::new(
            card_rect.x + card_rect.width.saturating_sub(5),
            card_rect.y,
            3,
            1,
        );
        cards.push(PinCardGeometry {
            pin_index,
            card_rect,
            unpin_rect,
            is_partial,
        });
        y = y.saturating_add(clamped_height.saturating_add(CARD_GAP));
    }

    PinnedCardsGeometry {
        visible_cards: cards.len(),
        cards,
        max_scroll,
        total_pins: pin_count,
        cards_area,
    }
}

// ─── widget ─────────────────────────────────────────────────────

pub struct InspectorWidget<'a> {
    pub block: Block<'a>,
    pub info: Option<&'a InspectorInfo>,
    pub pinned: &'a [InspectorInfo],
    pub pin_scroll: usize,
    /// Row offset from the smooth-scroll animator.  Positive = cards
    /// shifted down (scroll-down animation); negative = shifted up.
    pub scroll_row_offset: i16,
    pub selected_pin: Option<usize>,
    pub has_focus: bool,
    pub image_cache: &'a HashMap<PathBuf, Arc<image::RgbaImage>>,
}

impl<'a> Widget for InspectorWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = self.block.inner(area);
        self.block.render(area, buf);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // ── current selection ────────────────────────────────────
        let section_h = render_current_section(
            self.info,
            self.image_cache,
            inner,
            buf,
        );

        // ── pinned header ────────────────────────────────────────
        let header_y = inner.y.saturating_add(section_h.saturating_add(1));
        let bottom = inner.y.saturating_add(inner.height);
        if header_y >= bottom {
            return;
        }
        render_pinned_header(self.has_focus, inner.x, header_y, inner.width, buf);

        let cards_start_y = header_y.saturating_add(1) as i32;
        let area_bottom = bottom as i32;
        if cards_start_y >= area_bottom {
            return;
        }

        if self.pinned.is_empty() {
            Paragraph::new(vec![Line::from(Span::styled(
                "Pin entries by expanding them in the tree.",
                Theme::size_style(),
            ))])
            .render(Rect::new(inner.x, cards_start_y as u16, inner.width, 1), buf);
            return;
        }

        // ── compute absolute card positions (all cards, no clipping) ──
        // Build cumulative y-offsets relative to cards_start_y.
        let mut card_positions: Vec<(usize, i32, i32)> = Vec::new(); // (index, rel_y, height)
        {
            let mut y = 0i32;
            for (i, pin) in self.pinned.iter().enumerate() {
                let h = card_height_for(pin) as i32;
                card_positions.push((i, y, h));
                y += h + CARD_GAP as i32;
            }
        }

        // The scroll puts card[pin_scroll] at y=0 (top of cards area).
        // row_offset shifts everything from there (positive = cards below target).
        let scroll_y = card_positions
            .get(self.pin_scroll)
            .map(|&(_, y, _)| y)
            .unwrap_or(0);
        let shift = -scroll_y + self.scroll_row_offset as i32;

        // ── render each visible card ─────────────────────────────
        let cards_area = Rect::new(
            inner.x,
            cards_start_y as u16,
            inner.width,
            (area_bottom - cards_start_y).max(0) as u16,
        );

        for &(idx, rel_y, card_h) in &card_positions {
            let abs_y = cards_start_y + rel_y + shift;
            let abs_bottom = abs_y + card_h;

            // Skip fully off-screen cards.
            if abs_bottom <= cards_start_y || abs_y >= area_bottom {
                continue;
            }

            // Clamp to visible area.
            let vis_y = abs_y.max(cards_start_y) as u16;
            let vis_h = (abs_bottom.min(area_bottom) - vis_y as i32).max(0) as u16;
            if vis_h < 2 {
                continue;
            }

            let top_clipped = abs_y < cards_start_y;
            let bot_clipped = abs_bottom > area_bottom;
            let content_skip = if top_clipped {
                (cards_start_y - abs_y) as u16
            } else {
                0
            };

            let vis_rect = Rect::new(inner.x, vis_y, inner.width, vis_h);
            let is_selected = self.selected_pin == Some(idx);

            render_animated_card(
                &self.pinned[idx],
                vis_rect,
                is_selected,
                top_clipped,
                bot_clipped,
                content_skip,
                self.image_cache,
                buf,
            );
        }

        // ── scrollbar (uses target scroll, not animated) ─────────
        let geom = pinned_cards_geometry(inner, self.info, self.pinned, self.pin_scroll);
        render_scrollbar(
            cards_area,
            self.pinned.len(),
            self.pin_scroll,
            geom.visible_cards,
            buf,
        );
    }
}

// ─── render helpers ─────────────────────────────────────────────

/// Render the "Current Selection" section and return its total height.
fn render_current_section(
    info: Option<&InspectorInfo>,
    image_cache: &HashMap<PathBuf, Arc<image::RgbaImage>>,
    inner: Rect,
    buf: &mut Buffer,
) -> u16 {
    let lines = current_section_lines(info);
    let text_h = (lines.len() as u16).min(inner.height);

    let is_image = info.map_or(false, |i| i.is_image());
    let side_by_side = is_image && inner.width >= SIDE_BY_SIDE_MIN_WIDTH;

    if side_by_side {
        let text_w = TEXT_COL_MAX.min(inner.width / 2);
        let img_x = inner.x + text_w + 1;
        let img_w = inner.width.saturating_sub(text_w + 1);
        let section_h = text_h.min(inner.height);

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(Rect::new(inner.x, inner.y, text_w, section_h), buf);

        if img_w > 2 && section_h > 0 {
            if let Some(img) = info.and_then(|i| image_cache.get(&i.path)) {
                render_image_halfblocks(
                    img,
                    Rect::new(img_x, inner.y, img_w, section_h),
                    buf,
                );
            }
        }
        section_h
    } else {
        if text_h > 0 {
            Paragraph::new(lines)
                .render(Rect::new(inner.x, inner.y, inner.width, text_h), buf);
        }

        let preview_h = if is_image {
            let avail = inner.height.saturating_sub(text_h).min(CURRENT_PREVIEW_MAX);
            if avail > 1 {
                if let Some(img) = info.and_then(|i| image_cache.get(&i.path)) {
                    render_image_halfblocks(
                        img,
                        Rect::new(inner.x, inner.y + text_h, inner.width, avail),
                        buf,
                    );
                }
                avail
            } else {
                0
            }
        } else {
            0
        };

        text_h + preview_h
    }
}

fn render_pinned_header(focused: bool, x: u16, y: u16, w: u16, buf: &mut Buffer) {
    let header = if focused {
        Line::from(Span::styled(
            "Pinned [focused]",
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(Span::styled("Pinned", Theme::size_style()))
    };
    Paragraph::new(vec![header]).render(Rect::new(x, y, w, 1), buf);
}

/// Render a single pinned card with clipping support for smooth-scroll animation.
///
/// `vis_rect` is the on-screen area the card occupies (already clamped to the
/// visible region).  `top_clipped` / `bot_clipped` indicate which edges are
/// off-screen.  `content_skip` is the number of content rows hidden at the top.
fn render_animated_card(
    info: &InspectorInfo,
    vis_rect: Rect,
    is_selected: bool,
    top_clipped: bool,
    bot_clipped: bool,
    content_skip: u16,
    image_cache: &HashMap<PathBuf, Arc<image::RgbaImage>>,
    buf: &mut Buffer,
) {
    let border_style = if is_selected {
        Style::default().fg(Color::LightBlue)
    } else {
        Theme::border_style()
    };
    let title_style = if is_selected {
        Style::default()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    // Choose borders: omit top/bottom when clipped.
    let borders = match (top_clipped, bot_clipped) {
        (true, true) => Borders::LEFT | Borders::RIGHT,
        (true, false) => Borders::LEFT | Borders::RIGHT | Borders::BOTTOM,
        (false, true) => Borders::LEFT | Borders::RIGHT | Borders::TOP,
        (false, false) => Borders::ALL,
    };

    let mut block = Block::default()
        .borders(borders)
        .border_style(border_style);
    if !top_clipped {
        block = block.title(Span::styled(
            format!(" {} ", card_title(info)),
            title_style,
        ));
    }
    block.render(vis_rect, buf);

    // [x] unpin button — only if the top border is visible.
    if !top_clipped && vis_rect.width >= 6 {
        let unpin_rect = Rect::new(
            vis_rect.x + vis_rect.width.saturating_sub(5),
            vis_rect.y,
            3,
            1,
        );
        Paragraph::new(vec![Line::from(Span::styled(
            "[x]",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ))])
        .render(unpin_rect, buf);
    }

    // Content area (inset by border widths present).
    let top_inset: u16 = if top_clipped { 0 } else { 1 };
    let bot_inset: u16 = if bot_clipped { 0 } else { 1 };
    let ca = Rect::new(
        vis_rect.x.saturating_add(1),
        vis_rect.y.saturating_add(top_inset),
        vis_rect.width.saturating_sub(2),
        vis_rect.height.saturating_sub(top_inset + bot_inset),
    );
    if ca.width == 0 || ca.height == 0 {
        return;
    }

    let subtitle = match &info.detected_type {
        Some(t) => format!("{} · {}", info.kind, t),
        None => info.kind.clone(),
    };
    let mut body = vec![kv_line("Type", &subtitle)];
    body.extend(info_detail_lines(info));
    let body_h = body.len() as u16;

    let card_sbs = info.is_image() && ca.width >= SIDE_BY_SIDE_MIN_WIDTH;

    if card_sbs {
        let tw = TEXT_COL_MAX.min(ca.width / 2);
        let iw = ca.width.saturating_sub(tw + 1);
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .scroll((content_skip, 0))
            .render(Rect::new(ca.x, ca.y, tw, ca.height), buf);
        if iw > 2 {
            if let Some(img) = image_cache.get(&info.path) {
                render_image_halfblocks(
                    img,
                    Rect::new(ca.x + tw + 1, ca.y, iw, ca.height),
                    buf,
                );
            }
        }
    } else {
        Paragraph::new(body)
            .scroll((content_skip, 0))
            .render(ca, buf);
        if info.is_image() && content_skip < body_h {
            if let Some(img) = image_cache.get(&info.path) {
                let preview_start = body_h.saturating_sub(content_skip);
                let ph = ca.height.saturating_sub(preview_start);
                if ph > 1 {
                    render_image_halfblocks(
                        img,
                        Rect::new(ca.x, ca.y + preview_start, ca.width, ph),
                        buf,
                    );
                }
            }
        }
    }
}

// ─── text helpers ───────────────────────────────────────────────

fn current_section_lines(info: Option<&InspectorInfo>) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Current Selection",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];

    if let Some(info) = info {
        lines.push(Line::from(Span::styled(
            info.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        let sub = match &info.detected_type {
            Some(t) => format!("{} · {}", info.kind, t),
            None => info.kind.clone(),
        };
        lines.push(Line::from(Span::styled(sub, Theme::size_style())));
        lines.push(Line::raw(""));
        lines.extend(info_detail_lines(info));
    } else {
        lines.push(Line::from(Span::styled(
            "Select a file or directory to inspect.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn info_detail_lines(info: &InspectorInfo) -> Vec<Line<'static>> {
    let mut l = Vec::new();
    l.push(kv_line("Path", &info.path.display().to_string()));
    if let Some(sz) = info.size_bytes {
        l.push(kv_line(
            "Size",
            &format!("{} ({sz} B)", grouping::human_size(sz)),
        ));
    }
    l.push(kv_line("Readonly", if info.readonly { "yes" } else { "no" }));
    if let (Some(sym), Some(oct)) = (&info.perms_symbolic, &info.perms_octal) {
        l.push(kv_line("Permissions", &format!("{sym} ({oct})")));
    }
    if let Some(m) = info.modified_unix {
        l.push(kv_line("Modified", &format_ts(m)));
    }
    if let Some(c) = info.created_unix {
        l.push(kv_line("Created", &format_ts(c)));
    }
    if let Some(t) = &info.symlink_target {
        l.push(kv_line("Symlink ->", t));
    }
    if let Some(v) = info.subdirs {
        l.push(kv_line("Subdirs", &v.to_string()));
    }
    if let Some(v) = info.subfiles {
        l.push(kv_line("Subfiles", &v.to_string()));
    }
    if let Some(v) = info.others {
        l.push(kv_line("Other entries", &v.to_string()));
    }
    if let (Some(w), Some(h)) = (info.image_width, info.image_height) {
        l.push(kv_line("Resolution", &format!("{w} × {h}")));
    }
    if let Some(ref f) = info.image_pixel_format {
        l.push(kv_line("Pixel fmt", f));
    }
    if let Some(ch) = info.image_channels {
        l.push(kv_line("Channels", &ch.to_string()));
    }
    if let Some(e) = &info.error {
        l.push(Line::raw(""));
        l.push(Line::from(Span::styled(
            format!("Error: {e}"),
            Style::default().fg(Color::LightRed),
        )));
    }
    l
}

fn card_title(info: &InspectorInfo) -> String {
    if !info.name.is_empty() {
        return info.name.clone();
    }
    info.path
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| info.path.display().to_string())
}

fn card_height_for(info: &InspectorInfo) -> u16 {
    let body = 1 + info_detail_lines(info).len();
    let preview = if info.is_image() {
        CARD_PREVIEW_ROWS as usize + 1
    } else {
        0
    };
    ((body + preview + 2) as u16).clamp(CARD_MIN_HEIGHT, CARD_MAX_HEIGHT)
}

fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), Theme::size_style()),
        Span::raw(value.to_string()),
    ])
}

fn format_ts(unix_secs: u64) -> String {
    use chrono::{Local, TimeZone};
    let s = i64::try_from(unix_secs).unwrap_or(i64::MAX);
    match Local.timestamp_opt(s, 0).single() {
        Some(dt) => dt.format("%Y/%m/%d %H:%M").to_string(),
        None => "-".to_string(),
    }
}

// ─── image preview (halfblock renderer) ─────────────────────────

/// Render a pre-resized `RgbaImage` using Unicode `▀` half-blocks (2 pixels per cell).
///
/// Aspect ratio is preserved: the image is fitted inside `area` and centred
/// horizontally.  Terminal cells are ~2× taller than wide, so each cell
/// represents 1 pixel wide × 2 pixels tall; the fit calculation accounts
/// for this.
fn render_image_halfblocks(thumb: &image::RgbaImage, area: Rect, buf: &mut Buffer) {
    use image::imageops::FilterType;
    use ratatui::layout::Position;

    if area.width == 0 || area.height == 0 || thumb.width() == 0 || thumb.height() == 0 {
        return;
    }

    // Available pixel budget: each column = 1 px wide, each row = 2 px tall.
    let max_px_w = area.width as f64;
    let max_px_h = (area.height as f64) * 2.0;

    let src_w = thumb.width() as f64;
    let src_h = thumb.height() as f64;

    // Scale to fit within the pixel budget, preserving aspect ratio.
    let scale = (max_px_w / src_w).min(max_px_h / src_h).min(1.0);
    let fit_w = (src_w * scale).round().max(1.0) as u32;
    let fit_h = (src_h * scale).round().max(1.0) as u32;

    let rgba = image::imageops::resize(thumb, fit_w, fit_h, FilterType::Triangle);
    let (iw, ih) = (rgba.width(), rgba.height());

    // Centre horizontally within the area.
    let col_offset = (area.width.saturating_sub(iw as u16)) / 2;

    for row in 0..area.height {
        let yt = (row as u32) * 2;
        let yb = yt + 1;
        if yt >= ih {
            break;
        }
        for col in 0..iw.min(area.width as u32) {
            let t = rgba.get_pixel(col, yt);
            let fg = Color::Rgb(t[0], t[1], t[2]);
            let bg = if yb < ih {
                let b = rgba.get_pixel(col, yb);
                Color::Rgb(b[0], b[1], b[2])
            } else {
                Color::Reset
            };
            if let Some(cell) =
                buf.cell_mut(Position::new(area.x + col_offset + col as u16, area.y + row))
            {
                cell.set_char('▀').set_fg(fg).set_bg(bg);
            }
        }
    }
}

// ─── scrollbar ──────────────────────────────────────────────────

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
            ('█', Color::LightBlue)
        } else {
            ('│', Color::DarkGray)
        };
        if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
            cell.set_char(ch).set_fg(fg);
        }
    }
}
