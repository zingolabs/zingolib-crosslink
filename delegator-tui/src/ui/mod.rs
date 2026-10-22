//! Rendering. Every function here reads the state and draws; none of them change it.
//!
//! The chrome follows lwtui, the lightwallet-tools explorer: a one-line header with the title,
//! a status mark and a breadcrumb; panes with a rule on their left and a title on it; a gold
//! caret on the selected row; dim key hints on the last line.

mod addresses;
mod bignum;
mod bonds;
mod diagram;
mod dither;
mod help;
mod home;
mod jacquard;
mod onboarding;
mod receive;
mod send;
mod sigil;
mod stake;
mod sync;
mod themes;
mod transactions;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::app::{
    BondAction, FaucetStage, Network, Overlay, Phase, Screen, SendStage, StakeOp, StakeStage,
    State, SyncMode,
};
use crate::format;
use crate::theme::{self, Theme};

/// The brand on the header line.
const TITLE: &str = "delegator-tui";

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

pub fn view(frame: &mut Frame, state: &State) {
    let t = Theme::new(state.theme, state.color_depth);
    let area = frame.area();
    frame.render_widget(Block::new().style(t.base), area);
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let text = vec![
            Line::styled("terminal too small", t.title),
            Line::styled(
                format!(
                    "delegator-tui needs at least {MIN_WIDTH}x{MIN_HEIGHT}, this one is {}x{}",
                    area.width, area.height
                ),
                t.muted,
            ),
        ];
        centered(frame, area, text);
        return;
    }
    match &state.phase {
        Phase::Onboarding(o) => onboarding::draw(frame, area, state, &t, o),
        Phase::Loading(label) => {
            let [header_area, body, _] = split(area);
            header(
                frame,
                header_area,
                state,
                &t,
                vec![Span::styled(
                    format!("{} {}", theme::spinner(state.elapsed_secs()), label),
                    t.muted,
                )],
                None,
            );
            centered(
                frame,
                body,
                vec![
                    Line::styled(label.clone(), t.text),
                    Line::styled("ctrl-c aborts", t.muted),
                ],
            );
        }
        Phase::Fatal(message) => centered(
            frame,
            area,
            vec![
                Line::styled("cannot continue", t.negative.add_modifier(Modifier::BOLD)),
                Line::from(""),
                Line::styled(message.as_str(), t.text),
                Line::from(""),
                Line::styled("press q to exit", t.muted),
            ],
        ),
        Phase::Main | Phase::ShuttingDown => draw_main(frame, area, state, &t),
    }
}

/// Header, body, footer: one line, the rest, one line.
pub fn split(area: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area)
}

/// A muted message at the body's content edge, for empty states. Everything in the body
/// starts at the same left edge; only whole-screen states are centred.
pub fn note(frame: &mut Frame, area: Rect, message: &str, t: &Theme) {
    frame.render_widget(
        Paragraph::new(Line::styled(message.to_string(), t.muted)).wrap(Wrap { trim: false }),
        inset(area),
    );
}

/// Draws lines centered in `area`, wrapping long ones. For whole-screen states only: loading,
/// a fatal error, a terminal that is too small.
pub fn centered(frame: &mut Frame, area: Rect, lines: Vec<Line<'_>>) {
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(area.height);
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        middle,
    );
}

/// A rule across `area`, one row high.
pub fn separator(frame: &mut Frame, area: Rect, t: &Theme) {
    frame.render_widget(
        Paragraph::new(Line::styled("─".repeat(usize::from(area.width)), t.border)),
        area,
    );
}

