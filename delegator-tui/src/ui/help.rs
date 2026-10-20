use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::inset;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, t: &Theme) {
    let section = |title: &str| Line::styled(title.to_string(), t.title);
    let key = |k: &str, what: &str| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{k:<12}"), t.accent),
            Span::styled(what.to_string(), t.text),
        ])
    };
    let lines = vec![
        section("Everywhere"),
        key("h t r a s", "home, transactions, receive, addresses, sync"),
        key("c", "colour theme, previewed live"),
        key("Esc", "close the current view, or go home"),
        key("?", "this page"),
        key("q", "quit, saving the wallet first"),
        Line::from(""),
        section("Lists"),
        key("j k", "move, also the arrow keys"),
        key("g G", "first and last row, also Home and End"),
        key("Enter", "open the highlighted row"),
        Line::from(""),
        section("A transaction"),
        key(
            "v",
            "switch between inputs and outputs, and value transfers",
        ),
        key("y", "copy the txid"),
        Line::from(""),
        section("Receive and addresses"),
        key("n", "make a new address"),
        key(
            "v",
            "switch between the unified and the transparent address",
        ),
        key(
            "y",
            "copy the address to the clipboard through the terminal",
        ),
        Line::from(""),
        section("Sync"),
        key("p", "pause or resume"),
        key("m", "switch between the chain strip and the chain map"),
        key("R", "rescan from the birthday"),
        Line::from(""),
        Line::styled(
            "This wallet holds a viewing key only. It cannot spend.",
            t.muted,
        ),
    ];
    frame.render_widget(Paragraph::new(lines), inset(area));
}
