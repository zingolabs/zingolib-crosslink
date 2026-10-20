//! Rendering. Every function here reads the state and draws; none of them change it.
//!
//! The chrome follows lwtui, the lightwallet-tools explorer: a one-line header with the title,
//! a status mark and a breadcrumb; panes with a rule on their left and a title on it; a gold
//! caret on the selected row; dim key hints on the last line.

mod addresses;
mod bignum;
mod diagram;
mod dither;
mod help;
mod home;
mod onboarding;
mod receive;
mod sigil;
mod sync;
mod themes;
mod transactions;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::app::{Network, Overlay, Phase, Screen, State, SyncMode};
use crate::theme::{self, Theme};

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
                Span::styled(
                    format!("{} {}", theme::spinner(state.elapsed_secs()), label),
                    t.muted,
                ),
                Vec::new(),
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

/// The header line: the shimmering title, a status mark, then the breadcrumb or tabs.
pub fn header(
    frame: &mut Frame,
    area: Rect,
    state: &State,
    t: &Theme,
    indicator: Span<'static>,
    crumbs: Vec<Span<'static>>,
) {
    let mut spans = theme::shimmer(t, "delegator-tui", state.elapsed_secs(), state.color_depth);
    spans.push(Span::raw("   "));
    spans.push(indicator);
    if !crumbs.is_empty() {
        spans.push(Span::styled("  ·  ", t.muted));
        spans.extend(crumbs);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    header(
        frame,
        header_area,
        state,
        t,
        sync_indicator(state, t),
        screen_tabs(state, t, area.width),
    );
    let title = match &state.overlay {
        Overlay::Help | Overlay::None => state.screen.title().to_lowercase(),
        Overlay::TxDetail { .. } => "transaction".into(),
        Overlay::AddressQr => "address".into(),
        Overlay::RescanConfirm => "rescan".into(),
        Overlay::NewUnifiedAddress { .. } => "new address".into(),
        Overlay::ThemePicker { .. } => "theme".into(),
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
        Overlay::Help | Overlay::None => match state.screen {
            Screen::Home => home::draw(frame, body, state, t),
            Screen::Transactions => transactions::draw(frame, body, state, t),
            Screen::Receive => receive::draw(frame, body, state, t),
            Screen::Addresses => addresses::draw(frame, body, state, t),
            Screen::Sync => sync::draw(frame, body, state, t),
        },
    }
    if state.overlay == Overlay::None && state.screen == Screen::Home {
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
    footer(frame, footer_area, state, t, &keys);
}

/// The screens as a row of names, the current one bright. Each opens with its first letter.
/// The wallet's name comes first when the line has room for it.
fn screen_tabs(state: &State, t: &Theme, width: u16) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(w) = &state.wallet
        && width >= 100
    {
        spans.push(Span::styled(w.wallet_name.clone(), t.text));
        spans.push(Span::styled(
            format!(" {}", w.network.name()),
            if w.network == Network::Stable {
                t.muted
            } else {
                t.warning
            },
        ));
        spans.push(Span::styled("  ·  ", t.muted));
    }
    for (i, screen) in Screen::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let label = screen.title().to_lowercase();
        if *screen == state.screen {
            spans.push(Span::styled(label, t.accent.add_modifier(Modifier::BOLD)));
        } else {
            let mut chars = label.chars();
            let first = chars.next().map(String::from).unwrap_or_default();
            spans.push(Span::styled(first, t.text));
            spans.push(Span::styled(chars.as_str().to_string(), t.muted));
        }
    }
    spans
}

/// The sync state as one mark and a word: what the user acts on.
fn sync_indicator(state: &State, t: &Theme) -> Span<'static> {
    let s = &state.sync;
    let style = sync_style(state, t);
    let text = match s.mode {
        SyncMode::Running => format!("{} {}", theme::spinner(state.elapsed_secs()), s.label()),
        SyncMode::NotRunning if s.error.is_none() && s.auto && s.sessions_done > 0 => {
            format!("● {}", s.label())
        }
        _ => format!("○ {}", s.label()),
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
        Overlay::None => {
            let mut pairs = match state.screen {
                Screen::Home => vec![("j/k", "move"), ("enter", "open"), ("c", "theme")],
                Screen::Transactions => vec![("j/k", "move"), ("enter", "open")],
                Screen::Receive => vec![
                    ("v", "unified ⇄ transparent"),
                    ("n", "new address"),
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
                        Some(true) => vec![("p", "pause")],
                        Some(false) => vec![("p", "resume")],
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
