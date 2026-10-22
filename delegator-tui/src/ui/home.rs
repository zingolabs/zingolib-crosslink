//! Home: how much is here, how much is at stake, and what just happened.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{bignum, inset, jacquard, transactions};
use std::collections::BTreeSet;

use crate::app::{BondStatus, State};
use crate::format;
use crate::theme::{self, Theme};

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let area = inset(area);
    let width = usize::from(area.width);
    let b = &state.balance;

    let mut top = Vec::new();
    if !state.data_loaded {
        top.push(Line::styled("unknown until the wallet connects", t.muted));
        top.push(Line::from(""));
        top.push(Line::styled("Staked", t.muted));
        top.push(Line::styled("unknown until the wallet connects", t.muted));
    } else {
        let amount = format::zec(b.total());

        // Jacquard 24's eight rows on a tall terminal, the small glyphs otherwise
        let tall = area.height >= 30;
        let big: Option<(Vec<String>, usize)> = tall
            .then(|| jacquard::render(&amount))
            .flatten()
            .map(|rows| (rows, jacquard::HEIGHT - 2))
            .or_else(|| bignum::render(&amount).map(|rows| (rows.to_vec(), 2)))
            .filter(|(rows, _)| {
                rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) + 5 <= width
            });
        match big {
            Some((rows, baseline)) => {
                for (i, row) in rows.into_iter().enumerate() {
                    let mut spans = vec![Span::styled(row, t.title)];
                    // the unit sits on the baseline row
                    if i == baseline {
                        spans.push(Span::styled("  ZEC", t.muted));
                    }
                    top.push(Line::from(spans));
                }
            }
            None => top.push(Line::styled(format!("{amount} ZEC"), t.title)),
        }
        if b.pending() > 0 {
            top.push(Line::styled(
                format!("{} ZEC pending", format::zec(b.pending())),
                t.warning,
            ));
        }
        top.push(Line::from(""));
        top.extend(stake_lines(state, t));
    }

    let [top_area, _, title_area, list_area] = Layout::vertical([
        Constraint::Length(top.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(top), top_area);
    frame.render_widget(
        Paragraph::new(Line::styled("Recent transactions", t.muted)),
        title_area,
    );
    if state.txs.is_empty() {
        let message = if !state.data_loaded {
            "Nothing known yet. The wallet is still connecting."
        } else if state.sync.sessions_done == 0 {
            "No transactions yet. The first sync is still running."
        } else {
            "No transactions yet."
        };
        frame.render_widget(Paragraph::new(Line::styled(message, t.muted)), list_area);
    } else {
        transactions::draw_list(frame, list_area, state, t, false);
    }
}

/// What is at stake: the bonded amount in normal text with a pulsing star beside it, then what
/// is unbonding and waits for a withdrawal. With nothing staked the star stays still: there is
/// nothing earning.
pub fn stake_lines(state: &State, t: &Theme) -> Vec<Line<'static>> {
    let b = &state.balance;
    let (style, star) = if b.staked > 0 {
        (
            t.info.add_modifier(Modifier::BOLD),
            theme::star(state.elapsed_secs()),
        )
    } else {
        (t.muted, ' ')
    };
    let mut lines = vec![Line::styled("Staked", t.muted)];
    let finalizers = state
        .bonds
        .iter()
        .filter(|bond| bond.status == BondStatus::Bonded)
        .map(|bond| bond.finalizer.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let with = match finalizers {
        0 => String::new(),
        1 => "  with one finalizer".into(),
        n => format!("  with {n} finalizers"),
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("{star} {} ZEC staked", format::zec(b.staked)),
            style,
        ),
        Span::styled(with, t.muted),
    ]));
    if b.unbonding > 0 {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ZEC unbonding", format::zec(b.unbonding)),
                t.negative.add_modifier(Modifier::BOLD),
            ),
            Span::styled("  not withdrawn yet", t.muted),
        ]));
    }
    lines
}