/// The header line: the shimmering brand and the navigation on the left, anything in
/// `status` right-aligned at the opposite edge, so state and navigation never share a
/// corner.
pub fn header(
    frame: &mut Frame,
    area: Rect,
    state: &State,
    t: &Theme,
    tabs: Vec<Span<'static>>,
    status: Option<Vec<Span<'static>>>,
) {
    let mut left = theme::shimmer(t, TITLE, state.elapsed_secs(), state.color_depth);
    left.push(Span::raw(" "));
    left.extend(tabs);
    let width = |spans: &[Span<'static>]| spans.iter().map(Span::width).sum::<usize>();
    let gap = area
        .width
        .saturating_sub((width(&left) + status.as_ref().map(|s| width(s)).unwrap_or(0)) as u16);
    left.push(Span::raw(" ".repeat(usize::from(gap))));
    if let Some(status) = status {
        left.extend(status);
    }
    frame.render_widget(Paragraph::new(Line::from(left)), area);
}

/// A pane: a rule down its left edge with the title on the first row. Returns the content area,
/// which starts one row down and two columns in, the same place [`inset`] puts body content.
pub fn pane(frame: &mut Frame, area: Rect, t: &Theme, title: &str) -> Rect {
    for y in area.y..area.y + area.height {
        frame.render_widget(
            Paragraph::new(Line::styled("▏", t.border)),
            Rect {
                x: area.x,
                y,
                width: 1,
                height: 1,
            },
        );
    }
    frame.render_widget(
        Paragraph::new(Line::styled(title.to_string(), t.info)),
        Rect {
            x: area.x + 1,
            width: area.width.saturating_sub(1),
            height: 1,
            ..area
        },
    );
    inset(area)
}

/// The last line: a notice when there is one, else the key hints, all quiet.
pub fn footer(frame: &mut Frame, area: Rect, state: &State, t: &Theme, keys: &str) {
    let line = match state.notice_text() {
        Some(notice) => Line::styled(notice.to_string(), t.warning),
        None => Line::styled(keys.to_string(), t.muted),
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// A bordered box centred in `area`, cleared, with `title` on its top edge. Returns the inside.
pub fn modal(
    frame: &mut Frame,
    area: Rect,
    t: &Theme,
    title: &str,
    width: u16,
    height: u16,
) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_style(t.border)
        .title(Span::styled(title.to_string(), t.text))
        .style(t.base);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

fn draw_main(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let [header_area, body, footer_area] = split(area);
    let mut status = vec![sync_indicator(state, t)];
    if let Some(w) = &state.wallet {
        status.push(Span::styled(" · ", t.muted));
        status.push(Span::styled(w.wallet_name.clone(), t.text));
        status.push(Span::styled(
            format!(" {}", w.network.name()),
            if w.network == Network::Stable {
                t.muted
            } else {
                t.warning
            },
        ));
    }
    let status_width: usize = status.iter().map(Span::width).sum();
    let room = usize::from(header_area.width).saturating_sub(TITLE.len() + 2 + status_width);
    header(
        frame,
        header_area,
        state,
        t,
        screen_tabs(state, t, room),
        Some(status),
    );
    let title = match &state.overlay {
        Overlay::Help | Overlay::None => state.screen.title().to_lowercase(),
        Overlay::TxDetail { .. } => "transaction".into(),
        Overlay::AddressQr => "address".into(),
        Overlay::RescanConfirm => "rescan".into(),
        Overlay::NewUnifiedAddress { .. } => "new address".into(),
        Overlay::ThemePicker { .. } => "theme".into(),
        Overlay::Faucet(_) => "faucet".into(),
        Overlay::BondMenu { .. } => "bond".into(),
        Overlay::Stake(form) => form.op.verb().into(),
    };
    pane(frame, body, t, &title);
    match &state.overlay {
        Overlay::TxDetail { mode } => transactions::draw_detail(frame, body, state, t, *mode),
        Overlay::AddressQr => addresses::draw_qr(frame, body, state, t),
        Overlay::RescanConfirm => sync::draw_rescan_confirm(frame, body, state, t),
        Overlay::NewUnifiedAddress { orchard_only } => {
            receive::draw_new_address(frame, body, t, *orchard_only)
        }
        Overlay::ThemePicker { original } => themes::draw(frame, body, state, t, *original),
        Overlay::Faucet(stage) => send::draw_faucet(frame, body, state, t, stage),
        Overlay::BondMenu { selected } => stake::draw_menu(frame, body, state, t, *selected),
        Overlay::Stake(form) => stake::draw(frame, body, state, t, form),
        Overlay::Help | Overlay::None => match state.screen {
            Screen::Home => home::draw(frame, body, state, t),
            Screen::Pay => send::draw(frame, body, state, t, &state.send),
            Screen::Transactions => transactions::draw(frame, body, state, t),
            Screen::Bonds => bonds::draw(frame, body, state, t),
            Screen::Finalizers => bonds::draw_finalizers(frame, body, state, t),
            Screen::Receive => receive::draw(frame, body, state, t),
            Screen::Addresses => addresses::draw(frame, body, state, t),
            Screen::Sync => sync::draw(frame, body, state, t),
        },
    }
    if state.overlay == Overlay::None {
        dither::band(frame, body, state, t);
    }
    if state.overlay == Overlay::Help {
        help::draw(frame, body, state, t);
    }
    let keys = if state.phase == Phase::ShuttingDown {
        "saving the wallet · q again quits without waiting".to_string()
    } else {
        hints(state)
    };
    let chip = staking_day(state, t);
    let chip_width = chip
        .as_ref()
        .map(|line| u16::try_from(line.width()).unwrap_or(u16::MAX))
        .unwrap_or(0);
    let [hints_area, chip_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(if chip_width > 0 { chip_width + 2 } else { 0 }),
    ])
    .areas(footer_area);
    footer(frame, hints_area, state, t, &keys);
    if let Some(line) = chip {
        frame.render_widget(Paragraph::new(line), chip_area);
    }
}

fn screen_tabs(state: &State, t: &Theme, room: usize) -> Vec<Span<'static>> {
    let names: Vec<String> = Screen::ALL
        .iter()
        .map(|s| s.title().to_lowercase())
        .collect();
    let mut full = vec![false; names.len()];
    let mut used = names.len() * 2 - 1;
    let active = Screen::ALL.iter().position(|s| *s == state.screen);
    for i in active.into_iter().chain(0..names.len()) {
        let extra = names[i].chars().count() - 1;
        if !full[i] && used + extra <= room {
            full[i] = true;
            used += extra;
        }
    }
    let mut spans = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let mut chars = name.chars();
        let letter = chars.next().map(String::from).unwrap_or_default();
        let style = if Some(i) == active {
            t.accent.add_modifier(Modifier::BOLD)
        } else {
            t.text
        };
        spans.push(Span::styled(letter, style));
        if full[i] {
            spans.push(Span::styled(chars.as_str().to_string(), t.muted));
        }
    }
    spans
}

