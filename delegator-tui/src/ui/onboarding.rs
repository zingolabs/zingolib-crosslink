//! The first-run screens: the wallet list, the setup menu, and the branches that make a wallet.
//!
//! Laid out like zingolib-tui's wizard, a title row with the step count on the right, a rule
//! under it, and the body two columns in, dressed like athanor's lockscreen: a sigil for the
//! key, prompts that speak, double-framed input boxes, the birthday in big digits, the lake of
//! the dither field where there is room, and the key legend on the last row with the chain and
//! the tip on its right.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::{bignum, dither, edged, inset, separator, sigil};
use crate::app::onboarding::{
    check_ufvk, fresh_birthday, height_for_date, parse_birthday, parse_date,
};
use crate::app::{
    BirthdayMode, KeyMaterial, MenuItem, Network, Onboarding, State, Step, WalletKind,
};
use crate::format;
use crate::theme::Theme;

/// With this much room the lake takes the left of the screen and the content the right.
const LAKE_MIN_WIDTH: u16 = 110;
const LAKE_MIN_HEIGHT: u16 = 30;

/// One thing in the body, drawn in order. Every `Line` is one row, so rows can be counted.
enum Piece {
    Lines(Vec<Line<'static>>),
    /// A text field in a double frame, the cursor after the value.
    Field(String),
    /// Big digits.
    Number(String),
    /// The sigil for this material.
    Sigil(String),
}

/// `Enter continue  Esc back`: the key in the accent, what it does muted.
fn legend(t: &Theme, pairs: &[(&'static str, &'static str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, what)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*key, t.accent));
        spans.push(Span::styled(format!(" {what}"), t.muted));
    }
    Line::from(spans)
}

/// A question, spoken: the `@` in the accent, the words bold.
fn prompt(t: &Theme, text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("@ ", t.accent),
        Span::styled(text.to_string(), t.text.add_modifier(Modifier::BOLD)),
    ])
}

/// Prose wrapped at word boundaries into one `Line` per row.
fn para(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    let width = width.max(8);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(Line::styled(std::mem::take(&mut cur), style));
            cur = word.to_string();
        }
    }
    lines.push(Line::styled(cur, style));
    lines
}

/// Rows with the chosen one marked by `> ` and the title style, like the theme picker.
fn choices(t: &Theme, rows: Vec<(String, String)>, selected: usize) -> Vec<Line<'static>> {
    rows.into_iter()
        .enumerate()
        .map(|(i, (label, detail))| {
            let chosen = i == selected;
            Line::from(vec![
                if chosen {
                    Span::styled("> ", t.accent)
                } else {
                    Span::raw("  ")
                },
                Span::styled(
                    format!("{label:<30}"),
                    if chosen { t.title } else { t.text },
                ),
                Span::styled(detail, t.muted),
            ])
        })
        .collect()
}

fn blank() -> Line<'static> {
    Line::from("")
}

/// `2 min ago`, `3 h ago`, `5 d ago`.
pub fn age(secs: Option<u64>) -> String {
    match secs {
        None => "—".into(),
        Some(s) if s < 60 => "just now".into(),
        Some(s) if s < 3600 => format!("{} min ago", s / 60),
        Some(s) if s < 86_400 => format!("{} h ago", s / 3600),
        Some(s) => format!("{} d ago", s / 86_400),
    }
}

/// What the title row says after the name, and the step count on its right.
fn heading(o: &Onboarding) -> (&'static str, String) {
    let branch_steps = |first: usize, total: usize| format!("step {first} of {total}");
    match o.step {
        Step::Wallets => (
            "wallets",
            match o.wallets.len() {
                0 => String::new(),
                1 => "1 wallet".into(),
                n => format!("{n} wallets"),
            },
        ),
        Step::Menu => ("new wallet", String::new()),
        Step::Create => ("new wallet", branch_steps(1, 2)),
        Step::Restore => ("restore from seed", branch_steps(1, 3)),
        Step::Watch => ("new watch-only wallet", branch_steps(1, 3)),
        Step::Birthday => (
            if o.is_watch() {
                "new watch-only wallet"
            } else {
                "restore from seed"
            },
            branch_steps(2, 3),
        ),
        Step::Name => {
            if o.seed.is_some() && !o.is_watch() {
                ("new wallet", branch_steps(2, 2))
            } else if o.is_watch() {
                ("new watch-only wallet", branch_steps(3, 3))
            } else {
                ("restore from seed", branch_steps(3, 3))
            }
        }
    }
}

