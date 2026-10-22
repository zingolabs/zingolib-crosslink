//! Staking: what can be done to a bond, and the form that turns a choice into a transaction.
//! A new bond takes an amount, then every action shows its fee, then its txid.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::bonds::finalizer;
use super::inset;
use crate::app::{BondAction, BondRow, StakeForm, StakeOp, StakeStage, StakingDay, State};
use crate::format;
use crate::theme::{self, Theme};

/// A label column the width of the longest label, then the value.
fn field(t: &Theme, label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{label:<11}"), t.muted)];
    spans.extend(value);
    Line::from(spans)
}

fn text(value: String, style: Style) -> Vec<Span<'static>> {
    vec![Span::styled(value, style)]
}

/// When the chain takes the bond's next timed action: `now`, or the block and the wait.
/// `None` before a tip is known.
pub fn next_action(state: &State, bond: &BondRow) -> Option<(String, bool)> {
    let tip = state.sync.tip?;
    let at = bond.next_action_at(tip);
    Some(if at <= tip + 1 {
        ("now".into(), true)
    } else {
        let wait = u64::from(at - tip - 1) * state.spacing_secs();
        (
            format!(
                "from block {} · in ≈ {}",
                format::group(u64::from(at)),
                format::duration(wait)
            ),
            false,
        )
    })
}

/// The bond's facts, shared by the menu and the form.
fn bond_lines(t: &Theme, bond: &BondRow, lines: &mut Vec<Line<'static>>) {
    lines.push(field(t, "finalizer", finalizer(&bond.finalizer, t).spans));
    let mut amount = text(format!("{} ZEC", format::zec(bond.amount)), t.text);
    if bond.earned() > 0 {
        amount.push(Span::styled(
            format!(
                " · {} bonded + {} earned",
                format::zec(bond.initial),
                format::zec(bond.earned())
            ),
            t.muted,
        ));
    }
    lines.push(field(t, "amount", amount));
    lines.push(field(
        t,
        "status",
        text(
            bond.status.name().into(),
            match bond.status {
                crate::app::BondStatus::Bonded => t.info,
                crate::app::BondStatus::Unbonding => t.negative,
            },
        ),
    ));
    lines.push(field(
        t,
        "since",
        text(
            format!("block {}", format::group(u64::from(bond.since))),
            t.muted,
        ),
    ));
}

/// The amount between its neighbours on the ladder of powers of ten, so the choice reads as
/// a step: `0.1  ‹ 1 ZEC ›  10`. The ends of the ladder lose their arrow.
fn ladder(t: &Theme, form: &StakeForm, editing: bool) -> Vec<Span<'static>> {
    let amount = Span::styled(
        format!("{} ZEC", format::zec(form.amount_zats())),
        t.title.add_modifier(Modifier::BOLD),
    );
    if !editing {
        return vec![amount];
    }
    let mut spans = Vec::new();
    if form.power > 0 {
        spans.push(Span::styled(
            format!("{}  ", format::zec(StakeForm::zats_at(form.power - 1))),
            t.muted,
        ));
        spans.push(Span::styled("‹ ", t.accent));
    } else {
        spans.push(Span::raw("  "));
    }
    spans.push(amount);
    if form.power < StakeForm::MAX_POWER {
        spans.push(Span::styled(" ›", t.accent));
        spans.push(Span::styled(
            format!("  {}", format::zec(StakeForm::zats_at(form.power + 1))),
            t.muted,
        ));
    }
    spans
}