/// The staking calendar as one footer line, right of the hints: whether a staking day is
/// open, and how many blocks and how long until that changes. `None` before a tip is
/// known.
fn staking_day(state: &State, t: &Theme) -> Option<Line<'static>> {
    let day = state.staking_day()?;
    let secs = day.secs_left(state.spacing_secs());
    let (mark, word, style) = if day.open {
        ("✦", "staking day", t.positive.add_modifier(Modifier::BOLD))
    } else {
        ("✧", "no staking day", t.warning)
    };
    let what = if day.open { "closes" } else { "opens" };
    Some(Line::from(vec![
        Span::styled(format!("{mark} {word}"), style),
        Span::styled(
            format!(
                " · {what} in {} blocks ≈ {}",
                format::group(u64::from(day.blocks_left)),
                format::duration(secs)
            ),
            t.muted,
        ),
    ]))
}

/// The sync state as one mark and a word: what the user acts on.
fn sync_indicator(state: &State, t: &Theme) -> Span<'static> {
    let s = &state.sync;
    let style = sync_style(state, t);
    let text = match s.mode {
        SyncMode::Running => format!("{} {}", theme::spinner(state.elapsed_secs()), s.label()),
        SyncMode::NotRunning if s.error.is_none() && s.auto && s.sessions_done > 0 => {
            format!("◉ {}", s.label())
        }
        _ => format!("◌ {}", s.label()),
    };
    Span::styled(text, style)
}

/// Colour for the sync state wherever it is shown.
pub fn sync_style(state: &State, t: &Theme) -> Style {
    let s = &state.sync;
    match s.mode {
        SyncMode::Running => t.accent.add_modifier(Modifier::BOLD),
        SyncMode::Paused | SyncMode::Stopping => t.warning,
        SyncMode::NotRunning if s.error.is_some() => t.negative,
        SyncMode::NotRunning if !s.auto => t.warning,
        SyncMode::NotRunning if s.sessions_done > 0 => t.positive,
        SyncMode::NotRunning => t.muted,
    }
}