/// The key legend for the step, and the status for the right of the last row: the tip as
/// `T:` and the chain, like a roguelike status line.
fn footer(o: &Onboarding, t: &Theme) -> (Line<'static>, Line<'static>) {
    let keys: Vec<(&'static str, &'static str)> = match o.step {
        Step::Wallets => vec![
            ("j k", "move"),
            ("Enter", "open"),
            ("n", "new"),
            ("q", "quit"),
        ],
        Step::Menu => vec![
            ("j k", "move"),
            ("1 2 3", "jump"),
            ("Enter", "choose"),
            ("n", "chain"),
            ("Esc", if o.wallets.is_empty() { "quit" } else { "back" }),
        ],
        Step::Create => vec![
            ("Enter", "create"),
            ("r", if o.seed_line { "grid" } else { "one line" }),
            ("y", "copy birthday"),
            ("Esc", "back"),
        ],
        Step::Restore => vec![
            ("Enter", "continue"),
            ("Backspace", "remove"),
            (
                "Esc",
                if o.words.is_empty() && o.draft.is_empty() {
                    "back"
                } else {
                    "clear"
                },
            ),
        ],
        Step::Watch => vec![
            ("Enter", "continue"),
            ("Esc", if o.key.is_empty() { "back" } else { "clear" }),
        ],
        Step::Birthday => match o.birthday_mode {
            None => vec![
                ("j k", "move"),
                ("1-4", "jump"),
                ("Enter", "choose"),
                ("Esc", "back"),
            ],
            Some(BirthdayMode::Unknown) => vec![("y", "scan everything"), ("n", "choose again")],
            Some(_) => vec![("Enter", "continue"), ("Esc", "choose again")],
        },
        Step::Name => vec![("Enter", "create"), ("Esc", "back")],
    };
    let mut status = Vec::new();
    let network = o.target_network();
    if let Some(tip) = o.tip_for(network) {
        status.push(Span::styled("T:", t.muted));
        status.push(Span::styled(format::group(u64::from(tip.height)), t.text));
        status.push(Span::raw("  "));
    } else if o.tip_error.is_some() {
        status.push(Span::styled("T:none", t.warning));
        status.push(Span::raw("  "));
    }
    status.push(Span::styled(
        network.name().to_string(),
        if network == Network::Stable {
            t.muted
        } else {
            t.warning
        },
    ));
    (legend(t, &keys), Line::from(status))
}

pub fn draw(frame: &mut Frame, area: Rect, state: &State, t: &Theme, o: &Onboarding) {
    let (what, right) = heading(o);
    let header = edged(Rect { height: 1, ..area });
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("delegator-tui", t.title),
            Span::styled(format!("   {what}"), t.text),
        ])),
        header,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(right, t.muted)).alignment(Alignment::Right),
        header,
    );
    separator(
        frame,
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
        t,
    );
    // the legend on the last row, the status on its right; the status keeps its room
    let (keys, status) = footer(o, t);
    let last = edged(Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    });
    let status_width = u16::try_from(status.width()).unwrap_or(0);
    frame.render_widget(
        Paragraph::new(keys),
        Rect {
            width: last.width.saturating_sub(status_width + 2),
            ..last
        },
    );
    frame.render_widget(Paragraph::new(status).alignment(Alignment::Right), last);
    // the body: between the rule and the legend, with a blank row above the legend
    let body_area = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(4),
        ..area
    };
    let roomy = area.width >= LAKE_MIN_WIDTH && area.height >= LAKE_MIN_HEIGHT;
    let (lake, body) = if roomy {
        let lake_width = area.width * 2 / 5;
        (
            Some(Rect {
                width: lake_width,
                ..body_area
            }),
            inset(Rect {
                x: body_area.x + lake_width,
                width: body_area.width - lake_width,
                ..body_area
            }),
        )
    } else {
        (None, inset(body_area))
    };
    let width = usize::from(body.width);
    let pieces = match o.step {
        Step::Wallets => wallets(o, t),
        Step::Menu => menu(o, t, width),
        Step::Create => create(o, t, width),
        Step::Restore => restore(o, t, width),
        Step::Watch => watch(o, t, width),
        Step::Birthday => birthday(o, t, width),
        Step::Name => name(o, t, width),
    };
    let used = draw_pieces(frame, body, t, pieces);
    // the message line: the error sits on the row above the legend, where athanor's
    // notifications go, so it is never pushed off by the content
    if let Some(err) = &o.error {
        frame.render_widget(
            Paragraph::new(Line::styled(
                err.clone(),
                t.negative.add_modifier(Modifier::BOLD),
            )),
            edged(Rect {
                y: area.y + area.height - 2,
                height: 1,
                ..area
            }),
        );
    }
    match lake {
        // the lake fills its column, fading up from the legend
        Some(lake) => dither::band(frame, lake, state, t),
        // or the tail under the content, when at least a few rows are free
        None if body.height.saturating_sub(used) >= 5 => dither::band(frame, body, state, t),
        None => {}
    }
}

