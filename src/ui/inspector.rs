//! Inspector panel widget.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use crate::core::{grouping, inspector::InspectorInfo};
use crate::ui::theme::Theme;

pub struct InspectorWidget<'a> {
    pub block: Block<'a>,
    pub info: Option<&'a InspectorInfo>,
}

impl<'a> Widget for InspectorWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = self.block.inner(area);
        self.block.render(area, buf);

        let mut lines = Vec::new();
        lines.push(Line::raw(""));

        if let Some(info) = self.info {
            lines.push(Line::from(Span::styled(
                info.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            let subtitle = match &info.detected_type {
                Some(t) => format!("{} · {}", info.kind, t),
                None => info.kind.clone(),
            };
            lines.push(Line::from(Span::styled(subtitle, Theme::size_style())));
            lines.push(Line::raw(""));

            lines.push(line("Path", &info.path.display().to_string()));
            if let Some(size) = info.size_bytes {
                lines.push(line("Size", &format!("{}  ({size} B)", grouping::human_size(size))));
            }
            lines.push(line("Readonly", if info.readonly { "yes" } else { "no" }));
            if let (Some(sym), Some(oct)) = (&info.perms_symbolic, &info.perms_octal) {
                lines.push(line("Permissions", &format!("{sym}  ({oct})")));
            }
            if let Some(modified) = info.modified_unix {
                lines.push(line("Modified", &format_ts(modified)));
            }
            if let Some(created) = info.created_unix {
                lines.push(line("Created", &format_ts(created)));
            }
            if let Some(target) = &info.symlink_target {
                lines.push(line("Symlink ->", target));
            }
            if let Some(subdirs) = info.subdirs {
                lines.push(line("Subdirs", &subdirs.to_string()));
            }
            if let Some(subfiles) = info.subfiles {
                lines.push(line("Subfiles", &subfiles.to_string()));
            }
            if let Some(others) = info.others {
                lines.push(line("Other entries", &others.to_string()));
            }
            if let Some(err) = &info.error {
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(Color::LightRed),
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "Select a file or directory to inspect.",
                Style::default().fg(Color::DarkGray),
            )));
        }

        Paragraph::new(lines).render(inner, buf);
    }
}

fn line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<12}"),
            Theme::size_style(),
        ),
        Span::raw(value.to_string()),
    ])
}

fn format_ts(unix_secs: u64) -> String {
    use chrono::{Local, TimeZone};
    let secs = i64::try_from(unix_secs).unwrap_or(i64::MAX);
    match Local.timestamp_opt(secs, 0).single() {
        Some(dt) => dt.format("%Y/%m/%d %H:%M").to_string(),
        None => "-".to_string(),
    }
}