/// What this screen can do, as `key what · key what`. Switching screens is on the header.
fn hints(state: &State) -> String {
    let pairs: Vec<(&str, &str)> = match &state.overlay {
        Overlay::Help => vec![("esc", "close")],
        Overlay::TxDetail { .. } => vec![
            ("v", "switch view"),
            ("j/k", "scroll"),
            ("y", "copy txid"),
            ("esc", "back"),
        ],
        Overlay::AddressQr => vec![("y", "copy"), ("esc", "back")],
        Overlay::RescanConfirm => vec![("y", "rescan"), ("n", "cancel")],
        Overlay::NewUnifiedAddress { .. } => {
            vec![("j/k", "choose"), ("enter", "derive"), ("esc", "cancel")]
        }
        Overlay::ThemePicker { .. } => {
            vec![("j/k", "preview"), ("enter", "keep"), ("esc", "revert")]
        }
        Overlay::Faucet(stage) => match stage {
            FaucetStage::Ask { .. } => vec![("y", "ask"), ("n", "cancel")],
            FaucetStage::Requesting { .. } => Vec::new(),
            FaucetStage::Done { .. } | FaucetStage::Failed(_) => vec![("esc", "close")],
        },
        Overlay::BondMenu { .. } => vec![
            ("j/k", "choose"),
            ("enter", "go"),
            ("y", "copy finalizer"),
            ("esc", "back"),
        ],
        Overlay::Stake(form) => match &form.stage {
            StakeStage::Edit => vec![
                ("↑/↓", "power of ten"),
                ("enter", "review"),
                ("esc", "cancel"),
            ],
            StakeStage::Proposing => vec![("esc", "cancel")],
            StakeStage::Confirm { .. } => vec![
                (
                    "y",
                    match form.op {
                        StakeOp::Stake { .. } => "stake",
                        StakeOp::Unbond { .. } => "unbond",
                        StakeOp::Redelegate { .. } => "redelegate",
                        StakeOp::Withdraw { .. } => "withdraw",
                    },
                ),
                ("n", "back"),
            ],
            StakeStage::Sending => Vec::new(),
            StakeStage::Sent { .. } => vec![("y", "copy txid"), ("esc", "to bonds")],
            StakeStage::Failed(_) => vec![("esc", "close")],
        },
        Overlay::None => {
            let mut pairs = match state.screen {
                Screen::Home => vec![
                    ("j/k", "move"),
                    ("enter", "open"),
                    ("F", "faucet"),
                    ("c", "theme"),
                ],
                Screen::Pay => match state.send.stage {
                    SendStage::Edit => {
                        vec![("tab", "next field"), ("enter", "review"), ("esc", "home")]
                    }
                    SendStage::Proposing => vec![("esc", "cancel")],
                    SendStage::Confirm { .. } => vec![("y", "send"), ("n", "back")],
                    SendStage::Sending => Vec::new(),
                    SendStage::Sent { .. } => vec![("y", "copy txid"), ("esc", "home")],
                },
                Screen::Transactions => vec![("j/k", "move"), ("enter", "open")],
                // the actions first: the footer shares its row with the staking calendar
                Screen::Bonds => match state.selected_bond() {
                    Some(bond) => std::iter::once(("enter", "actions"))
                        .chain(
                            BondAction::for_status(bond.status)
                                .iter()
                                .map(|a| (a.key_name(), a.label())),
                        )
                        .collect(),
                    None => vec![("f", "finalizers, to stake")],
                },
                Screen::Finalizers if state.redelegating.is_some() => {
                    vec![("enter", "move here"), ("esc", "cancel")]
                }
                Screen::Finalizers => {
                    vec![("enter", "stake"), ("y", "copy finalizer"), ("j/k", "move")]
                }
                Screen::Receive => vec![
                    ("v", "unified ⇄ transparent"),
                    ("n", "new address"),
                    ("F", "faucet"),
                    ("y", "copy"),
                ],
                Screen::Addresses => {
                    vec![
                        ("j/k", "move"),
                        ("enter", "qr"),
                        ("n", "new"),
                        ("y", "copy"),
                    ]
                }
                Screen::Sync => {
                    let mut pairs = match state.sync.p_pauses() {
                        Some(true) => vec![("space", "pause")],
                        Some(false) => vec![("space", "resume")],
                        None => Vec::new(),
                    };
                    pairs.push(("m", if state.sync_map { "strip" } else { "chain map" }));
                    pairs.push(("R", "rescan"));
                    pairs
                }
            };
            pairs.push(("?", "help"));
            pairs.push(("q", "quit"));
            pairs
        }
    };
    join_hints(&pairs)
}

/// `key what · key what`.
pub fn join_hints(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, w)| format!("{k} {w}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A row of tabs. The active one is a filled pill; the others show their key letter in the
/// accent colour, so the tab bar doubles as the navigation hint.
pub fn tabs(labels: &[&str], active: usize, t: &Theme, keyed: bool) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        if i == active {
            spans.push(Span::styled(format!(" {label} "), t.selection));
        } else if keyed {
            let mut chars = label.chars();
            let first = chars.next().map(String::from).unwrap_or_default();
            spans.push(Span::raw(" "));
            spans.push(Span::styled(first, t.accent));
            spans.push(Span::styled(format!("{} ", chars.as_str()), t.muted));
        } else {
            spans.push(Span::styled(format!(" {label} "), t.muted));
        }
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// A label column followed by a value in body text.
pub fn kv(t: &Theme, label: &str, value: impl Into<String>) -> Line<'static> {
    kv_styled(t, label, value, t.text)
}

/// A label column followed by a value in `style`.
pub fn kv_styled(t: &Theme, label: &str, value: impl Into<String>, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<14}"), t.muted),
        Span::styled(value.into(), style),
    ])
}

/// Columns between the pane's rule and its content.
pub const EDGE: u16 = 2;

/// `area` narrowed to the content edges, keeping its rows.
pub fn edged(area: Rect) -> Rect {
    Rect {
        x: area.x + EDGE,
        width: area.width.saturating_sub(2 * EDGE),
        ..area
    }
}

/// The body area: within the content edges, one row below the pane title.
pub fn inset(area: Rect) -> Rect {
    Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..edged(area)
    }
}

#[cfg(test)]
mod tests;
