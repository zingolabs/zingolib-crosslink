//! The first-run screens: the wallet list, the setup menu, and the branches that make a wallet.
//!
//! Laid out like zingolib-tui's wizard: a title row inside the content edges with the step
//! count on the right, a rule under it, the body two columns in, `> ` fields with a block
//! cursor, the key hints under the field, and the error in bold under everything.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{edged, inset, separator};
use crate::app::onboarding::{
    check_ufvk, fresh_birthday, height_for_date, parse_birthday, parse_date,
};
use crate::app::{BirthdayMode, MenuItem, Network, Onboarding, State, Step, WalletKind};
use crate::format;
use crate::theme::Theme;

/// A text field: a prompt marker, the value wrapped to the width, and a block cursor.
fn field(t: &Theme, value: &str, width: usize) -> Vec<Line<'static>> {
    let rows = format::wrap_chars(value, width.saturating_sub(2).max(1));
    let last = rows.len().saturating_sub(1);
    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = vec![
                if i == 0 {
                    Span::styled("> ", t.accent)
                } else {
                    Span::raw("  ")
                },
                Span::styled(row, t.text),
            ];
            if i == last {
                spans.push(Span::styled(" ", t.cursor));
            }
            Line::from(spans)
        })
        .collect()
}

/// `Enter continue  Esc back`: the key in the accent, what it does muted.
fn hint(t: &Theme, pairs: &[(&'static str, &'static str)]) -> Line<'static> {
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

fn error(t: &Theme, message: &str) -> Line<'static> {
    Line::styled(message.to_string(), t.negative.add_modifier(Modifier::BOLD))
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

pub fn draw(frame: &mut Frame, area: Rect, _state: &State, t: &Theme, o: &Onboarding) {
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
    let body = inset(Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(2),
        ..area
    });
    let width = usize::from(body.width);
    let mut lines = match o.step {
        Step::Wallets => wallets(o, t),
        Step::Menu => menu(o, t),
        Step::Create => create(o, t, width),
        Step::Restore => restore(o, t, width),
        Step::Watch => watch(o, t, width),
        Step::Birthday => birthday(o, t, width),
        Step::Name => name(o, t, width),
    };
    if let Some(err) = &o.error {
        lines.push(blank());
        lines.push(error(t, err));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

fn wallets(o: &Onboarding, t: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !o.listed {
        lines.push(Line::styled("Reading the data directory…", t.muted));
        return lines;
    }
    lines.push(Line::styled("Open a wallet, or make a new one.", t.text));
    lines.push(blank());
    lines.push(Line::styled(
        format!(
            "  {:<18}{:<9}{:<7}{:<12}{}",
            "name", "network", "kind", "birthday", "last written"
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
                if w.network == Network::Mainnet {
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
    lines.push(blank());
    lines.push(hint(
        t,
        &[
            ("j k", "move"),
            ("Enter", "open"),
            ("n", "new"),
            ("q", "quit"),
        ],
    ));
    lines
}

fn menu(o: &Onboarding, t: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            if o.wallets.is_empty() {
                "No wallet yet. What would you like to do?"
            } else {
                "What would you like to do?"
            },
            t.text,
        ),
        Line::styled(
            "You can also paste a seed phrase or a viewing key here.",
            t.muted,
        ),
        blank(),
    ];
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
        Span::styled("Network for a seed wallet: ", t.muted),
        Span::styled(o.network.name().to_string(), t.text),
        Span::styled(". A viewing key names its own.", t.muted),
    ]));
    lines.push(blank());
    lines.push(hint(
        t,
        &[
            ("j k", "move"),
            ("1 2 3", "jump"),
            ("Enter", "choose"),
            ("n", "network"),
            ("Esc", if o.wallets.is_empty() { "quit" } else { "back" }),
        ],
    ));
    lines
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

fn create(o: &Onboarding, t: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled("Your seed phrase.", t.text),
        Line::styled(
            "It is the only backup. Anyone holding it can spend. Write it down with the birthday.",
            t.muted,
        ),
        blank(),
    ];
    match &o.seed {
        None => lines.push(Line::styled("Generating…", t.muted)),
        Some(seed) if o.seed_line => lines.push(Line::styled(seed.clone(), t.text)),
        Some(seed) => {
            let words: Vec<Option<String>> = seed.split(' ').map(|w| Some(w.to_string())).collect();
            lines.extend(word_grid(t, &words, width, None));
        }
    }
    lines.push(blank());
    match o.tip_for(o.network) {
        Some(tip) => lines.push(Line::from(vec![
            Span::styled("Birthday ", t.text),
            Span::styled(
                format::group(u64::from(fresh_birthday(o.network, tip.height))),
                t.title,
            ),
            Span::styled(
                format!(
                    "  on {}. A restore with it starts here instead of scanning the whole chain.",
                    o.network.name()
                ),
                t.muted,
            ),
        ])),
        None => match &o.tip_error {
            Some(e) => lines.push(Line::styled(
                format!("The birthday needs the chain tip, and the server did not answer: {e}. R tries again."),
                t.warning,
            )),
            None => lines.push(Line::styled(
                format!(
                    "Asking {} for the chain tip, which sets the birthday…",
                    o.network.default_server()
                ),
                t.muted,
            )),
        },
    }
    lines.push(blank());
    lines.push(hint(
        t,
        &[
            ("Enter", "create wallet"),
            ("r", if o.seed_line { "grid" } else { "one line" }),
            ("y", "copy birthday"),
            ("Esc", "back"),
        ],
    ));
    lines
}

