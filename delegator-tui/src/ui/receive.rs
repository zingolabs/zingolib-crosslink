//! Receive: an address to give to whoever is paying, and its QR code. Nothing else.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{inset, note};
use crate::app::{AddressKind, AddressRow, ReceiveKind, State};
use crate::theme::Theme;
use crate::{format, qr};

/// Narrowest the address column may be for the QR code to sit beside it.
const TEXT_COLUMN: u16 = 30;
/// Widest the address column grows, so on a wide terminal the QR code stays next to the
/// address instead of drifting to the far edge.
const TEXT_MAX: u16 = 48;

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let Some(address) = state.receive_address() else {
        let msg = match state.receive_kind {
            ReceiveKind::Unified => "No unified address yet. Press n to make one.",
            ReceiveKind::Transparent => "No transparent address yet. Press n to make one.",
        };
        note(frame, area, msg, t);
        return;
    };
    draw_address(frame, inset(area), t, address, state.ascii_qr);
}

/// The address with a short note, and its QR code beside it when there is room, below it
/// otherwise. When the code fits nowhere, the text takes the whole width and says so.
pub fn draw_address(frame: &mut Frame, area: Rect, t: &Theme, address: &AddressRow, ascii: bool) {
    // Themed QR codes use high-contrast versions of the theme's own tones. The ASCII fallback
    // and the monochrome theme keep the terminal's default colours.
    let colours = t.qr.filter(|_| !ascii);
    let code = qr::render(&address.address, ascii, colours.is_none());
    let (qr_cols, qr_rows) = match &code {
        Ok(lines) => (
            lines.first().map_or(0, |l| l.chars().count()) as u16,
            lines.len() as u16,
        ),
        Err(_) => (u16::MAX, u16::MAX),
    };

    let beside = area.width >= qr_cols.saturating_add(2 + TEXT_COLUMN) && qr_rows <= area.height;
    let text_width = if beside {
        (area.width - qr_cols - 2).min(TEXT_MAX)
    } else {
        area.width
    };
    let mut text = address_text(t, address, text_width);
    let below = !beside
        && qr_cols <= area.width
        && (text.len() as u16 + 1).saturating_add(qr_rows) <= area.height;

    let qr_area = if beside {
        let [left, _, right, _] = Layout::horizontal([
            Constraint::Length(text_width),
            Constraint::Length(2),
            Constraint::Length(qr_cols),
            Constraint::Fill(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), left);
        Some(right)
    } else if below {
        let [top, _, bottom] = Layout::vertical([
            Constraint::Length(text.len() as u16),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), top);
        Some(bottom)
    } else {
        text.push(Line::from(""));
        text.push(match &code {
            Ok(_) => Line::styled("Make the terminal larger to show the QR code.", t.muted),
            Err(e) => Line::styled(e.clone(), t.negative),
        });
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
        None
    };

    if let (Some(qr_area), Ok(lines)) = (qr_area, code) {
        let style = colours.unwrap_or(t.text);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .map(|l| Line::from(Span::styled(l, style)))
                    .collect::<Vec<_>>(),
            ),
            qr_area,
        );
    }
}

/// Title, one note that matters, and the address wrapped to `width`.
fn address_text(t: &Theme, address: &AddressRow, width: u16) -> Vec<Line<'static>> {
    let mut text = vec![Line::styled(
        match address.kind {
            AddressKind::Unified { .. } => "Unified address",
            AddressKind::Transparent { .. } => "Transparent address",
        },
        t.title,
    )];
    text.push(match address.kind {
        AddressKind::Unified { .. } => {
            Line::styled(address.kind.describe().replace('+', " and "), t.muted)
        }
        // what the owner should know before handing it out
        AddressKind::Transparent { .. } => {
            Line::styled("Public: anyone can see what is paid to it.", t.warning)
        }
    });
    text.push(Line::from(""));
    for l in format::wrap_chars(&address.address, usize::from(width)) {
        text.push(Line::styled(l, t.info));
    }
    text
}

pub fn draw_new_address(frame: &mut Frame, area: Rect, t: &Theme, orchard_only: bool) {
    let option = |selected: bool, label: &str| {
        Line::from(vec![
            if selected {
                Span::styled("> ", t.accent)
            } else {
                Span::raw("  ")
            },
            Span::styled(label.to_string(), if selected { t.title } else { t.text }),
        ])
    };
    let lines = vec![
        Line::styled("New unified address", t.title),
        Line::styled("choose the receivers it includes", t.muted),
        Line::from(""),
        option(!orchard_only, "orchard and sapling"),
        option(orchard_only, "orchard only"),
    ];
    frame.render_widget(Paragraph::new(lines), inset(area));
}
