//! The help modal: every key, in a box over the current screen.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::modal;
use crate::app::{State, WalletKind};
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let key = |k: &str, what: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<14}"), t.text),
            Span::styled(what.to_string(), t.muted),
        ])
    };
    let mut lines = vec![
        Line::styled("delegator-tui — keys", t.title),
        Line::from(""),
        key(
            "h t b r a s",
            "home, transactions, bonds, receive, addresses, sync",
        ),
        key("j / k · ↑ / ↓", "move; g G, home, end jump"),
        key("enter", "open the highlighted row"),
        key("esc", "close the current view, or go home"),
        key("v", "transaction: inputs/outputs ⇄ wallet events"),
        key("", "receive: unified ⇄ transparent address"),
        key("n", "make a new address"),
        key("p", "pay: send ZEC to an address, with a memo"),
        key("f", "ask the chain's faucet for test funds"),
        key("y", "copy the txid, address or finalizer address"),
        key("space", "sync: pause or resume"),
        key("m", "sync: chain strip ⇄ chain map"),
        key("R", "sync: rescan from the birthday"),
        key("c", "colour theme, previewed live"),
        key("?", "this help"),
        key("q / ctrl-c", "quit, saving the wallet first"),
        Line::from(""),
    ];
    let kind = state.wallet.as_ref().map(|w| w.kind);
    lines.push(Line::styled(
        match kind {
            Some(WalletKind::Seed) => "  this wallet holds a seed phrase · it can send",
            _ => "  this wallet holds a viewing key only · it cannot send",
        },
        t.muted,
    ));
    lines.push(Line::styled("  any key to close", t.muted));
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX) + 2;
    let inner = modal(frame, area, t, "help", 68, height);
    frame.render_widget(Paragraph::new(lines), inner);
}
