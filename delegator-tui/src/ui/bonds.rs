//! Bonds and finalizers: the bonds screen lists the wallet's own delegations; the
//! finalizers screen lists every finalizer on the chain, largest stake first.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::{home, inset};
use crate::app::{BondStatus, State};
use crate::format;
use crate::theme::Theme;

/// The Bonds screen: what this wallet has at stake, and each of its bonds.
pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let area = inset(area);
    let top = home::stake_lines(state, t);
    let bonds_rows = if state.bonds.is_empty() {
        1
    } else {
        state.bonds.len() as u16 + 1
    };
    let [top_area, _, bonds_area, rest] = Layout::vertical([
        Constraint::Length(top.len() as u16),
        Constraint::Length(1),
        Constraint::Length(bonds_rows),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(top), top_area);

    if state.bonds.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "No bonds yet. Funds staked with a finalizer appear here.",
                t.muted,
            )),
            bonds_area,
        );
    } else {
        bonds_table(frame, bonds_area, state, t);
    }
    // the wallet's stake on this chain, wherever it is delegated: the chain's own
    // finalizer list has its screen under `f`
    if !state.roster.is_empty() {
        let total: u64 = state
            .roster
            .iter()
            .map(|r| r.stake)
            .fold(0, u64::saturating_add);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("the chain ", t.muted),
                Span::styled(format!("{} ZEC", format::zec(total)), t.text),
                Span::styled(
                    format!(
                        " at stake across {}",
                        match state.roster.len() {
                            1 => "one finalizer".to_string(),
                            n => format!("{n} finalizers"),
                        }
                    ),
                    t.muted,
                ),
                Span::styled("  ·  press f for the list", t.muted),
            ])),
            rest,
        );
    }
}

