//! The theme picker. The highlighted theme is already live, so the whole screen is the preview.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::inset;
use crate::app::State;
use crate::theme::{THEMES, Theme, swatch};

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme, original: usize) {
    let mut lines = vec![
        Line::styled("Colour theme", t.title),
        Line::styled(
            "the interface shows the highlighted theme as you move",
            t.muted,
        ),
        Line::from(""),
    ];
    for (i, def) in THEMES.iter().enumerate() {
        let highlighted = i == state.theme;
        let mut spans = vec![
            if highlighted {
                Span::styled("> ", t.accent)
            } else {
                Span::raw("  ")
            },
            Span::styled(
                format!("{:<24}", def.name),
                if highlighted { t.title } else { t.text },
            ),
        ];
        match swatch(i, state.color_depth) {
            Some((bg, colors)) => {
                spans.push(Span::styled(" ", Style::new().bg(bg)));
                for color in colors {
                    spans.push(Span::styled("██ ", Style::new().fg(color).bg(bg)));
                }
            }
            None => spans.push(Span::styled(" no colour", t.muted)),
        }
        if i == original {
            spans.push(Span::styled("  current", t.muted));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inset(area));
}
