use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};

use super::diagram::{Flow, Part, Piece};
use super::{inset, kv, kv_styled, note, tabs};
use crate::app::{DetailMode, OutputRole, OutputRow, SpendState, State, TxKind, TxRow, TxStatus};
use crate::format;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    if state.txs.is_empty() {
        let msg = if state.sync.sessions_done == 0 {
            "No transactions yet. The first sync is still running."
        } else {
            "No transactions yet."
        };
        note(frame, area, msg, t);
        return;
    }
    draw_list(frame, inset(area), state, t, true);
}

/// The transaction list shared by Home and Transactions: when, how much, what kind of
/// transaction it was, its memo, and a flag only if it is not settled. The kind and the memo
/// have a column each, so one never stands in for the other. `with_time` adds the time of day.
pub fn draw_list(frame: &mut Frame, area: Rect, state: &State, t: &Theme, with_time: bool) {
    let kind_width = state
        .txs
        .iter()
        .map(|tx| tx.kind.long().chars().count())
        .max()
        .unwrap_or(0) as u16;
    let rows = state.txs.iter().map(|tx| {
        let memo = tx
            .first_memo()
            .map(|memo| memo.replace('\n', " "))
            .unwrap_or_default();
        let when = if with_time {
            format::datetime(tx.datetime)
        } else {
            format::date(tx.datetime)
        };
        Row::new(vec![
            Cell::from(Span::styled(when, t.muted)),
            Cell::from(
                Line::from(Span::styled(amount(tx), kind_style(tx.kind, t)))
                    .alignment(Alignment::Right),
            ),
            Cell::from(Span::styled(tx.kind.long(), t.muted)),
            Cell::from(Span::styled(memo, t.text.add_modifier(Modifier::ITALIC))),
            Cell::from(
                Line::from(Span::styled(flag(tx), status_style(tx.status, t)))
                    .alignment(Alignment::Right),
            ),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(if with_time { 16 } else { 10 }),
            Constraint::Length(14),
            Constraint::Length(kind_width),
            Constraint::Fill(1),
            Constraint::Length(7),
        ],
    )
    .column_spacing(2)
    .row_highlight_style(t.selection);
    let mut ts = TableState::default().with_selected(Some(state.tx_selected));
    frame.render_stateful_widget(table, area, &mut ts);
}

fn amount(tx: &TxRow) -> String {
    format!("{}{}", tx.kind.sign(), format::zec(tx.value))
}

/// Settled transactions need no label; only the exceptions do.
fn flag(tx: &TxRow) -> &'static str {
    match tx.status {
        TxStatus::Confirmed(_) => "",
        TxStatus::Pending => "pending",
        TxStatus::Failed => "failed",
    }
}

/// Incoming green, outgoing red, moves within the wallet in the info colour.
fn kind_style(kind: TxKind, t: &Theme) -> Style {
    match kind {
        TxKind::Received => t.positive,
        TxKind::Sent => t.negative,
        TxKind::SentToSelf
        | TxKind::Shield
        | TxKind::CreateBond
        | TxKind::BeginUnbond
        | TxKind::WithdrawBond
        | TxKind::RetargetBond => t.info,
    }
}

/// Settled transactions recede; pending and failed ones stand out.
fn status_style(status: TxStatus, t: &Theme) -> Style {
    match status {
        TxStatus::Confirmed(_) => t.muted,
        TxStatus::Pending => t.warning,
        TxStatus::Failed => t.negative,
    }
}

fn event_style(kind: &str, t: &Theme) -> Style {
    match kind {
        "received" => t.positive,
        "sent" => t.negative,
        _ => t.info,
    }
}

pub fn draw_detail(frame: &mut Frame, area: Rect, state: &State, t: &Theme, mode: DetailMode) {
    let Some(tx) = state.selected_tx() else {
        note(frame, area, "No transaction selected.", t);
        return;
    };
    let area = inset(area);
    let width = usize::from(area.width);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{:<14}", "amount"), t.muted),
            Span::styled(
                format!("{} ZEC", signed_full(tx)),
                kind_style(tx.kind, t).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}", tx.kind.long()), t.muted),
        ]),
        kv_styled(
            t,
            "status",
            match tx.status {
                TxStatus::Confirmed(h) => {
                    format!("confirmed in block {}", format::group(u64::from(h)))
                }
                TxStatus::Pending => "pending".into(),
                TxStatus::Failed => "failed".into(),
            },
            match tx.status {
                TxStatus::Confirmed(_) => t.text,
                other => status_style(other, t),
            },
        ),
        kv(t, "time", format::datetime(tx.datetime)),
    ];
    if let Some(fee) = tx.fee {
        lines.push(kv(t, "fee", format!("{} ZEC", format::zec_full(fee))));
    }
    lines.push(kv_styled(t, "txid", tx.txid.clone(), t.info));
    lines.push(Line::from(""));
    lines.push(tabs(
        &["Inputs and outputs", "Value transfers"],
        match mode {
            DetailMode::InputsOutputs => 0,
            DetailMode::ValueTransfers => 1,
        },
        t,
        false,
    ));
    lines.push(Line::from(""));

    match mode {
        DetailMode::InputsOutputs => {
            lines.extend(inputs_outputs_flow(tx, width, t));
            lines.push(Line::from(""));

            lines.push(Line::styled("Spent from this wallet", t.muted));
            if tx.inputs.is_empty() {
                lines.push(Line::styled("  nothing", t.muted));
            }
            for i in &tx.inputs {
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(format!("{:<12}", i.pool.name()), t.pool(i.pool)),
                    Span::styled(format!("{:>20} ZEC", format::zec_full(i.value)), t.text),
                    Span::styled("   from ", t.muted),
                    Span::styled(
                        format!(
                            "{}:{}",
                            format::truncate_middle(&i.source_txid, 18),
                            i.output_index
                        ),
                        t.info,
                    ),
                ];
                if i.pending {
                    spans.push(Span::styled("   spend pending", t.warning));
                }
                lines.push(Line::from(spans));
                push_memo(
                    &mut lines,
                    i.memo.as_deref(),
                    width,
                    t,
                    t.text.add_modifier(Modifier::ITALIC),
                );
            }
            lines.push(Line::from(""));

            let with_role = |role: OutputRole| -> Vec<&OutputRow> {
                tx.outputs.iter().filter(|o| o.role == role).collect()
            };
            let memo_style = t.text.add_modifier(Modifier::ITALIC);
            let mut first = true;
            let mut section = |lines: &mut Vec<Line<'static>>, title: &'static str| {
                if !first {
                    lines.push(Line::from(""));
                }
                first = false;
                lines.push(Line::styled(title, t.muted));
            };

            let received = with_role(OutputRole::Received);
            if !received.is_empty() {
                section(&mut lines, "Received by this wallet");
                for o in received {
                    lines.push(output_line(o, t, false));
                    let spend = match &o.spend {
                        SpendState::Unspent => t.positive,
                        SpendState::Spent(_) => t.muted,
                        SpendState::PendingSpent(_) => t.warning,
                    };
                    lines.push(Line::styled(format!("      {}", o.spend.describe()), spend));
                    push_memo(&mut lines, o.memo.as_deref(), width, t, memo_style);
                }
            }

            let sent = with_role(OutputRole::Sent);
            if !sent.is_empty() {
                section(&mut lines, "Sent to others");
                for o in sent {
                    lines.push(output_line(o, t, false));
                    if let Some(address) = &o.address {
                        for l in format::wrap_chars(address, width.saturating_sub(6)) {
                            lines.push(Line::styled(format!("      {l}"), t.info));
                        }
                    }
                    push_memo(&mut lines, o.memo.as_deref(), width, t, memo_style);
                }
            }

            let to_self = with_role(OutputRole::ToSelf);
            if !to_self.is_empty() {
                section(&mut lines, "Sent to this wallet");
                for o in to_self {
                    lines.push(output_line(o, t, false));
                    lines.push(Line::styled(
                        format!("      {}", o.spend.describe()),
                        t.muted,
                    ));
                    push_memo(&mut lines, o.memo.as_deref(), width, t, memo_style);
                }
            }

            // change is the wallet paying itself back: listed last, and quiet
            let change = with_role(OutputRole::Change);
            if !change.is_empty() {
                section(&mut lines, "Change");
                for o in change {
                    lines.push(output_line(o, t, true));
                    lines.push(Line::styled(
                        format!("      {}", o.spend.describe()),
                        t.muted,
                    ));
                    push_memo(
                        &mut lines,
                        o.memo.as_deref(),
                        width,
                        t,
                        t.muted.add_modifier(Modifier::ITALIC),
                    );
                }
            }
        }
        DetailMode::ValueTransfers => {
            lines.extend(value_transfer_flow(tx, width, t));
            lines.push(Line::from(""));
            if tx.value_transfers.is_empty() {
                lines.push(Line::styled("no value transfers", t.muted));
            }
            for event in &tx.value_transfers {
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(format!("{:<14}", event.kind), event_style(&event.kind, t)),
                    Span::styled(format!("{:>20} ZEC", format::zec_full(event.value)), t.text),
                ];
                if let Some(pool) = &event.pool_received {
                    spans.push(Span::styled(format!("   to {pool}"), t.muted));
                }
                lines.push(Line::from(spans));
                if let Some(addr) = &event.recipient {
                    for l in format::wrap_chars(addr, width.saturating_sub(6)) {
                        lines.push(Line::styled(format!("      {l}"), t.info));
                    }
                }
                for memo in &event.memos {
                    push_memo(
                        &mut lines,
                        Some(memo),
                        width,
                        t,
                        t.text.add_modifier(Modifier::ITALIC),
                    );
                }
                lines.push(Line::from(""));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((state.detail_scroll, 0)),
        area,
    );
}

