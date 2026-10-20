use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{inset, separator};
use crate::app::{Network, Wizard, WizardStep};
use crate::format;
use crate::theme::Theme;

const STEPS: [WizardStep; 3] = [WizardStep::Ufvk, WizardStep::Server, WizardStep::Birthday];

/// A text field: a prompt marker, the value, and a block cursor.
fn field(t: &Theme, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("> ", t.accent),
        Span::styled(value.to_string(), t.text),
        Span::styled(" ", t.cursor),
    ])
}

fn hint(t: &Theme, pairs: &[(&'static str, &'static str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, what)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*key, t.accent));
        spans.push(Span::styled(format!(" {what}"), t.muted));
    }
    Line::from(spans)
}

fn error(t: &Theme, message: &str) -> Line<'static> {
    Line::styled(message.to_string(), t.negative.add_modifier(Modifier::BOLD))
}

pub fn draw(frame: &mut Frame, area: Rect, t: &Theme, w: &Wizard) {
    let step_no = STEPS.iter().position(|s| *s == w.step).unwrap_or(0) + 1;
    let network = w.network.map_or("the network", Network::name);
    let header = super::edged(Rect { height: 1, ..area });
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("delegator-tui", t.title),
            Span::styled("   new watch-only wallet", t.text),
        ])),
        header,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("step {step_no} of {}", STEPS.len()),
            t.muted,
        ))
        .alignment(ratatui::layout::Alignment::Right),
        header,
    );
    separator(
        frame,
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
        t,
    );
    let body = inset(Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(2),
        ..area
    });
    let width = usize::from(body.width);
    let mut lines: Vec<Line<'static>> = Vec::new();
    match w.step {
        WizardStep::Ufvk => {
            lines.push(Line::styled("Paste the unified full viewing key.", t.text));
            lines.push(Line::styled(
                "Its prefix names the network: uview1 mainnet, uviewtest1 testnet, uviewregtest1 regtest. Nothing is sent anywhere.",
                t.muted,
            ));
            lines.push(Line::from(""));
            let wrapped = format::wrap_chars(&w.ufvk, width.saturating_sub(2));
            match wrapped.split_last() {
                Some((last, rest)) => {
                    for (i, l) in rest.iter().enumerate() {
                        lines.push(Line::from(vec![
                            if i == 0 {
                                Span::styled("> ", t.accent)
                            } else {
                                Span::raw("  ")
                            },
                            Span::styled(l.clone(), t.text),
                        ]));
                    }
                    let mut last_line = field(t, last);
                    if !rest.is_empty() {
                        last_line.spans[0] = Span::raw("  ");
                    }
                    lines.push(last_line);
                }
                None => lines.push(field(t, "")),
            }
            lines.push(Line::from(""));
            lines.push(hint(t, &[("Enter", "continue"), ("Esc", "quit")]));
        }
        WizardStep::Server => {
            lines.push(Line::styled(
                format!("Which {network} server should the wallet sync from?"),
                t.text,
            ));
            lines.push(Line::from(""));
            lines.push(field(t, &w.server));
            lines.push(Line::from(""));
            lines.push(hint(t, &[("Enter", "continue"), ("Esc", "back")]));
        }
        WizardStep::Birthday => {
            lines.push(Line::styled(
                "At which block height was the key created?",
                t.text,
            ));
            let floor = w.network.map_or(1, Network::sapling_activation);
            let mut hint_text = "Blocks before this height are not scanned. Too high and old funds are missed. Too low and the first sync is slow.".to_string();
            if floor > 1 {
                hint_text.push_str(&format!(
                    " It cannot be below {}, where Sapling activated on {network}.",
                    format::group(u64::from(floor))
                ));
            }
            lines.push(Line::styled(hint_text, t.muted));
            lines.push(Line::from(""));
            lines.push(field(t, &w.birthday));
            lines.push(Line::from(""));
            lines.push(hint(t, &[("Enter", "create wallet"), ("Esc", "back")]));
        }
    }
    if let Some(err) = &w.error {
        lines.push(Line::from(""));
        lines.push(error(t, err));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}
