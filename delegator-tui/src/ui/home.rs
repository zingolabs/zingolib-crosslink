//! Home: how much is here, and what just happened.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{bignum, inset, transactions};
use crate::app::{Balance, Pool, State};
use crate::format;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let area = inset(area);
    let width = usize::from(area.width);
    let b = &state.balance;
    let amount = format::zec(b.total());

    let mut top = vec![Line::styled("Balance", t.muted)];
    let big = bignum::render(&amount).filter(|rows| rows[0].chars().count() + 5 <= width);
    let number_width = match big {
        Some(rows) => {
            let w = rows[0].chars().count() + 5;
            for (i, row) in rows.into_iter().enumerate() {
                let mut spans = vec![Span::styled(row, t.title)];
                // the unit sits on the baseline row
                if i == 2 {
                    spans.push(Span::styled("  ZEC", t.muted));
                }
                top.push(Line::from(spans));
            }
            w
        }
        None => {
            top.push(Line::styled(format!("{amount} ZEC"), t.title));
            amount.len() + 4
        }
    };
    if b.pending() > 0 {
        top.push(Line::styled(
            format!("{} ZEC pending", format::zec(b.pending())),
            t.warning,
        ));
    }
    if b.staked > 0 {
        top.push(Line::styled(
            format!("{} ZEC staked", format::zec(b.staked)),
            t.info,
        ));
    }
    if b.total() > 0 {
        // the bar spans the legend beneath it, so the two read as one
        let legend = pool_legend(b, t);
        let bar_width = legend.width().max(number_width).min(width);
        top.push(Line::from(""));
        top.push(pool_bar(b, bar_width, t));
        top.push(legend);
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
        let message = if state.sync.sessions_done == 0 {
            "No transactions yet. The first sync is still running."
        } else {
            "No transactions yet."
        };
        frame.render_widget(Paragraph::new(Line::styled(message, t.muted)), list_area);
    } else {
        transactions::draw_list(frame, list_area, state, t, false);
    }
}

/// A bar split by how much each pool holds, in the pools' colours.
fn pool_bar(b: &Balance, width: usize, t: &Theme) -> Line<'static> {
    Line::from(
        pool_split(b, width)
            .into_iter()
            .map(|(pool, cells)| Span::styled("━".repeat(cells), t.pool(pool)))
            .collect::<Vec<_>>(),
    )
}

/// Each pool that holds something, with its amount.
fn pool_legend(b: &Balance, t: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for (pool, balance) in b.pools().into_iter().filter(|(_, p)| p.total() > 0) {
        if !spans.is_empty() {
            spans.push(Span::raw("    "));
        }
        spans.push(Span::styled(pool.name(), t.pool(pool)));
        spans.push(Span::styled(
            format!(" {}", format::zec(balance.total())),
            t.text,
        ));
    }
    Line::from(spans)
}

/// Splits `width` cells between the pools that hold funds, by share. Every such pool gets at
/// least one cell, so dust still shows; the rest goes by largest remainder.
pub fn pool_split(b: &Balance, width: usize) -> Vec<(Pool, usize)> {
    let pools: Vec<(Pool, u64)> = b
        .pools()
        .into_iter()
        .map(|(pool, balance)| (pool, balance.total()))
        .filter(|(_, total)| *total > 0)
        .collect();
    let total: u64 = pools.iter().map(|(_, v)| v).sum();
    if total == 0 {
        return Vec::new();
    }
    let spare = width.max(pools.len()) - pools.len();
    let exact: Vec<f64> = pools
        .iter()
        .map(|(_, v)| *v as f64 / total as f64 * spare as f64)
        .collect();
    let mut cells: Vec<usize> = exact.iter().map(|e| e.floor() as usize).collect();
    let mut left = spare - cells.iter().sum::<usize>();
    let mut order: Vec<usize> = (0..pools.len()).collect();
    order.sort_by(|&a, &b| (exact[b] - exact[b].floor()).total_cmp(&(exact[a] - exact[a].floor())));
    for i in order {
        if left == 0 {
            break;
        }
        cells[i] += 1;
        left -= 1;
    }
    pools
        .into_iter()
        .zip(cells)
        .map(|((pool, _), c)| (pool, c + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PoolBalance;

    fn balance(orchard: u64, sapling: u64, transparent: u64) -> Balance {
        let pb = |confirmed| PoolBalance {
            confirmed,
            pending: 0,
        };
        Balance {
            orchard: pb(orchard),
            sapling: pb(sapling),
            transparent: pb(transparent),
            staked: 0,
        }
    }

    #[test]
    fn the_split_fills_the_width_exactly() {
        for width in [1, 7, 30, 61, 100] {
            let split = pool_split(&balance(150_000_000, 30_000, 25_000), width);
            assert_eq!(split.iter().map(|(_, c)| c).sum::<usize>(), width.max(3));
        }
    }

    #[test]
    fn dust_still_gets_a_cell_and_empty_pools_none() {
        let split = pool_split(&balance(150_000_000, 1, 0), 40);
        assert_eq!(split, vec![(Pool::Orchard, 39), (Pool::Sapling, 1)]);
        assert!(pool_split(&balance(0, 0, 0), 40).is_empty());
    }

    #[test]
    fn even_halves_split_evenly() {
        let split = pool_split(&balance(0, 50, 50), 20);
        assert_eq!(split, vec![(Pool::Sapling, 10), (Pool::Transparent, 10)]);
    }
}