/// The amount with its sign, at full precision.
fn signed_full(tx: &TxRow) -> String {
    format!("{}{}", tx.kind.sign(), format::zec_full(tx.value))
}

/// The two lines inside the box: what the transaction did, and what it cost.
fn box_lines(tx: &TxRow) -> Vec<String> {
    let mut lines = vec![tx.kind.long().to_string()];
    if let Some(fee) = tx.fee {
        lines.push(format!("fee {}", format::zec(fee)));
    }
    lines
}

/// Width of the widest entry, for sizing a column to its content.
fn widest(entries: impl Iterator<Item = String>) -> usize {
    entries.map(|e| e.chars().count()).max().unwrap_or(0)
}

/// How an output is labelled in the diagram. An output that stays in this wallet is never
/// called sent.
fn output_tag(o: &OutputRow) -> &'static str {
    match o.role {
        OutputRole::Received => "received",
        OutputRole::Sent => "sent",
        OutputRole::ToSelf => "self",
        OutputRole::Change => "change",
    }
}

/// Diagram order: payments out first, then what stayed in the wallet, change last.
fn rank(o: &OutputRow) -> u8 {
    match o.role {
        OutputRole::Sent => 0,
        OutputRole::ToSelf => 1,
        OutputRole::Received => 2,
        OutputRole::Change => 3,
    }
}