/// Draws the pieces top to bottom; returns the rows used.
fn draw_pieces(frame: &mut Frame, area: Rect, t: &Theme, pieces: Vec<Piece>) -> u16 {
    let mut y = area.y;
    let bottom = area.y + area.height;
    for piece in pieces {
        if y >= bottom {
            break;
        }
        let room = Rect {
            y,
            height: bottom - y,
            ..area
        };
        let used = match piece {
            Piece::Lines(lines) => {
                let n = u16::try_from(lines.len())
                    .unwrap_or(u16::MAX)
                    .min(room.height);
                frame.render_widget(Paragraph::new(lines), room);
                n
            }
            Piece::Field(value) => {
                let inner_width = usize::from(area.width.saturating_sub(4)).max(1);
                let rows = format::wrap_chars(&value, inner_width);
                let height = (u16::try_from(rows.len()).unwrap_or(1) + 2).min(room.height);
                let rect = Rect { height, ..room };
                let block = Block::bordered()
                    .border_type(BorderType::Double)
                    .border_style(t.accent);
                let inner = block.inner(rect);
                frame.render_widget(block, rect);
                let last = rows.len().saturating_sub(1);
                let lines: Vec<Line<'static>> = rows
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| {
                        let mut spans = vec![Span::raw(" "), Span::styled(row, t.text)];
                        if i == last {
                            spans.push(Span::styled(" ", t.cursor));
                        }
                        Line::from(spans)
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), inner);
                height
            }
            Piece::Number(text) => match bignum::render(&text) {
                Some(rows) if rows[0].chars().count() <= usize::from(area.width) => {
                    let lines: Vec<Line<'static>> = rows
                        .into_iter()
                        .map(|r| Line::from(Span::styled(r, t.title)))
                        .collect();
                    frame.render_widget(Paragraph::new(lines), room);
                    3
                }
                _ => {
                    frame.render_widget(Paragraph::new(Line::styled(text, t.title)), room);
                    1
                }
            },
            Piece::Sigil(material) => {
                sigil::draw(frame, room, t, &material);
                sigil::HEIGHT.min(room.height)
            }
        };
        y += used;
    }
    y - area.y
}