/// The Finalizers screen: every finalizer on the chain, largest stake first, with the stake
/// delegated to each and its share of the whole. A mark leads the ones this wallet has a bond
/// with.
pub fn draw_finalizers(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let area = inset(area);
    let [heading_area, roster_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    roster(frame, heading_area, roster_area, state, t);
}

/// The wallet's bonds: finalizer, amount, standing, height of the last action.
fn bonds_table(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let rows = state.bonds.iter().map(|bond| {
        let (status, style) = match bond.status {
            BondStatus::Bonded => (bond.status.name(), t.info),
            BondStatus::Unbonding => (bond.status.name(), t.negative),
        };
        Row::new(vec![
            Cell::from(finalizer(&bond.finalizer, t)),
            Cell::from(
                Line::from(Span::styled(
                    format::zec(bond.amount),
                    style.add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Right),
            ),
            Cell::from(Span::styled(status, style)),
            Cell::from(Span::styled(format::group(u64::from(bond.since)), t.muted)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(SHORT_WIDTH as u16 + 2),
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .column_spacing(2)
    .header(Row::new(vec!["  finalizer", "amount", "status", "since"]).style(t.muted))
    .row_highlight_style(t.selection);
    let mut ts = TableState::default().with_selected(Some(state.bond_selected));
    frame.render_stateful_widget(table, area, &mut ts);
}

/// Every finalizer on the chain, largest stake first: its address, the stake with it, and that
/// stake's share of the whole. A mark leads the ones this wallet has a bond with.
fn roster(frame: &mut Frame, heading_area: Rect, area: Rect, state: &State, t: &Theme) {
    let total: u64 = state
        .roster
        .iter()
        .map(|r| r.stake)
        .fold(0, u64::saturating_add);
    let heading_line = if state.roster.is_empty() {
        Line::styled("Finalizers  the node has not answered yet", t.muted)
    } else {
        Line::from(vec![
            Span::styled("Finalizers  ", t.muted),
            Span::styled(format!("{} ZEC", format::zec(total)), t.text),
            Span::styled(
                format!(
                    " at stake across {}",
                    match state.roster.len() {
                        1 => "one finalizer".to_string(),
                        n => format!("{n} finalizers"),
                    }
                ),
                t.muted,
            ),
        ])
    };
    frame.render_widget(Paragraph::new(heading_line), heading_area);
    if state.roster.is_empty() {
        return;
    }
    let rows = state.roster.iter().map(|r| {
        let share = if total == 0 {
            0.0
        } else {
            r.stake as f64 / total as f64 * 100.0
        };
        let mut line = finalizer(&r.finalizer, t);
        line.spans.insert(
            0,
            if r.ours {
                Span::styled("● ", t.accent)
            } else {
                Span::raw("  ")
            },
        );
        let style = if r.ours { t.text } else { t.muted };
        Row::new(vec![
            Cell::from(line),
            Cell::from(
                Line::from(Span::styled(format::zec(r.stake), style)).alignment(Alignment::Right),
            ),
            Cell::from(
                Line::from(Span::styled(format!("{share:.1}%"), style)).alignment(Alignment::Right),
            ),
            Cell::from(Span::styled(share_bar(share, 20), t.info)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(SHORT_WIDTH as u16 + 2),
            Constraint::Length(16),
            Constraint::Length(7),
            Constraint::Fill(1),
        ],
    )
    .column_spacing(2)
    .header(Row::new(vec!["  finalizer", "stake", "share", ""]).style(t.muted));
    frame.render_widget(table, area);
}

/// A bar of `cells` for a percentage, filled by share.
fn share_bar(percent: f64, cells: usize) -> String {
    let filled = ((percent / 100.0) * cells as f64).round() as usize;
    format!(
        "{}{}",
        "━".repeat(filled.min(cells)),
        "╌".repeat(cells - filled.min(cells))
    )
}

/// The finalizer address prefix, the same for every finalizer.
const PREFIX: &str = "zfinv1";
/// Characters kept at each end of the body.
const ENDS: usize = 8;
/// Width of a shortened address: the prefix, the head, an ellipsis and the tail.
const SHORT_WIDTH: usize = PREFIX.len() + ENDS + 1 + ENDS;

/// A finalizer shortened to the first and last characters of its body, the prefix kept whole
/// and in its own colour so every finalizer starts the same way. A bare key, with no prefix,
/// is shortened the same way.
pub fn finalizer(address: &str, t: &Theme) -> Line<'static> {
    let (prefix, body) = match address.strip_prefix(PREFIX) {
        Some(body) => (PREFIX, body),
        None => ("", address),
    };
    let chars: Vec<char> = body.chars().collect();
    let short = if chars.len() <= 2 * ENDS + 1 {
        body.to_string()
    } else {
        let head: String = chars[..ENDS].iter().collect();
        let tail: String = chars[chars.len() - ENDS..].iter().collect();
        format!("{head}…{tail}")
    };
    Line::from(vec![
        Span::styled(prefix, t.accent),
        Span::styled(short, t.text),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorDepth, DEFAULT_THEME, index_of};

    #[test]
    fn the_prefix_is_kept_whole_and_the_body_shortened_at_both_ends() {
        let t = Theme::new(index_of(DEFAULT_THEME).unwrap(), ColorDepth::TrueColor);
        let address = format!("zfinv1{}{}", "A".repeat(120), "zyxwvuts");
        let line = finalizer(&address, &t);
        assert_eq!(line.spans[0].content, "zfinv1");
        assert_eq!(line.spans[1].content, "AAAAAAAA…zyxwvuts");
        assert_eq!(line.width(), SHORT_WIDTH);
        assert_eq!(line.spans[0].style, t.accent);

        let bare = finalizer(&"ab".repeat(32), &t);
        assert_eq!(bare.spans[0].content, "");
        assert_eq!(bare.spans[1].content, "abababab…abababab");

        let short = finalizer("zfinv1tiny", &t);
        assert_eq!(short.spans[1].content, "tiny");
    }
}
