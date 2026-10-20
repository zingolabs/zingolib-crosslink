//! Rendering. Every function here reads the state and draws; none of them change it.

mod addresses;
mod bignum;
mod diagram;
mod help;
mod home;
mod receive;
mod sync;
mod themes;
mod transactions;
mod wizard;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::{Network, Overlay, Phase, Screen, State, SyncMode};
use crate::theme::Theme;

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
        Phase::Wizard(w) => wizard::draw(frame, area, &t, w),
        Phase::Loading(label) => centered(
            frame,
            area,
            vec![
                Line::styled(label.clone(), t.text),
                Line::styled("Ctrl-C aborts", t.muted),
            ],
        ),
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

pub fn separator(frame: &mut Frame, area: Rect, t: &Theme) {
    frame.render_widget(
        Paragraph::new(Line::styled("─".repeat(usize::from(area.width)), t.border)),
        area,
    );
}

fn draw_main(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let [header, sep1, body, sep2, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    // the bars share the body's edges: two columns in from either side
    let status = status_line(state, t);
    let [_, tabs_area, status_area, _] = Layout::horizontal([
        Constraint::Length(EDGE),
        Constraint::Fill(1),
        Constraint::Length(status.width() as u16 + 1),
        Constraint::Length(EDGE),
    ])
    .areas(header);
    frame.render_widget(Paragraph::new(screen_tabs(state.screen, t)), tabs_area);
    frame.render_widget(
        Paragraph::new(status).alignment(Alignment::Right),
        status_area,
    );
    separator(frame, sep1, t);
    separator(frame, sep2, t);
    frame.render_widget(Paragraph::new(footer_line(state, t)), edged(footer));

    match &state.overlay {
        Overlay::Help => help::draw(frame, body, t),
        Overlay::TxDetail { mode } => transactions::draw_detail(frame, body, state, t, *mode),
        Overlay::AddressQr => addresses::draw_qr(frame, body, state, t),
        Overlay::RescanConfirm => sync::draw_rescan_confirm(frame, body, state, t),
        Overlay::NewUnifiedAddress { orchard_only } => {
            receive::draw_new_address(frame, body, t, *orchard_only)
        }
        Overlay::ThemePicker { original } => themes::draw(frame, body, state, t, *original),
        Overlay::None => match state.screen {
            Screen::Home => home::draw(frame, body, state, t),
            Screen::Transactions => transactions::draw(frame, body, state, t),
            Screen::Receive => receive::draw(frame, body, state, t),
            Screen::Addresses => addresses::draw(frame, body, state, t),
            Screen::Sync => sync::draw(frame, body, state, t),
        },
    }
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

fn screen_tabs(active: Screen, t: &Theme) -> Line<'static> {
    let labels: Vec<&str> = Screen::ALL.iter().map(|s| s.title()).collect();
    let index = Screen::ALL.iter().position(|s| *s == active).unwrap_or(0);
    tabs(&labels, index, t, true)
}

/// Only what needs attention: the network when it is not mainnet, and the sync state.
fn status_line(state: &State, t: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    if let Some(w) = &state.wallet
        && w.network != Network::Mainnet
    {
        spans.push(Span::styled(w.network.name().to_string(), t.warning));
        spans.push(Span::raw("   "));
    }
    spans.push(Span::styled(state.sync.label(), sync_style(state, t)));
    Line::from(spans)
}

/// Colour for the sync state wherever it is shown.
pub fn sync_style(state: &State, t: &Theme) -> Style {
    let s = &state.sync;
    match s.mode {
        SyncMode::Running => t.accent,
        SyncMode::Paused | SyncMode::Stopping => t.warning,
        SyncMode::NotRunning if s.error.is_some() => t.negative,
        SyncMode::NotRunning if !s.auto => t.warning,
        SyncMode::NotRunning if s.sessions_done > 0 => t.positive,
        SyncMode::NotRunning => t.muted,
    }
}

fn footer_line(state: &State, t: &Theme) -> Line<'static> {
    if state.phase == Phase::ShuttingDown {
        return Line::styled(
            "Saving the wallet. Press q again to quit without waiting.",
            t.muted,
        );
    }
    let mut spans = Vec::new();
    if let Some(notice) = state.notice_text() {
        spans.push(Span::styled(
            notice.to_string(),
            t.warning.add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("   "));
    }
    for (i, (key, what)) in hints(state).into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(key, t.accent));
        spans.push(Span::styled(format!(" {what}"), t.muted));
    }
    Line::from(spans)
}

/// Key and description pairs for the footer: what this screen can do. Switching screens is
/// on the tab bar, so it is not repeated here.
fn hints(state: &State) -> Vec<(&'static str, &'static str)> {
    let always = [("?", "help"), ("q", "quit")];
    match &state.overlay {
        Overlay::Help => vec![("Esc", "back")],
        Overlay::TxDetail { .. } => vec![
            ("Esc", "back"),
            ("v", "switch view"),
            ("j k", "scroll"),
            ("y", "copy txid"),
        ],
        Overlay::AddressQr => vec![("Esc", "back"), ("y", "copy")],
        Overlay::RescanConfirm => vec![("y", "rescan"), ("n", "cancel")],
        Overlay::NewUnifiedAddress { .. } => {
            vec![("j k", "choose"), ("Enter", "derive"), ("Esc", "cancel")]
        }
        Overlay::ThemePicker { .. } => {
            vec![("j k", "preview"), ("Enter", "keep"), ("Esc", "revert")]
        }
        Overlay::None => {
            let mut pairs = match state.screen {
                Screen::Home => vec![("j k", "move"), ("Enter", "open"), ("c", "theme")],
                Screen::Transactions => vec![("j k", "move"), ("Enter", "open")],
                Screen::Receive => vec![
                    ("v", "unified or transparent"),
                    ("n", "new address"),
                    ("y", "copy"),
                ],
                Screen::Addresses => {
                    vec![
                        ("j k", "move"),
                        ("Enter", "QR code"),
                        ("n", "new"),
                        ("y", "copy"),
                    ]
                }
                Screen::Sync => {
                    let mut pairs = match state.sync.p_pauses() {
                        Some(true) => vec![("p", "pause")],
                        Some(false) => vec![("p", "resume")],
                        None => Vec::new(),
                    };
                    pairs.push(("m", if state.sync_map { "strip" } else { "chain map" }));
                    pairs.push(("R", "rescan"));
                    pairs
                }
            };
            pairs.extend(always);
            pairs
        }
    }
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

/// Columns between the screen edge and any content, on both sides. Everything that starts a
/// line, in the bars and in the body, starts here.
pub const EDGE: u16 = 2;

/// `area` narrowed to the content edges, keeping its rows.
pub fn edged(area: Rect) -> Rect {
    Rect {
        x: area.x + EDGE,
        width: area.width.saturating_sub(2 * EDGE),
        ..area
    }
}

/// The body area: within the content edges, one row below the separator.
pub fn inset(area: Rect) -> Rect {
    Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..edged(area)
    }
}

#[cfg(test)]
mod tests;