fn wallets(o: &Onboarding, t: &Theme) -> Vec<Piece> {
    let mut lines = Vec::new();
    if !o.listed {
        lines.push(Line::styled("Reading the data directory…", t.muted));
        return vec![Piece::Lines(lines)];
    }
    lines.push(prompt(t, "open a wallet, or make a new one:"));
    lines.push(blank());
    lines.push(Line::styled(
        format!(
            "  {:<18}{:<9}{:<7}{:<12}{}",
            "name", "chain", "kind", "birthday", "last written"
        ),
        t.muted,
    ));
    for (i, w) in o.wallets.iter().enumerate() {
        let chosen = i == o.wallet_sel;
        lines.push(Line::from(vec![
            if chosen {
                Span::styled("> ", t.accent)
            } else {
                Span::raw("  ")
            },
            Span::styled(
                format!("{:<18}", w.name),
                if chosen { t.title } else { t.text },
            ),
            Span::styled(
                format!("{:<9}", w.network.name()),
                if w.network == Network::Stable {
                    t.text
                } else {
                    t.warning
                },
            ),
            Span::styled(
                format!("{:<7}", w.kind.name()),
                match w.kind {
                    WalletKind::Seed => t.accent,
                    WalletKind::Watch => t.info,
                    WalletKind::Unknown => t.muted,
                },
            ),
            Span::styled(
                format!(
                    "{:<12}",
                    w.birthday
                        .map_or("—".to_string(), |b| format::group(u64::from(b)))
                ),
                t.text,
            ),
            Span::styled(age(w.age_secs), t.muted),
        ]));
    }
    let chosen = o.wallet_sel == o.wallets.len();
    lines.push(Line::from(vec![
        if chosen {
            Span::styled("> ", t.accent)
        } else {
            Span::raw("  ")
        },
        Span::styled("New wallet…", if chosen { t.title } else { t.muted }),
    ]));
    let mut pieces = vec![Piece::Lines(lines)];
    if let Some(w) = o.wallets.get(o.wallet_sel) {
        pieces.push(Piece::Lines(vec![blank()]));
        pieces.push(Piece::Sigil(format!("{}:{}", w.network.name(), w.name)));
    }
    pieces
}

fn menu(o: &Onboarding, t: &Theme, width: usize) -> Vec<Piece> {
    let mut lines = vec![prompt(
        t,
        if o.wallets.is_empty() {
            "no wallet yet. what would you like to do?"
        } else {
            "what would you like to do?"
        },
    )];
    lines.extend(para(
        "You can also paste a seed phrase or a viewing key here.",
        t.muted,
        width,
    ));
    lines.push(blank());
    lines.extend(choices(
        t,
        MenuItem::ALL
            .iter()
            .map(|m| (m.label().to_string(), m.detail().to_string()))
            .collect(),
        o.menu_sel,
    ));
    lines.push(blank());
    lines.push(Line::from(vec![
        Span::styled("Chain: ", t.muted),
        Span::styled(o.network.label().to_string(), t.text),
        Span::styled(".", t.muted),
    ]));
    lines.extend(para(
        "The two crosslink chains share their keys, so a key cannot tell them apart.",
        t.muted,
        width,
    ));
    vec![Piece::Lines(lines)]
}