fn restore(o: &Onboarding, t: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled("Type or paste the seed phrase.", t.text),
        Line::styled(
            "Space ends a word. A number after the words is read as the birthday, the way the app copies both.",
            t.muted,
        ),
        blank(),
    ];
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
    lines.push(blank());
    lines.push(hint(
        t,
        &[
            ("Enter", "continue"),
            ("Backspace", "remove"),
            (
                "Esc",
                if count > 0 || !o.draft.is_empty() {
                    "clear"
                } else {
                    "back"
                },
            ),
        ],
    ));
    lines
}

fn watch(o: &Onboarding, t: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled("Paste the unified full viewing key.", t.text),
        Line::styled(
            "From Zingo Delegator: settings › viewing key. The key alone, or the key and the birthday. Nothing is sent anywhere.",
            t.muted,
        ),
        blank(),
    ];
    lines.extend(field(t, &o.key, width));
    let mut parts = o.key.split_whitespace();
    if let Some(first) = parts.next() {
        lines.push(blank());
        match check_ufvk(first) {
            Ok(network) => {
                let hint = parts.next().and_then(|n| n.parse::<u32>().ok());
                lines.push(Line::from(vec![
                    Span::styled(format!("{} viewing key", network.name()), t.positive),
                    Span::styled(
                        match hint {
                            Some(h) => {
                                format!(", birthday {} from the paste", format::group(u64::from(h)))
                            }
                            None => ", no birthday in the paste: the next step asks".into(),
                        },
                        t.muted,
                    ),
                ]));
            }
            Err(e) => lines.push(Line::styled(e, t.warning)),
        }
    }
    lines.push(blank());
    lines.push(hint(
        t,
        &[
            ("Enter", "continue"),
            ("Esc", if o.key.is_empty() { "back" } else { "clear" }),
        ],
    ));
    lines
}

fn birthday(o: &Onboarding, t: &Theme, width: usize) -> Vec<Line<'static>> {
    let network = o.target_network();
    let floor = network.sapling_activation();
    let Some(mode) = o.birthday_mode else {
        let mut lines = vec![
            Line::styled("When was the wallet created?", t.text),
            Line::styled(
                "Blocks before its birthday are never scanned. Too late hides old funds; too early makes the first sync slow.",
                t.muted,
            ),
            blank(),
        ];
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
                        "scan from {}, the start of Sapling",
                        format::group(u64::from(floor))
                    ),
                ),
            ],
            o.birthday_sel,
        ));
        lines.push(blank());
        lines.push(hint(
            t,
            &[
                ("j k", "move"),
                ("1-4", "jump"),
                ("Enter", "choose"),
                ("Esc", "back"),
            ],
        ));
        return lines;
    };
    if mode == BirthdayMode::Unknown {
        let blocks = o
            .tip_for(network)
            .map(|tip| tip.height.saturating_sub(floor));
        return vec![
            Line::styled("Scan the whole chain?", t.text),
            Line::styled(
                format!(
                    "From {}{}. The wallet is usable only once the scan finishes, which can take hours.",
                    format::group(u64::from(floor)),
                    blocks.map_or(String::new(), |b| format!(
                        ", {} blocks",
                        format::group(u64::from(b))
                    ))
                ),
                t.muted,
            ),
            Line::styled(
                "Installed the app this year? \"I know roughly the date\" is faster and safe.",
                t.muted,
            ),
            blank(),
            hint(t, &[("y", "scan everything"), ("n", "choose again")]),
        ];
    }
    let mut lines = vec![
        Line::styled(
            match mode {
                BirthdayMode::FromApp => "Paste the birthday from the app.",
                BirthdayMode::Height => "At which block height was the wallet created?",
                _ => "Around which date was the wallet created?",
            },
            t.text,
        ),
        Line::styled(
            match mode {
                BirthdayMode::FromApp => {
                    "In the app, tap the birthday under the viewing key to copy it.".to_string()
                }
                BirthdayMode::Height => format!(
                    "A number between {} and the chain tip.",
                    format::group(u64::from(floor))
                ),
                _ => {
                    "YYYY-MM or YYYY-MM-DD. The birthday is placed two weeks before it, for safety."
                        .to_string()
                }
            },
            t.muted,
        ),
        blank(),
    ];
    lines.extend(field(t, &o.birthday_input, width));
    if !o.birthday_input.trim().is_empty() {
        lines.push(blank());
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
            Err(e) => lines.push(Line::styled(e, t.warning)),
        }
    } else if mode == BirthdayMode::Date
        && let Some(tip) = o.tip_for(network)
        && let Some(h) = parse_date("2025-01-01")
            .ok()
            .and_then(|d| height_for_date(network, tip, d))
    {
        lines.push(blank());
        lines.push(Line::styled(
            format!(
                "For example, 2025-01-01 gives height {}.",
                format::group(u64::from(h))
            ),
            t.muted,
        ));
    }
    lines.push(blank());
    lines.push(hint(t, &[("Enter", "continue"), ("Esc", "choose again")]));
    lines
}

fn name(o: &Onboarding, t: &Theme, width: usize) -> Vec<Line<'static>> {
    let network = o.target_network();
    let mut lines = vec![
        Line::styled("Name the wallet.", t.text),
        Line::styled(
            format!(
                "A {} {} wallet with birthday {}. The name becomes the file name, <name>.dat, in the data directory.",
                network.name(),
                if o.is_watch() { "watch-only" } else { "seed" },
                o.birthday
                    .map_or("—".to_string(), |b| format::group(u64::from(b)))
            ),
            t.muted,
        ),
        blank(),
    ];
    lines.extend(field(t, &o.name, width));
    if !o.wallets.is_empty() {
        lines.push(blank());
        lines.push(Line::styled(
            format!(
                "In use: {}.",
                o.wallets
                    .iter()
                    .map(|w| w.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            t.muted,
        ));
    }
    lines.push(blank());
    lines.push(hint(t, &[("Enter", "create wallet"), ("Esc", "back")]));
    lines
}
