//! Send: to whom, how much, and a word with it. Then the fee, then the txid.
//! Also the faucet request, which is the same conversation in one line.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::inset;
use crate::app::{FaucetStage, SendField, SendForm, SendStage, State};
use crate::format;
use crate::theme::{self, Theme};

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme, form: &SendForm) {
    let area = inset(area);
    let editing = form.stage == SendStage::Edit;
    let mut lines = vec![
        Line::styled("Send", t.title),
        Line::from(vec![
            Span::styled("spendable ", t.muted),
            Span::styled(
                format!("{} ZEC", format::zec(state.balance.total())),
                t.text,
            ),
        ]),
        Line::from(""),
    ];
    for field in [SendField::To, SendField::Amount, SendField::Memo] {
        let active = editing && form.field == field;
        let label = Span::styled(
            format!("{:<8}", field.label()),
            if active { t.accent } else { t.muted },
        );
        let value = form.value(field);
        let mut spans = vec![label];
        if value.is_empty() && !active {
            spans.push(Span::styled(
                match field {
                    SendField::To => "a unified, sapling or transparent address",
                    SendField::Amount => "in ZEC",
                    SendField::Memo => "optional, shielded addresses only",
                },
                t.muted.add_modifier(Modifier::ITALIC),
            ));
        } else {
            spans.push(Span::styled(value.to_string(), t.text));
        }
        if active {
            spans.push(Span::styled("▏", t.accent));
        }
        if field == SendField::Amount && !value.is_empty() {
            spans.push(Span::styled("  ZEC", t.muted));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    match &form.stage {
        SendStage::Edit => {
            if let Some(error) = &form.error {
                lines.push(Line::styled(error.clone(), t.negative));
            }
        }
        SendStage::Proposing => lines.push(Line::styled(
            format!(
                "{} choosing notes and working out the fee",
                theme::spinner(state.elapsed_secs())
            ),
            t.info,
        )),
        SendStage::Confirm { fee, total } => {
            lines.push(Line::from(vec![
                Span::styled("fee     ", t.muted),
                Span::styled(format!("{} ZEC", format::zec_exact(*fee)), t.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("total   ", t.muted),
                Span::styled(format!("{} ZEC", format::zec_exact(*total)), t.title),
                Span::styled("  leaves the wallet", t.muted),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::styled("y to send it, n to go back", t.warning));
        }
        SendStage::Sending => lines.push(Line::styled(
            format!(
                "{} building and transmitting",
                theme::spinner(state.elapsed_secs())
            ),
            t.info,
        )),
        SendStage::Sent { txids } => {
            lines.push(Line::styled(
                "sent",
                t.positive.add_modifier(Modifier::BOLD),
            ));
            for txid in txids {
                lines.push(Line::styled(txid.clone(), t.info));
            }
            lines.push(Line::styled(
                "it shows as pending until a block carries it",
                t.muted,
            ));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

pub fn draw_faucet(frame: &mut Frame, area: Rect, state: &State, t: &Theme, stage: &FaucetStage) {
    let area = inset(area);
    let mut lines = vec![Line::styled("Faucet", t.title)];
    match stage {
        FaucetStage::Ask { address } => {
            lines.push(Line::styled(
                "ask the chain's faucet for test funds, paid to this address:",
                t.muted,
            ));
            lines.push(Line::from(""));
            lines.push(Line::styled(address.clone(), t.info));
            lines.push(Line::from(""));
            lines.push(Line::styled("y to ask, n to go back", t.warning));
        }
        FaucetStage::Requesting { address } => {
            lines.push(Line::styled(address.clone(), t.info));
            lines.push(Line::from(""));
            lines.push(Line::styled(
                format!("{} asking the faucet", theme::spinner(state.elapsed_secs())),
                t.info,
            ));
        }
        FaucetStage::Done { amount } => {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("the faucet sent ", t.text),
                Span::styled(
                    format!("{} ZEC", format::zec(*amount)),
                    t.positive.add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::styled(
                "it shows as pending until a block carries it",
                t.muted,
            ));
        }
        FaucetStage::Failed(error) => {
            lines.push(Line::from(""));
            lines.push(Line::styled("the faucet did not pay", t.negative));
            lines.push(Line::styled(error.clone(), t.muted));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}