/// The numbered word grid, as many columns as fit.
fn word_grid(
    t: &Theme,
    words: &[Option<String>],
    width: usize,
    draft_at: Option<usize>,
) -> Vec<Line<'static>> {
    let cols = (width / 15).clamp(2, 4);
    words
        .chunks(cols)
        .enumerate()
        .map(|(row, chunk)| {
            let mut spans = Vec::new();
            for (i, word) in chunk.iter().enumerate() {
                let n = row * cols + i;
                spans.push(Span::styled(format!("{:>2} ", n + 1), t.muted));
                match word {
                    Some(w) => spans.push(Span::styled(
                        format!("{w:<11}"),
                        if draft_at == Some(n) { t.muted } else { t.text },
                    )),
                    None => spans.push(Span::styled(format!("{:<11}", "·"), t.border)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

fn create(o: &Onboarding, t: &Theme, width: usize) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut lines = vec![prompt(t, "your seed phrase. write it down.")];
    lines.extend(para(
        "It is the only backup. Anyone holding it can spend. Keep the birthday with it.",
        t.muted,
        width,
    ));
    lines.push(blank());
    match &o.seed {
        None => lines.push(Line::styled("Generating…", t.muted)),
        Some(seed) if o.seed_line => lines.extend(para(seed, t.text, width)),
        Some(seed) => {
            let words: Vec<Option<String>> = seed.split(' ').map(|w| Some(w.to_string())).collect();
            lines.extend(word_grid(t, &words, width, None));
        }
    }
    lines.push(blank());
    pieces.push(Piece::Lines(lines));
    match o.tip_for(o.network) {
        Some(tip) => {
            let birthday = fresh_birthday(o.network, tip.height);
            pieces.push(Piece::Lines(vec![Line::styled("Birthday", t.muted)]));
            pieces.push(Piece::Number(format::group(u64::from(birthday))));
            pieces.push(Piece::Lines(para(
                &format!(
                    "{} on {}. A restore with it starts here instead of scanning the whole chain.",
                    format::group(u64::from(birthday)),
                    o.network.label()
                ),
                t.muted,
                width,
            )));
        }
        None => pieces.push(Piece::Lines(match &o.tip_error {
            Some(e) => para(
                &format!(
                    "The birthday needs the chain tip, and the indexer did not answer: {e}. R tries again."
                ),
                t.warning,
                width,
            ),
            None => para(
                &format!(
                    "Asking {} for the chain tip, which sets the birthday…",
                    o.network.default_server()
                ),
                t.muted,
                width,
            ),
        })),
    }
    if let Some(seed) = &o.seed {
        pieces.push(Piece::Lines(vec![blank()]));
        pieces.push(Piece::Sigil(seed.clone()));
    }
    pieces
}

fn restore(o: &Onboarding, t: &Theme, width: usize) -> Vec<Piece> {
    let mut lines = vec![prompt(t, "speak the seed phrase:")];
    lines.extend(para(
        "Type or paste it. Space ends a word. A number after the words is read as the birthday, the way the app copies both.",
        t.muted,
        width,
    ));
    lines.push(blank());
    let mut words: Vec<Option<String>> = o.words.iter().cloned().map(Some).collect();
    let draft_at = (!o.draft.is_empty() && words.len() < 24).then_some(words.len());
    if draft_at.is_some() {
        words.push(Some(o.draft.clone()));
    }
    words.resize(24, None);
    lines.extend(word_grid(t, &words, width, draft_at));
    lines.push(blank());
    let count = o.words.len();
    let complete = [12, 15, 18, 21, 24].contains(&count);
    let mut status = vec![Span::styled(
        format!("{count} of 24 words"),
        if complete { t.positive } else { t.muted },
    )];
    if let Some(hint) = o.birthday_hint {
        status.push(Span::styled(
            format!(
                ", birthday {} from the paste",
                format::group(u64::from(hint))
            ),
            t.muted,
        ));
    }
    lines.push(Line::from(status));
    vec![Piece::Lines(lines)]
}

fn watch(o: &Onboarding, t: &Theme, width: usize) -> Vec<Piece> {
    let mut intro = vec![prompt(t, "paste the viewing key:")];
    intro.extend(para(
        "From Zingo Delegator: settings › viewing key. The key alone, or the key and the birthday. Nothing is sent anywhere.",
        t.muted,
        width,
    ));
    intro.push(blank());
    let mut pieces = vec![Piece::Lines(intro), Piece::Field(o.key.clone())];
    let mut parts = o.key.split_whitespace();
    if let Some(first) = parts.next() {
        let mut lines = vec![blank()];
        match check_ufvk(first) {
            Ok(()) => {
                let hint = parts.next().and_then(|n| n.parse::<u32>().ok());
                lines.push(Line::styled(
                    "viewing key for the crosslink chains",
                    t.positive,
                ));
                lines.extend(para(
                    &match hint {
                        Some(h) => {
                            format!("birthday {} from the paste", format::group(u64::from(h)))
                        }
                        None => "no birthday in the paste: the next step asks".to_string(),
                    },
                    t.muted,
                    width,
                ));
                lines.push(blank());
                pieces.push(Piece::Lines(lines));
                pieces.push(Piece::Sigil(first.to_string()));
            }
            Err(e) => {
                lines.extend(para(&e, t.warning, width));
                pieces.push(Piece::Lines(lines));
            }
        }
    }
    pieces
}

fn birthday(o: &Onboarding, t: &Theme, width: usize) -> Vec<Piece> {
    let network = o.target_network();
    let floor = network.sapling_activation();
    let Some(mode) = o.birthday_mode else {
        let mut lines = vec![prompt(t, "when was the wallet created?")];
        lines.extend(para(
            "Blocks before its birthday are never scanned. Too late hides old funds; too early makes the first sync slow.",
            t.muted,
            width,
        ));
        lines.push(blank());
        lines.extend(choices(
            t,
            vec![
                (
                    "paste it from the app".into(),
                    match o.birthday_hint {
                        Some(h) => format!("found {}", format::group(u64::from(h))),
                        None => "settings › viewing key › tap the birthday".into(),
                    },
                ),
                ("I know the block height".into(), String::new()),
                ("I know roughly the date".into(), String::new()),
                (
                    "I don't know".into(),
                    format!(
                        "scan from {}, the first block",
                        format::group(u64::from(floor))
                    ),
                ),
            ],
            o.birthday_sel,
        ));
        return vec![Piece::Lines(lines)];
    };
    if mode == BirthdayMode::Unknown {
        let blocks = o
            .tip_for(network)
            .map(|tip| tip.height.saturating_sub(floor));
        let mut lines = vec![prompt(t, "scan the whole chain?")];
        lines.extend(para(
            &format!(
                "From block {}{}. The wallet is usable only once the scan finishes, which can take hours.",
                format::group(u64::from(floor)),
                blocks.map_or(String::new(), |b| format!(
                    ", {} blocks",
                    format::group(u64::from(b))
                ))
            ),
            t.muted,
            width,
        ));
        lines.extend(para(
            "Installed the app this year? \"I know roughly the date\" is faster and safe.",
            t.muted,
            width,
        ));
        return vec![Piece::Lines(lines)];
    }
    let mut intro = vec![prompt(
        t,
        match mode {
            BirthdayMode::FromApp => "paste the birthday from the app:",
            BirthdayMode::Height => "at which block height was the wallet created?",
            _ => "around which date was the wallet created?",
        },
    )];
    intro.extend(para(
        &match mode {
            BirthdayMode::FromApp => {
                "In the app, tap the birthday under the viewing key to copy it.".to_string()
            }
            BirthdayMode::Height => format!(
                "A number between {} and the chain tip.",
                format::group(u64::from(floor))
            ),
            _ => "YYYY-MM or YYYY-MM-DD. The birthday is placed two weeks before it, for safety."
                .to_string(),
        },
        t.muted,
        width,
    ));
    intro.push(blank());
    let mut pieces = vec![Piece::Lines(intro), Piece::Field(o.birthday_input.clone())];
    let mut lines = vec![blank()];
    if !o.birthday_input.trim().is_empty() {
        match parse_birthday(o, mode) {
            Ok(height) => {
                let mut spans = vec![Span::styled(
                    format!("Height {}", format::group(u64::from(height))),
                    t.positive,
                )];
                if let Some(tip) = o.tip_for(network) {
                    spans.push(Span::styled(
                        format!(
                            ", {} blocks to scan up to the tip at {}",
                            format::group(u64::from(tip.height.saturating_sub(height))),
                            format::group(u64::from(tip.height))
                        ),
                        t.muted,
                    ));
                }
                lines.push(Line::from(spans));
            }
            Err(e) => lines.extend(para(&e, t.warning, width)),
        }
    } else if mode == BirthdayMode::Date
        && let Some(tip) = o.tip_for(network)
        && let Some(h) = parse_date("2025-01-01")
            .ok()
            .and_then(|d| height_for_date(network, tip, d))
    {
        lines.push(Line::styled(
            format!(
                "For example, 2025-01-01 gives height {}.",
                format::group(u64::from(h))
            ),
            t.muted,
        ));
    }
    pieces.push(Piece::Lines(lines));
    pieces
}

fn name(o: &Onboarding, t: &Theme, width: usize) -> Vec<Piece> {
    let network = o.target_network();
    let mut intro = vec![prompt(t, "name the wallet:")];
    intro.extend(para(
        &format!(
            "A {} {} wallet with birthday {}. The name becomes the file name, <name>.dat, in the data directory.",
            network.label(),
            if o.is_watch() { "watch-only" } else { "seed" },
            o.birthday
                .map_or("—".to_string(), |b| format::group(u64::from(b)))
        ),
        t.muted,
        width,
    ));
    intro.push(blank());
    let mut pieces = vec![Piece::Lines(intro), Piece::Field(o.name.clone())];
    let mut lines = Vec::new();
    if !o.wallets.is_empty() {
        lines.push(blank());
        lines.extend(para(
            &format!(
                "In use: {}.",
                o.wallets
                    .iter()
                    .map(|w| w.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            t.muted,
            width,
        ));
    }
    lines.push(blank());
    pieces.push(Piece::Lines(lines));
    if let Some((_, material)) = &o.material {
        let key = match material {
            KeyMaterial::Seed(s) => s.clone(),
            KeyMaterial::Ufvk(k) => k.clone(),
        };
        pieces.push(Piece::Sigil(key));
    }
    pieces
}
