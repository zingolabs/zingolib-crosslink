use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table, TableState};

use super::{inset, note};
use crate::app::{AddressKind, AddressRow, Pool, State};
use crate::format;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    if state.addresses.is_empty() {
        note(frame, area, "No addresses yet.", t);
        return;
    }
    let area = inset(area);
    let fixed = 12 + 2 + 18 + 2 + 16 + 2;
    let addr_width = usize::from(area.width.saturating_sub(fixed));
    let rows = state.addresses.iter().map(|a| {
        let (kind, kind_style) = match a.kind {
            AddressKind::Unified { .. } => ("unified", t.accent),
            AddressKind::Transparent { .. } => ("transparent", t.pool(Pool::Transparent)),
        };
        Row::new(vec![
            Cell::from(Span::styled(kind, kind_style)),
            Cell::from(receivers(a, t)),
            Cell::from(Span::styled(
                format::truncate_middle(&a.address, addr_width),
                t.info,
            )),
            Cell::from(if a.received > 0 {
                Span::styled(format::zec(a.received), t.positive)
            } else {
                Span::styled("–", t.muted)
            }),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(18),
            Constraint::Fill(1),
            Constraint::Length(16),
        ],
    )
    .column_spacing(2)
    .header(Row::new(vec!["type", "receivers", "address", "received"]).style(t.muted))
    .row_highlight_style(t.selection);
    let mut ts = TableState::default().with_selected(Some(state.addr_selected));
    frame.render_stateful_widget(table, area, &mut ts);
}

/// Receiver names in their pool colours, joined by a muted `+`.
fn receivers(a: &AddressRow, t: &Theme) -> Line<'static> {
    let AddressKind::Unified {
        orchard,
        sapling,
        transparent,
    } = a.kind
    else {
        // the type column already says transparent; there is nothing more to say
        return Line::default();
    };
    let pools = [
        (orchard, Pool::Orchard),
        (sapling, Pool::Sapling),
        (transparent, Pool::Transparent),
    ];
    let mut spans = Vec::new();
    for (_, pool) in pools.into_iter().filter(|(present, _)| *present) {
        if !spans.is_empty() {
            spans.push(Span::styled("+", t.muted));
        }
        spans.push(Span::styled(pool.name(), t.pool(pool)));
    }
    Line::from(spans)
}

pub fn draw_qr(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    match state.selected_address() {
        Some(a) => super::receive::draw_address(frame, inset(area), t, a, state.ascii_qr),
        None => note(frame, area, "No address selected.", t),
    }
}