/// The selected bond and its actions, the highlighted one under a caret.
pub fn draw_menu(frame: &mut Frame, area: Rect, state: &State, t: &Theme, selected: usize) {
    let area = inset(area);
    let Some(bond) = state.selected_bond() else {
        return;
    };
    let mut lines = vec![Line::styled("Bond", t.title), Line::from("")];
    bond_lines(t, bond, &mut lines);
    lines.push(Line::from(""));
    for (i, action) in BondAction::for_status(bond.status).iter().enumerate() {
        let chosen = i == selected;
        // a redelegation is never held back; the others wait for the calendar, and say so
        // in place of what they do
        let wait = (*action != BondAction::Redelegate)
            .then(|| next_action(state, bond))
            .flatten()
            .filter(|(_, now)| !now);
        let spans = vec![
            if chosen {
                Span::styled("> ", t.accent)
            } else {
                Span::raw("  ")
            },
            Span::styled(
                format!("{:<12}", action.label()),
                if chosen { t.title } else { t.text },
            ),
            Span::styled(format!("{}  ", action.key()), t.accent),
            match wait {
                Some((when, _)) => Span::styled(when, t.warning),
                None => Span::styled(action.detail(), t.muted),
            },
        ];
        lines.push(Line::from(spans));
    }
    if !state.can_spend() {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "this wallet holds a viewing key only · it cannot stake",
            t.muted,
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// The staking form, at whatever stage it has reached.
pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme, form: &StakeForm) {
    let area = inset(area);
    let title = match &form.op {
        StakeOp::Stake { .. } => "New bond",
        StakeOp::Unbond { .. } => "Unbond",
        StakeOp::Redelegate { .. } => "Redelegate",
        StakeOp::Withdraw { .. } => "Withdraw",
    };
    let mut lines = vec![Line::styled(title, t.title), Line::from("")];
    let editing = form.stage == StakeStage::Edit;
    match &form.op {
        StakeOp::Stake { finalizer: to } => {
            lines.push(field(t, "finalizer", finalizer(to, t).spans));
            if let Some(row) = state.roster.iter().find(|r| &r.finalizer == to) {
                lines.push(field(
                    t,
                    "",
                    text(
                        format!("{} ZEC at stake with it already", format::zec(row.stake)),
                        t.muted,
                    ),
                ));
            }
            lines.push(field(
                t,
                "spendable",
                text(
                    format!("{} ZEC", format::zec(state.balance.total())),
                    t.text,
                ),
            ));
            lines.push(Line::from(
                [
                    vec![Span::styled(
                        format!("{:<11}", "amount"),
                        if editing { t.accent } else { t.muted },
                    )],
                    ladder(t, form, editing),
                ]
                .concat(),
            ));
            if form.amount_zats() > state.balance.total() {
                lines.push(field(
                    t,
                    "",
                    text("more than the spendable balance".into(), t.negative),
                ));
            }
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "a bond is a power of ten: … 0.1, 1, 10, 100, 1,000 ZEC … · ↑ ↓ choose",
                t.muted,
            ));
            lines.push(Line::styled(
                "the funds stay locked until the bond is unbonded and withdrawn",
                t.muted,
            ));
        }
        StakeOp::Unbond { bond } => {
            bond_lines(t, bond, &mut lines);
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "the bond stops earning once the unbond is mined",
                t.muted,
            ));
            if let Some(tip) = state.sync.tip {
                let at = StakingDay::next_open(tip + 1 + StakingDay::ACTION_DELAY);
                lines.push(Line::styled(
                    format!(
                        "its funds can be withdrawn from about block {} · in ≈ {}",
                        format::group(u64::from(at)),
                        format::duration(u64::from(at - tip) * state.spacing_secs())
                    ),
                    t.muted,
                ));
            }
        }
        StakeOp::Redelegate { bond, to } => {
            lines.push(field(t, "from", finalizer(&bond.finalizer, t).spans));
            lines.push(field(t, "to", finalizer(to, t).spans));
            lines.push(field(
                t,
                "amount",
                text(format!("{} ZEC", format::zec(bond.amount)), t.text),
            ));
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "the stake moves and keeps earning · no unbonding, no staking day needed",
                t.muted,
            ));
        }
        StakeOp::Withdraw { bond } => {
            bond_lines(t, bond, &mut lines);
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "the bond's funds come back to the ironwood pool",
                t.muted,
            ));
            lines.push(Line::styled(
                "the fee is paid from spendable funds and is set as the transaction is built",
                t.muted,
            ));
        }
    }
    lines.push(Line::from(""));
    let verb = form.op.verb();
    match &form.stage {
        StakeStage::Edit => {
            if let Some(error) = &form.error {
                lines.push(Line::styled(error.clone(), t.negative));
            }
        }
        StakeStage::Proposing => lines.push(Line::styled(
            format!(
                "{} checking the action and working out the fee",
                theme::spinner(state.elapsed_secs())
            ),
            t.info,
        )),
        StakeStage::Confirm { fee } => {
            if let Some(fee) = fee {
                lines.push(field(
                    t,
                    "fee",
                    text(format!("{} ZEC", format::zec_exact(*fee)), t.text),
                ));
                if let StakeOp::Stake { .. } = &form.op {
                    let amount = form.amount_zats();
                    lines.push(Line::from(vec![
                        Span::styled(format!("{:<11}", "total"), t.muted),
                        Span::styled(
                            format!("{} ZEC", format::zec_exact(amount.saturating_add(*fee))),
                            t.title,
                        ),
                        Span::styled("  leaves the spendable balance", t.muted),
                    ]));
                }
                lines.push(Line::from(""));
            }
            lines.push(Line::styled(
                format!("y to {verb}, n to go back"),
                t.warning,
            ));
        }
        StakeStage::Sending => lines.push(Line::styled(
            format!(
                "{} building and transmitting",
                theme::spinner(state.elapsed_secs())
            ),
            t.info,
        )),
        StakeStage::Sent { txids } => {
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
        StakeStage::Failed(error) => {
            lines.push(Line::styled(
                format!("the wallet did not {verb}"),
                t.negative,
            ));
            lines.push(Line::styled(error.clone(), t.muted));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}