/// Turns diagram pieces into styled lines. Each label takes the style its caller gave it.
fn paint(rows: Vec<Vec<Piece>>, t: &Theme, left: &[Style], right: &[Style]) -> Vec<Line<'static>> {
    rows.into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|piece| {
                        let style = match piece.part {
                            Part::Pad => Style::new(),
                            Part::Wire => t.border,
                            Part::Frame => t.accent,
                            Part::Title(0) => t.title,
                            Part::Title(_) => t.muted,
                            Part::Left(i) => left.get(i).copied().unwrap_or(t.text),
                            Part::Right(i) => right.get(i).copied().unwrap_or(t.text),
                            Part::Note => t.muted,
                        };
                        Span::styled(piece.text, style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// Spent wallet outputs on the left, every output of the transaction on the right, each line
/// in its pool's colour. Change comes last and stays muted.
fn inputs_outputs_flow(tx: &TxRow, width: usize, t: &Theme) -> Vec<Line<'static>> {
    // columns inside the labels are as wide as their widest entry, and the amount always sits
    // next to the wire
    let pool_w = widest(tx.inputs.iter().map(|i| i.pool.name().to_string()));
    let value_w = widest(tx.inputs.iter().map(|i| format::zec(i.value)));
    let left: Vec<String> = tx
        .inputs
        .iter()
        .map(|i| {
            format!(
                "{:<pool_w$}  {:>value_w$}",
                i.pool.name(),
                format::zec(i.value)
            )
        })
        .collect();
    let mut outputs: Vec<&OutputRow> = tx.outputs.iter().collect();
    outputs.sort_by_key(|o| rank(o));
    let pool_w = widest(outputs.iter().map(|o| o.pool.name().to_string()));
    let value_w = widest(outputs.iter().map(|o| format::zec(o.value)));
    let right: Vec<String> = outputs
        .iter()
        .map(|o| {
            format!(
                "{:<value_w$}  {:<pool_w$}  {}",
                format::zec(o.value),
                o.pool.name(),
                output_tag(o)
            )
        })
        .collect();
    let left_styles: Vec<Style> = tx.inputs.iter().map(|i| t.pool(i.pool)).collect();
    let right_styles: Vec<Style> = outputs
        .iter()
        .map(|o| {
            if o.role == OutputRole::Change {
                t.muted
            } else {
                t.pool(o.pool)
            }
        })
        .collect();
    let box_lines = box_lines(tx);
    let rows = Flow {
        left: &left,
        right: &right,
        box_lines: &box_lines,
        left_empty: "no wallet inputs",
        right_empty: "no outputs",
    }
    .render_parts(width);
    paint(rows, t, &left_styles, &right_styles)
}

/// Value coming in on the left, value going out on the right.
fn value_transfer_flow(tx: &TxRow, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let (incoming, outgoing): (Vec<_>, Vec<_>) = tx
        .value_transfers
        .iter()
        .partition(|event| event.kind.eq_ignore_ascii_case("received"));
    let kind_w = widest(incoming.iter().map(|event| event.kind.clone()));
    let value_w = widest(incoming.iter().map(|event| format::zec(event.value)));
    let left: Vec<String> = incoming
        .iter()
        .map(|event| {
            format!(
                "{:<kind_w$}  {:>value_w$}",
                event.kind,
                format::zec(event.value)
            )
        })
        .collect();
    let value_w = widest(outgoing.iter().map(|event| format::zec(event.value)));
    let right: Vec<String> = outgoing
        .iter()
        .map(|event| format!("{:<value_w$}  {}", format::zec(event.value), event.kind))
        .collect();
    let left_styles: Vec<Style> = incoming
        .iter()
        .map(|event| event_style(&event.kind, t))
        .collect();
    let right_styles: Vec<Style> = outgoing
        .iter()
        .map(|event| event_style(&event.kind, t))
        .collect();
    let box_lines = box_lines(tx);
    let rows = Flow {
        left: &left,
        right: &right,
        box_lines: &box_lines,
        left_empty: "nothing received",
        right_empty: "nothing sent",
    }
    .render_parts(width);
    paint(rows, t, &left_styles, &right_styles)
}

/// Pool, value, index, and scope of one output. `quiet` mutes the whole line, for change,
/// whose section heading already says what it is.
fn output_line(o: &OutputRow, t: &Theme, quiet: bool) -> Line<'static> {
    let (pool, value) = if quiet {
        (t.muted, t.muted)
    } else {
        (t.pool(o.pool), t.text)
    };
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(format!("{:<12}", o.pool.name()), pool),
        Span::styled(format!("{:>20} ZEC", format::zec_full(o.value)), value),
        Span::styled(format!("   index {}", o.index), t.muted),
    ];
    if let Some(scope) = o.scope.as_ref().filter(|_| !quiet) {
        spans.push(Span::styled(
            format!("   {}", scope.to_lowercase()),
            t.muted,
        ));
    }
    Line::from(spans)
}

fn push_memo(
    lines: &mut Vec<Line<'static>>,
    memo: Option<&str>,
    width: usize,
    t: &Theme,
    style: Style,
) {
    let Some(memo) = memo.filter(|m| !m.is_empty()) else {
        return;
    };
    lines.push(Line::styled("      memo:", t.muted));
    for raw in memo.lines() {
        for l in format::wrap_chars(raw, width.saturating_sub(8)) {
            lines.push(Line::styled(format!("        {l}"), style));
        }
    }
}
