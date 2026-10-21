//! Renders every phase, screen, and overlay into an in-memory backend. Catches layout panics
//! and checks that the words a user relies on actually appear.

use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

use super::view;
use crate::app::{
    AddressKind, AddressRow, ChainRange, DetailMode, InputRow, Network, OutputRole, OutputRow,
    Overlay, Phase, Pool, RangePriority, Screen, SpendState, State, SyncMode, TxKind, TxRow,
    TxStatus, ValueTransferRow, WalletEntry, WalletInfo, WalletKind,
};
use crate::app::{BirthdayMode, Evidence, KeyMaterial, Onboarding, Step, Tip};
use crate::theme::{ColorDepth, THEMES, index_of};

fn draw(state: &State, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view(frame, state)).unwrap();
    terminal.backend().buffer().clone()
}

fn render(state: &State, width: u16, height: u16) -> String {
    let buffer = draw(state, width, height);
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn populated() -> State {
    let mut s = State::new(Phase::Main, Instant::now());
    s.size = (100, 40);
    s.wallet = Some(WalletInfo {
        network: Network::Stable,
        wallet_name: "main".into(),
        wallet_path: "/tmp/main.dat".into(),
        birthday: 2_500_000,
        server: "https://zec.rocks:443".into(),
        has_orchard: true,
        kind: WalletKind::Watch,
    });
    s.balance.orchard.confirmed = 150_000_000;
    s.balance.transparent.confirmed = 30_000;
    s.balance.staked = 500_000_000;
    s.balance.sapling.pending = 25_000;
    s.sync.tip = Some(2_600_000);
    s.sync.mode = SyncMode::Running;
    s.sync.outputs_total = 1000;
    s.sync.outputs_scanned = 250;
    s.sync.log("session started".into());
    s.txs = vec![
        TxRow {
            txid: "ab".repeat(32),
            datetime: 1_700_000_000,
            height: 2_599_990,
            status: TxStatus::Confirmed(2_599_990),
            kind: TxKind::Received,
            value: 150_000_000,
            fee: None,
            inputs: Vec::new(),
            outputs: vec![OutputRow {
                pool: Pool::Orchard,
                role: OutputRole::Received,
                value: 150_000_000,
                index: 0,
                address: None,
                memo: Some("thanks for the coffee\nsecond line".into()),
                spend: SpendState::Unspent,
                scope: Some("External".into()),
            }],
            value_transfers: vec![ValueTransferRow {
                kind: "received".into(),
                value: 150_000_000,
                recipient: None,
                pool_received: Some("Orchard".into()),
                memos: vec!["thanks for the coffee".into()],
            }],
        },
        TxRow {
            txid: "cd".repeat(32),
            datetime: 1_700_100_000,
            height: 2_600_001,
            status: TxStatus::Pending,
            kind: TxKind::Sent,
            value: 10_000,
            fee: Some(10_000),
            inputs: vec![InputRow {
                pool: Pool::Sapling,
                value: 20_000,
                source_txid: "ab".repeat(32),
                output_index: 3,
                memo: Some("spent note memo".into()),
                pending: false,
            }],
            outputs: vec![OutputRow {
                pool: Pool::Sapling,
                role: OutputRole::Sent,
                value: 10_000,
                index: 1,
                address: Some("u1".to_string() + &"x".repeat(140)),
                memo: None,
                spend: SpendState::Unspent,
                scope: Some("External".into()),
            }],
            value_transfers: Vec::new(),
        },
    ];
    s.addresses = vec![
        AddressRow {
            kind: AddressKind::Unified {
                orchard: true,
                sapling: true,
                transparent: false,
            },
            index: 0,
            address: "u1".to_string() + &"q".repeat(140),
            received: 150_000_000,
        },
        AddressRow {
            kind: AddressKind::Transparent {
                scope: "External".into(),
            },
            index: 0,
            address: "t1".to_string() + &"r".repeat(33),
            received: 0,
        },
    ];
    s
}

#[test]
fn too_small_terminal_says_so() {
    let out = render(&populated(), 60, 20);
    assert!(out.contains("terminal too small"));
}

#[test]
fn every_main_screen_renders() {
    let mut s = populated();
    for screen in [
        Screen::Home,
        Screen::Transactions,
        Screen::Receive,
        Screen::Addresses,
        Screen::Sync,
    ] {
        s.screen = screen;
        let out = render(&s, 100, 40);
        let header = out.lines().next().unwrap();
        for tab in Screen::ALL {
            assert!(
                header.contains(&tab.title().to_lowercase()),
                "{screen:?}: {tab:?} tab"
            );
        }
        assert!(header.contains("syncing 25%"), "{screen:?} sync state");
        assert!(
            header.contains("main"),
            "{screen:?} header names the wallet"
        );
        let narrow = render(&s, 80, 24);
        let narrow_header = narrow.lines().next().unwrap();
        assert!(
            narrow_header.contains("sync "),
            "{screen:?}: every tab fits at 80 columns: {narrow_header}"
        );
        assert!(
            !narrow_header.contains("main"),
            "{screen:?}: the name gives way at 80 columns"
        );
        for noise in ["height", ".dat", "mainnet"] {
            assert!(!header.contains(noise), "{screen:?} header shows {noise}");
        }
    }

    s.screen = Screen::Home;
    let out = render(&s, 100, 40);
    for row in super::bignum::render("1.5006").unwrap() {
        assert!(out.contains(&row), "the balance is drawn in large digits");
    }
    assert!(out.contains("ZEC"));
    assert!(out.contains("0.00025 ZEC pending"));
    for pool in ["orchard 1.5", "sapling 0.00025", "transparent 0.0003"] {
        assert!(
            out.contains(pool),
            "{pool} missing from where the money sits"
        );
    }
    assert!(out.contains("5 ZEC staked"), "bonded funds are shown apart");
    s.balance.transparent.confirmed = 0;
    assert!(
        !render(&s, 100, 40).contains("transparent"),
        "empty pools are left out"
    );
    s.balance.transparent.confirmed = 30_000;
    assert!(out.contains("Recent transactions"));
    assert!(
        out.contains("thanks for the coffee"),
        "recent transactions on home"
    );
    for noise in ["birthday", "server", "zec.rocks"] {
        assert!(!out.contains(noise), "home shows {noise}");
    }

    s.screen = Screen::Transactions;
    let out = render(&s, 100, 40);
    assert!(out.contains("+1.5"));
    assert!(out.contains("thanks for the coffee"));
    assert!(out.contains("pending"));
    assert!(
        !out.contains("confirmed"),
        "settled transactions carry no label"
    );

    s.screen = Screen::Receive;
    let out = render(&s, 100, 40);
    assert!(out.contains("Unified address"));
    assert!(out.contains("orchard and sapling"));
    assert!(
        !out.contains("received"),
        "receive is for getting paid, not history"
    );

    s.screen = Screen::Addresses;
    let out = render(&s, 100, 40);
    assert!(out.contains("transparent"));
    assert!(
        !out.contains("external"),
        "only receiving addresses, so no scope"
    );

    s.screen = Screen::Sync;
    let out = render(&s, 100, 40);
    assert!(out.contains("Syncing"));
    assert!(out.contains("25%"));
    for noise in [
        "batches",
        "outputs",
        "next check",
        "session started",
        "latest block",
    ] {
        assert!(!out.contains(noise), "sync shows {noise}");
    }
}

/// The populated wallet with a scan plan: scanned at the start, being scanned at the tip.
fn planned() -> State {
    let mut s = populated();
    s.screen = Screen::Sync;
    s.sync.birthday = Some(2_500_000);
    s.sync.chain_ranges = vec![
        ChainRange {
            start: 2_500_000,
            end: 2_530_000,
            priority: RangePriority::Scanned,
        },
        ChainRange {
            start: 2_530_000,
            end: 2_560_000,
            priority: RangePriority::Historic,
        },
        ChainRange {
            start: 2_560_000,
            end: 2_580_000,
            priority: RangePriority::Verify,
        },
        ChainRange {
            start: 2_580_000,
            end: 2_600_001,
            priority: RangePriority::Scanning,
        },
    ];
    s
}

/// Leaves the plan in place but nothing in it being scanned.
fn settle(s: &mut State) {
    for range in &mut s.sync.chain_ranges {
        if range.priority == RangePriority::Scanning {
            range.priority = RangePriority::ChainTip;
        }
    }
}

#[test]
fn sync_strip_spans_the_birthday_to_tip() {
    let s = planned();
    let out = render(&s, 100, 40);
    let strip = out.lines().find(|l| l.contains('━')).expect("a strip");
    assert!(
        strip.trim_start_matches(['▏', ' ']).starts_with('━'),
        "scanned from the birthday"
    );
    assert!(strip.contains('─'), "not yet scanned in the middle");
    assert!(out.contains("2,500,000"));
    assert!(out.contains("2,600,000"));
    for glyph in ['▒', '▓', '◆', '▰', '■'] {
        assert!(
            !out.contains(glyph),
            "the strip draws three stages only: {glyph}"
        );
    }
}

#[test]
fn sync_strip_shows_the_live_range_in_the_accent() {
    let s = planned();
    let t = crate::theme::Theme::new(s.theme, s.color_depth);
    let buffer = draw(&s, 100, 40);
    let row = (0..buffer.area.height)
        .find(|y| (0..buffer.area.width).any(|x| buffer[(x, *y)].symbol() == "━"))
        .unwrap();
    // the live range covers the last fifth of the chain
    let last = (0..buffer.area.width)
        .rev()
        .find(|x| buffer[(*x, row)].symbol() != " ")
        .unwrap();
    assert_eq!(buffer[(last, row)].fg, t.accent.fg.unwrap());
}

#[test]
fn m_swaps_the_strip_for_the_chain_map() {
    let mut s = planned();
    s.txs[0].height = 2_510_000;
    s.sync_map = true;
    let out = render(&s, 100, 40);
    assert!(out.contains('■'), "scanned cells");
    assert!(out.contains('·'), "cells not yet scanned");
    assert!(out.contains('◆'), "the transaction at its height");
    assert!(out.contains("transaction"), "a legend");
    assert!(!out.contains('━'), "no strip beside the map");
    let footer = out.lines().nth(39).unwrap();
    assert!(footer.contains("m strip"), "{footer}");
}

#[test]
fn the_chain_map_fits_the_smallest_terminal() {
    let mut s = planned();
    s.sync_map = true;
    s.sync.error = Some("connection refused".into());
    s.sync.last_found = Some(crate::app::FoundTx {
        txid: "cd".repeat(32),
        confirmed: true,
        at: s.now,
    });
    let out = render(&s, super::MIN_WIDTH, super::MIN_HEIGHT);
    assert!(out.contains('■'), "{out}");
    assert!(out.contains("transaction"), "the legend fits: {out}");
    assert!(out.contains("Last found"), "{out}");
}

#[test]
fn sync_states_say_what_to_expect() {
    let mut s = planned();
    s.sync.mode = SyncMode::Paused;
    assert!(render(&s, 100, 40).contains("Nothing is scanned until you resume."));

    s.sync.mode = SyncMode::NotRunning;
    s.sync.sessions_done = 1;
    settle(&mut s);
    let out = render(&s, 100, 40);
    assert!(out.contains("Up to date"));
    assert!(out.contains("block 2,600,000"));
    assert!(out.contains("Watching for new blocks."));

    s.sync.error = Some("connection refused".into());
    s.sync.next_launch = Some(s.now + std::time::Duration::from_secs(30));
    let out = render(&s, 100, 40);
    assert!(out.contains("Sync failed"));
    assert!(out.contains("connection refused"));
    assert!(out.contains("trying again in 30s"));
}

/// Whether the Sync screen looks different one tick later.
fn moves(state: &State) -> bool {
    let mut later = state.clone();
    crate::app::reduce(
        &mut later,
        crate::app::Action::Tick(state.now + std::time::Duration::from_millis(250)),
    );
    render(state, 100, 40) != render(&later, 100, 40)
}

#[test]
fn the_live_range_moves_on_every_tick() {
    let mut s = planned();
    assert!(moves(&s), "the strip");
    s.sync_map = true;
    assert!(moves(&s), "the map");
}

#[test]
fn a_running_sync_moves_even_between_ranges() {
    let mut s = planned();
    settle(&mut s);
    assert!(
        moves(&s),
        "nothing is being scanned, but the sync still runs"
    );
    s.sync.clear_scan_plan();
    assert!(moves(&s), "no plan yet");
}

#[test]
fn nothing_moves_unless_the_sync_runs() {
    let mut s = planned();
    for mode in [SyncMode::Paused, SyncMode::Stopping, SyncMode::NotRunning] {
        s.sync.mode = mode;
        assert!(!moves(&s), "{mode:?}");
    }
}

#[test]
fn the_last_find_sits_on_the_bottom_row() {
    let mut s = planned();
    s.sync.last_found = Some(crate::app::FoundTx {
        txid: "cd".repeat(32),
        confirmed: false,
        at: s.now,
    });
    let out = render(&s, 100, 40);
    let lines: Vec<&str> = out.lines().collect();
    // the body ends above the footer
    assert!(lines[38].contains("Last found"), "{}", lines[38]);
    assert!(lines[38].contains("pending"));
}

#[test]
fn the_header_names_the_chain_and_warns_on_nightly() {
    let mut s = populated();
    assert!(
        render(&s, 100, 40)
            .lines()
            .next()
            .unwrap()
            .contains("stable")
    );
    s.wallet.as_mut().unwrap().network = Network::Nightly;
    let buffer = draw(&s, 100, 40);
    let header: String = (0..100)
        .map(|x| buffer[(x, 0)].symbol().to_string())
        .collect();
    let at = col_of(&header, "nightly");
    assert_ne!(
        buffer[(at, 0)].fg,
        buffer[(0, 1)].fg,
        "nightly is coloured as a warning"
    );
}

#[test]
fn the_active_tab_is_highlighted() {
    let mut s = populated();
    s.theme = index_of("lightwallet").unwrap();
    let accent = Color::Rgb(0xd3, 0xa0, 0x50);
    for screen in Screen::ALL {
        s.screen = screen;
        let buffer = draw(&s, 100, 40);
        let header: String = (0..100)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        // the tabs follow the last separator; "sync" also appears in the sync state
        let tabs_at = header.rfind('·').unwrap();
        let header = format!(
            "{}{}",
            " ".repeat(header[..tabs_at].chars().count()),
            &header[tabs_at..]
        );
        let at = col_of(&header, &screen.title().to_lowercase());
        assert_eq!(
            buffer[(at, 0)].fg,
            accent,
            "{screen:?} tab is not in the accent"
        );
        for other in Screen::ALL.into_iter().filter(|o| *o != screen) {
            let at = col_of(&header, &other.title().to_lowercase());
            assert_ne!(buffer[(at, 0)].fg, accent, "{other:?} highlighted too");
        }
    }
}

#[test]
fn no_screen_shows_a_confirmation_count() {
    let mut s = populated();
    for screen in [
        Screen::Home,
        Screen::Transactions,
        Screen::Receive,
        Screen::Addresses,
        Screen::Sync,
    ] {
        s.screen = screen;
        assert!(
            !render(&s, 100, 40).contains("confirmations"),
            "{screen:?} still counts confirmations"
        );
    }
    s.screen = Screen::Transactions;
    s.overlay = Overlay::TxDetail {
        mode: DetailMode::InputsOutputs,
    };
    let out = render(&s, 100, 40);
    assert!(!out.contains("confirmations"));
    assert!(
        out.contains("confirmed in block 2,599,990"),
        "the height stays"
    );
}

#[test]
fn overlays_render() {
    let mut s = populated();
    for overlay in [
        Overlay::Help,
        Overlay::TxDetail {
            mode: DetailMode::InputsOutputs,
        },
        Overlay::TxDetail {
            mode: DetailMode::ValueTransfers,
        },
        Overlay::RescanConfirm,
        Overlay::NewUnifiedAddress { orchard_only: true },
        Overlay::AddressQr,
        Overlay::ThemePicker { original: 0 },
    ] {
        s.overlay = overlay.clone();
        let out = render(&s, 100, 40);
        match overlay {
            Overlay::Help => assert!(out.contains("delegator-tui — keys")),
            Overlay::TxDetail {
                mode: DetailMode::InputsOutputs,
            } => {
                assert!(out.contains("Inputs and outputs"));
                assert!(
                    out.contains("second line"),
                    "the whole memo, not just its first line"
                );
                assert!(out.contains("index 0"), "output position");
                assert!(out.contains("external"), "output scope");
                assert!(out.contains("unspent"), "spend state");
            }
            Overlay::TxDetail { .. } => assert!(out.contains("Value transfers")),
            Overlay::RescanConfirm => assert!(out.contains("block 2,500,000")),
            Overlay::NewUnifiedAddress { .. } => assert!(out.contains("> orchard only")),
            Overlay::AddressQr => assert!(out.contains("Unified address")),
            Overlay::ThemePicker { .. } => {
                for def in THEMES {
                    assert!(
                        out.contains(def.name),
                        "{} missing from the picker",
                        def.name
                    );
                }
                assert!(out.contains("current"), "the theme Esc restores is marked");
            }
            Overlay::None => unreachable!(),
        }
    }
    // the outgoing address wraps in the detail view instead of being cut
    s.overlay = Overlay::TxDetail {
        mode: DetailMode::InputsOutputs,
    };
    s.tx_selected = 1;
    let out = render(&s, 100, 40);
    assert!(out.contains("xxxxxxxx"));
    assert!(out.contains("fee"));
    // spent wallet outputs are listed, including the shielded pool they came from
    assert!(out.contains("Spent from this wallet"));
    assert!(out.contains("sapling"), "the input's pool");
    assert!(out.contains("spent note memo"), "the input's memo");
    assert!(
        out.contains(":3"),
        "the input's index within its source transaction"
    );
    assert!(
        !out.contains("Received by this wallet"),
        "a section with nothing in it is not shown"
    );
}

#[test]
fn qr_fits_on_a_tall_terminal_and_explains_when_it_does_not() {
    let mut s = populated();
    s.screen = Screen::Receive;
    let tall = render(&s, 100, 60);
    assert!(tall.contains('█') || tall.contains('▀') || tall.contains('▄'));
    let short = render(&s, 100, 24);
    assert!(short.contains("Make the terminal larger to show the QR code."));
    s.ascii_qr = true;
    let ascii = render(&s, 160, 70);
    assert!(
        ascii.contains("##"),
        "ascii modules are two columns wide, so it needs a wide terminal"
    );
    assert!(render(&s, 100, 60).contains("Make the terminal larger"));
}

/// Every onboarding step, with the words a user relies on.
fn onboarding_states() -> Vec<(Step, State)> {
    let now = Instant::now();
    let mut out = Vec::new();
    let base = {
        let mut o = Onboarding::new(Network::Stable, None);
        o.listed = true;
        o.wallets = vec![WalletEntry {
            name: "main".into(),
            network: Network::Stable,
            kind: WalletKind::Seed,
            birthday: Some(2_604_500),
            age_secs: Some(120),
            evidence: Evidence::Meta,
        }];
        o.tip = Some(Tip {
            network: Network::Stable,
            height: 3_000_000,
            at_unix: 1_800_000_000,
            spacing_secs: 75,
        });
        o.error = Some("something to show".into());
        o
    };
    for step in [
        Step::Wallets,
        Step::Menu,
        Step::Create,
        Step::Restore,
        Step::Watch,
        Step::Birthday,
        Step::Name,
    ] {
        let mut o = base.clone();
        o.step = step;
        match step {
            Step::Create => o.seed = Some("abandon ".repeat(23) + "art"),
            Step::Restore => {
                o.words = vec!["abandon".into(); 12];
                o.draft = "ab".into();
                o.birthday_hint = Some(2_604_500);
            }
            Step::Watch => o.key = "uviewtest1".to_string() + &"k".repeat(300) + " 2604500",
            Step::Birthday | Step::Name => {
                o.material = Some((Network::Stable, KeyMaterial::Ufvk("uviewtest1k".into())));
                o.birthday = Some(2_604_500);
                o.name = "watch".into();
            }
            _ => {}
        }
        out.push((step, State::new(Phase::Onboarding(Box::new(o)), now)));
    }
    out
}

#[test]
fn startup_phases_render() {
    let now = Instant::now();
    for (step, s) in onboarding_states() {
        let out = render(&s, 80, 24);
        assert!(out.contains("delegator-tui"), "{step:?}");
        assert!(out.contains("something to show"), "{step:?}");
        match step {
            Step::Wallets => {
                assert!(out.contains("main"));
                assert!(out.contains("2,604,500"));
                assert!(out.contains("2 min ago"));
                assert!(out.contains("New wallet…"));
            }
            Step::Menu => {
                assert!(out.contains("create a new wallet"));
                assert!(out.contains("watch with a viewing key"));
            }
            Step::Create => {
                assert!(out.contains(" 1 abandon"), "{out}");
                assert!(out.contains("24 art"), "{out}");
                assert!(out.contains("2,999,900 on crosslink"), "{out}");
            }
            Step::Restore => {
                assert!(out.contains("12 of 24 words"), "{out}");
                assert!(out.contains("13 ab"), "the draft shows in its slot: {out}");
                assert!(out.contains("birthday 2,604,500 from the paste"));
            }
            Step::Watch => {
                assert!(
                    out.contains("viewing key for the crosslink chains"),
                    "{out}"
                );
                assert!(out.contains("birthday 2,604,500 from the paste"));
            }
            Step::Birthday => {
                assert!(out.contains("I know roughly the date"));
                assert!(out.contains("scan from 1,"), "the floor is block 1: {out}");
            }
            Step::Name => assert!(out.contains("watch")),
        }
    }
    // the birthday input modes
    let (_, mut s) = onboarding_states().remove(5);
    let Phase::Onboarding(o) = &mut s.phase else {
        panic!()
    };
    o.birthday_mode = Some(BirthdayMode::Date);
    o.birthday_input = "2025-01".into();
    let out = render(&s, 80, 24);
    assert!(out.contains("two weeks before it"), "{out}");
    let Phase::Onboarding(o) = &mut s.phase else {
        panic!()
    };
    o.birthday_mode = Some(BirthdayMode::Unknown);
    assert!(render(&s, 80, 24).contains("scan the whole chain?"));
    let Phase::Onboarding(o) = &mut s.phase else {
        panic!()
    };
    o.birthday_mode = Some(BirthdayMode::Height);
    o.birthday_input = "0".into();
    assert!(render(&s, 80, 24).contains("must be at least 1,"));

    let s = State::new(Phase::Loading("Opening wallet".into()), now);
    assert!(render(&s, 80, 24).contains("Opening wallet"));
    let s = State::new(Phase::Fatal("disk on fire".into()), now);
    assert!(render(&s, 80, 24).contains("disk on fire"));
    let mut s = populated();
    s.phase = Phase::ShuttingDown;
    assert!(render(&s, 80, 24).contains("saving the wallet"));
}

#[test]
fn the_wide_restore_grid_has_four_columns_and_the_narrow_one_fewer() {
    let (_, s) = onboarding_states().remove(3);
    let wide = render(&s, 100, 30);
    assert!(wide.contains(" 1 abandon"));
    assert!(
        wide.lines()
            .any(|l| l.contains(" 1 abandon") && l.contains(" 4 abandon")),
        "{wide}"
    );
    let narrow = render(&s, 80, 24);
    assert!(
        narrow
            .lines()
            .any(|l| l.contains(" 1 abandon") && l.contains(" 4 abandon")),
        "80 columns still fit four: {narrow}"
    );
}

#[test]
fn empty_wallet_has_friendly_messages() {
    let mut s = populated();
    s.txs.clear();
    s.addresses.clear();
    s.screen = Screen::Transactions;
    assert!(render(&s, 80, 24).contains("No transactions yet"));
    s.screen = Screen::Home;
    assert!(render(&s, 80, 24).contains("No transactions yet"));
    s.screen = Screen::Receive;
    assert!(render(&s, 80, 24).contains("Press n to make one"));
    s.screen = Screen::Addresses;
    assert!(render(&s, 80, 24).contains("No addresses yet."));
}

#[test]
fn both_detail_views_draw_a_flow_diagram() {
    let mut s = populated();
    let tx = &mut s.txs[1];
    let input = |pool, value, source: &str, output_index| InputRow {
        pool,
        value,
        source_txid: source.repeat(32),
        output_index,
        memo: None,
        pending: false,
    };
    tx.inputs = vec![
        input(Pool::Sapling, 20_000, "ab", 3),
        input(Pool::Orchard, 50_000, "cd", 0),
        input(Pool::Transparent, 100_000, "ef", 1),
    ];
    let output = |pool, role, value, index, scope: &str| OutputRow {
        pool,
        role,
        value,
        index,
        address: None,
        memo: None,
        spend: SpendState::Unspent,
        scope: Some(scope.to_string()),
    };
    tx.outputs = vec![
        output(Pool::Sapling, OutputRole::Sent, 10_000, 0, "External"),
        output(Pool::Orchard, OutputRole::Change, 150_000, 1, "Internal"),
    ];
    let event = |kind: &str, value| ValueTransferRow {
        kind: kind.to_string(),
        value,
        recipient: None,
        pool_received: None,
        memos: Vec::new(),
    };
    tx.value_transfers = vec![
        event("received", 150_000),
        event("sent", 10_000),
        event("shield", 100_000),
    ];
    s.screen = Screen::Transactions;
    s.tx_selected = 1;

    s.overlay = Overlay::TxDetail {
        mode: DetailMode::InputsOutputs,
    };
    let out = render(&s, 100, 30);
    assert!(out.contains("─┼──┤"), "three inputs funnel into the box");
    assert!(out.contains("├──┬─"), "the box forks into two outputs");
    assert!(out.contains("fee 0.0001"), "the box carries the fee");
    assert!(out.contains("change"), "the change output is labelled");
    assert!(
        out.contains("transparent"),
        "every pool reaches the diagram"
    );

    s.overlay = Overlay::TxDetail {
        mode: DetailMode::ValueTransfers,
    };
    let out = render(&s, 100, 30);
    assert!(out.contains("────┤"), "a lone received event needs no bus");
    assert!(out.contains("shield"), "outgoing events leave on the right");

    // the smallest supported terminal still gets a diagram
    s.overlay = Overlay::TxDetail {
        mode: DetailMode::InputsOutputs,
    };
    assert!(render(&s, 80, 24).contains("──┤"));
}

/// Every colour a rendered screen uses, foreground and background.
fn colours(buffer: &Buffer) -> Vec<Color> {
    buffer
        .content()
        .iter()
        .flat_map(|cell| [cell.fg, cell.bg])
        .collect()
}

fn every_view(s: &mut State) -> Vec<String> {
    let mut seen = Vec::new();
    for screen in [
        Screen::Home,
        Screen::Transactions,
        Screen::Receive,
        Screen::Addresses,
        Screen::Sync,
    ] {
        s.screen = screen;
        for overlay in [
            Overlay::None,
            Overlay::Help,
            Overlay::TxDetail {
                mode: DetailMode::InputsOutputs,
            },
            Overlay::TxDetail {
                mode: DetailMode::ValueTransfers,
            },
            Overlay::RescanConfirm,
            Overlay::NewUnifiedAddress {
                orchard_only: false,
            },
            Overlay::AddressQr,
            Overlay::ThemePicker { original: 0 },
        ] {
            s.overlay = overlay;
            seen.push(render(s, 80, 24));
            seen.push(render(s, 120, 50));
        }
    }
    s.overlay = Overlay::None;
    seen
}

#[test]
fn every_theme_draws_every_view_at_both_depths() {
    for depth in [ColorDepth::TrueColor, ColorDepth::Ansi256] {
        for i in 0..THEMES.len() {
            let mut s = populated();
            s.theme = i;
            s.color_depth = depth;
            assert!(!every_view(&mut s).is_empty());
            for (_, mut w) in onboarding_states() {
                w.theme = i;
                w.color_depth = depth;
                assert!(render(&w, 80, 24).contains("delegator-tui"));
            }
        }
    }
}

#[test]
fn a_palette_theme_paints_its_background_and_accent() {
    let mut s = populated();
    s.theme = index_of("gruvbox-dark").unwrap();
    let buffer = draw(&s, 100, 40);
    let gruvbox_bg = Color::Rgb(0x28, 0x28, 0x28);
    assert!(
        buffer.content().iter().all(|cell| cell.bg != Color::Reset),
        "no cell is left on the terminal's own background"
    );
    assert_eq!(
        buffer[(99, 39)].bg,
        gruvbox_bg,
        "even an empty corner is themed"
    );
    let header: String = (0..100)
        .map(|x| buffer[(x, 0)].symbol().to_string())
        .collect();
    let key = col_of(&header, "home");
    assert_eq!(
        buffer[(key, 0)].fg,
        Color::Rgb(0xfe, 0x80, 0x19),
        "the active screen's name is in the accent"
    );

    s.theme = index_of("catppuccin-latte").unwrap();
    assert_eq!(draw(&s, 100, 40)[(99, 39)].bg, Color::Rgb(0xef, 0xf1, 0xf5));
}

#[test]
fn a_256_colour_terminal_never_receives_24_bit_colour() {
    let mut s = populated();
    s.color_depth = ColorDepth::Ansi256;
    for (i, def) in THEMES.iter().enumerate() {
        s.theme = i;
        for screen in [Screen::Home, Screen::Transactions, Screen::Receive] {
            s.screen = screen;
            assert!(
                !colours(&draw(&s, 100, 60))
                    .iter()
                    .any(|c| matches!(c, Color::Rgb(..))),
                "{} leaked 24-bit colour on {screen:?}",
                def.id
            );
        }
    }
}

#[test]
fn monochrome_uses_no_colour_at_all() {
    let mut s = populated();
    s.theme = index_of("mono").unwrap();
    for out in every_view(&mut s) {
        assert!(!out.is_empty());
    }
    for screen in [Screen::Home, Screen::Transactions, Screen::Receive] {
        s.screen = screen;
        assert!(
            colours(&draw(&s, 100, 60))
                .iter()
                .all(|c| *c == Color::Reset),
            "{screen:?} coloured something under NO_COLOR"
        );
    }
}

#[test]
fn qr_codes_are_drawn_in_the_themes_high_contrast_tones() {
    let mut s = populated();
    s.screen = Screen::Receive;
    for id in ["catppuccin-latte", "gruvbox-dark", "nord"] {
        s.theme = index_of(id).unwrap();
        let qr = crate::theme::Theme::new(s.theme, s.color_depth).qr.unwrap();
        let buffer = draw(&s, 100, 60);
        let module = buffer
            .content()
            .iter()
            .find(|cell| matches!(cell.symbol(), "█" | "▀" | "▄"))
            .expect("a QR code is drawn");
        assert_eq!((Some(module.fg), Some(module.bg)), (qr.fg, qr.bg), "{id}");
        assert_ne!(
            module.bg,
            Color::Rgb(255, 255, 255),
            "{id} paper is tinted, not white"
        );
    }
}

#[test]
fn pools_are_told_apart_by_colour() {
    let mut s = populated();
    s.theme = index_of("nord").unwrap();
    s.balance.transparent.confirmed = 70_000;
    let buffer = draw(&s, 100, 40);
    let fg_of = |word: &str| {
        let text = render(&s, 100, 40);
        let row = text
            .lines()
            .position(|l| l.contains("orchard") && l.contains("transparent"))
            .unwrap();
        let col = text.lines().nth(row).unwrap().find(word).unwrap();
        buffer[(col as u16, row as u16)].fg
    };
    let pools = ["orchard", "sapling", "transparent"].map(fg_of);
    for (i, a) in pools.iter().enumerate() {
        for b in &pools[i + 1..] {
            assert_ne!(a, b, "two pools share a colour");
        }
    }
}

#[test]
fn change_is_listed_last_and_left_uncoloured() {
    let mut s = populated();
    s.theme = index_of("gruvbox-dark").unwrap();
    let muted = Color::Rgb(0xa8, 0x99, 0x84);
    let tx = &mut s.txs[1];
    let output = |pool, role, value, index, scope: &str| OutputRow {
        pool,
        role,
        value,
        index,
        address: None,
        memo: None,
        spend: SpendState::Unspent,
        scope: Some(scope.to_string()),
    };
    // change first in the data, to prove the view reorders it
    tx.outputs = vec![
        output(Pool::Orchard, OutputRole::Change, 150_000, 1, "Internal"),
        output(Pool::Sapling, OutputRole::Sent, 10_000, 0, "External"),
    ];
    s.screen = Screen::Transactions;
    s.tx_selected = 1;
    s.overlay = Overlay::TxDetail {
        mode: DetailMode::InputsOutputs,
    };
    let buffer = draw(&s, 100, 40);
    let text = render(&s, 100, 40);
    let lines: Vec<&str> = text.lines().collect();
    // the colour of the first cell of `word` on `row`
    let fg_at = |row: usize, word: &str| {
        let byte = lines[row].find(word).unwrap();
        let col = lines[row][..byte].chars().count() as u16;
        buffer[(col, row as u16)].fg
    };

    // in the diagram, the payment comes first in its pool colour; change follows, muted
    let paid = lines
        .iter()
        .position(|l| l.contains("sapling") && l.contains("sent"))
        .unwrap();
    let back = lines
        .iter()
        .position(|l| l.contains("orchard") && l.contains("change"))
        .unwrap();
    assert!(paid < back, "change is drawn after the payment");
    assert_eq!(
        fg_at(paid, "0.0001"),
        Color::Rgb(0xd6, 0x5d, 0x0e),
        "payment in sapling colour"
    );
    assert_eq!(fg_at(back, "0.0015"), muted, "change is not coloured");

    // in the lists, change has its own section after the payments, not under received
    assert!(
        !lines.iter().any(|l| l.contains("Received by this wallet")),
        "change is not a receipt"
    );
    let sent = lines
        .iter()
        .position(|l| l.contains("Sent to others"))
        .unwrap();
    let change = lines
        .iter()
        .position(|l| l.trim_start_matches('▏').trim() == "Change")
        .unwrap();
    assert!(sent < change, "change comes last");
    assert_eq!(
        fg_at(change + 1, "orchard"),
        muted,
        "the change line is muted"
    );

    // a transaction without change has no change section
    s.txs[1].outputs.retain(|o| o.role == OutputRole::Sent);
    assert!(
        !render(&s, 100, 40)
            .lines()
            .any(|l| l.trim_start_matches('▏').trim() == "Change")
    );
}

/// The cell column where `needle` starts in a row of symbols. Byte offsets would be off by the
/// multi-byte glyphs before it.
fn col_of(row: &str, needle: &str) -> u16 {
    let at = row.find(needle).expect(needle);
    row[..at].chars().count() as u16
}

/// Column where the first line containing `needle` starts, counted in cells. The pane's rule
/// in the first column is blank for this purpose.
fn indent_of(out: &str, needle: &str) -> usize {
    let line = out.lines().find(|l| l.contains(needle)).expect(needle);
    blank_prefix(line)
}

fn blank_prefix(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '▏').count()
}

/// Every piece of content starts at the same left edge.
const EDGE: usize = super::EDGE as usize;

#[test]
fn empty_states_start_at_the_content_edge_not_the_centre() {
    let mut s = populated();
    s.txs.clear();
    s.addresses.clear();
    for (screen, message) in [
        (Screen::Home, "No transactions yet"),
        (Screen::Transactions, "No transactions yet"),
        (Screen::Receive, "No unified address yet"),
        (Screen::Addresses, "No addresses yet"),
    ] {
        s.screen = screen;
        assert_eq!(indent_of(&render(&s, 120, 30), message), EDGE, "{screen:?}");
    }
}

#[test]
fn the_transaction_diagram_starts_at_the_content_edge() {
    let mut s = populated();
    s.screen = Screen::Transactions;
    s.tx_selected = 1;
    for width in [80, 120, 200] {
        for mode in [DetailMode::InputsOutputs, DetailMode::ValueTransfers] {
            s.overlay = Overlay::TxDetail { mode };
            let out = render(&s, width, 40);
            let diagram = out
                .lines()
                .filter(|l| l.contains(['┌', '│', '┤', '├', '└']));
            let edge = diagram.map(blank_prefix).min().expect("a diagram is drawn");
            assert_eq!(edge, EDGE, "{mode:?} at {width} columns");
            assert_eq!(
                indent_of(&out, "amount"),
                EDGE,
                "the summary shares the edge"
            );
        }
    }
}

#[test]
fn the_qr_code_stays_beside_its_address_on_a_wide_terminal() {
    let mut s = populated();
    s.screen = Screen::Receive;
    let out = render(&s, 220, 50);
    let qr_column = out
        .lines()
        .filter_map(|l| l.chars().position(|c| matches!(c, '█' | '▀' | '▄')))
        .min()
        .expect("a QR code is drawn");
    assert!(qr_column < 70, "the QR code drifted to column {qr_column}");
    assert_eq!(indent_of(&out, "Unified address"), EDGE);
}

#[test]
fn the_chrome_sits_on_the_left_edge_and_content_two_columns_in() {
    let mut s = populated();
    let (w, h) = (100u16, 30u16);
    let out = render(&s, w, h);
    let lines: Vec<&str> = out.lines().collect();
    // the title starts in the first column, the body rows carry the rule there
    assert!(lines[0].starts_with("delegator-tui"));
    for row in lines.iter().take(usize::from(h) - 1).skip(1) {
        assert!(row.starts_with('▏'), "{row}");
    }
    // the pane title sits on the rule's first row
    assert!(lines[1].starts_with("▏home"), "{}", lines[1]);
    // the footer's first key and the body's first word share the content edge, less the rule
    let footer = lines[usize::from(h) - 1];
    assert!(footer.starts_with("j/k move"), "{footer}");
    assert_eq!(indent_of(&out, "Balance"), EDGE);
    // a notice takes the footer over
    s.set_notice("copied");
    let out = render(&s, w, h);
    assert!(out.lines().last().unwrap().starts_with("copied"));
}

/// A delegation bond: 0.0102 ZEC of Orchard locked into a bond, with a zero-value Orchard
/// padding output. zingolib reports it as a bond creation worth 0.01, with one value transfer
/// carrying that amount.
fn bond() -> State {
    let mut s = populated();
    let tx = &mut s.txs[1];
    tx.kind = TxKind::CreateBond;
    tx.value = 1_000_000;
    tx.fee = Some(20_000);
    tx.inputs = vec![InputRow {
        pool: Pool::Orchard,
        value: 1_020_000,
        source_txid: "ab".repeat(32),
        output_index: 0,
        memo: None,
        pending: false,
    }];
    tx.outputs = vec![OutputRow {
        pool: Pool::Orchard,
        role: OutputRole::Change,
        value: 0,
        index: 1,
        address: None,
        memo: None,
        spend: SpendState::Unspent,
        scope: Some("Internal".into()),
    }];
    tx.value_transfers = vec![ValueTransferRow {
        kind: "bond created".into(),
        value: 1_000_000,
        recipient: None,
        pool_received: None,
        memos: Vec::new(),
    }];
    s.screen = Screen::Transactions;
    s.tx_selected = 1;
    s
}

#[test]
fn a_bond_names_itself_and_never_says_sent() {
    let mut s = bond();
    s.overlay = Overlay::TxDetail {
        mode: DetailMode::InputsOutputs,
    };
    let out = render(&s, 100, 40);
    let diagram: Vec<&str> = out
        .lines()
        .filter(|l| l.contains(['┌', '│', '┤', '├', '└']))
        .collect();
    assert!(
        diagram.iter().any(|l| l.contains("bond created")),
        "the box names the action"
    );
    assert!(
        diagram
            .iter()
            .any(|l| l.contains("orchard") && l.contains("change")),
        "the zero padding is change"
    );
    assert!(
        !diagram.iter().any(|l| l.contains("sent")),
        "nothing that stays in the wallet is called sent"
    );
    assert!(!out.contains("Sent to others"));

    // the list names it too, with the bonded amount and no sign
    s.overlay = Overlay::None;
    let list = render(&s, 100, 40);
    let row = list.lines().find(|l| l.contains("bond created")).unwrap();
    assert!(row.contains("0.01") && !row.contains("-0.01") && !row.contains("+0.01"));
}

#[test]
fn value_transfers_show_their_amounts() {
    let mut s = bond();
    s.overlay = Overlay::TxDetail {
        mode: DetailMode::ValueTransfers,
    };
    let out = render(&s, 100, 40);
    assert!(out.contains("bond created"));
    assert!(
        out.contains("0.01000000 ZEC"),
        "the bonded amount, not zero"
    );
    assert!(!out.contains("memo to self"));
}

#[test]
fn a_same_pool_send_to_self_keeps_its_name() {
    let mut s = bond();
    s.txs[1].kind = TxKind::SentToSelf;
    s.overlay = Overlay::None;
    assert!(render(&s, 100, 40).contains("sent to self"));
}

#[test]
fn the_kind_and_the_memo_each_have_their_own_column() {
    let mut s = populated();
    for screen in [Screen::Home, Screen::Transactions] {
        s.screen = screen;
        let out = render(&s, 100, 40);
        let rows: Vec<&str> = out.lines().filter(|l| l.contains("2023-11-1")).collect();
        let received = rows
            .iter()
            .find(|l| l.contains("thanks"))
            .expect("the row with a memo");
        let sent = rows
            .iter()
            .find(|l| l.contains("pending"))
            .expect("the row without one");
        // the row with a memo still says what kind of transaction it was
        assert!(received.contains("received"), "{screen:?}: {received}");
        let col = |row: &str, word: &str| row[..row.find(word).unwrap()].chars().count();
        assert_eq!(
            col(received, "received"),
            col(sent, "sent"),
            "{screen:?}: kinds share a column"
        );
        assert!(
            col(received, "thanks") > col(received, "received") + "received".len(),
            "{screen:?}: the memo sits in its own column after the kind"
        );
    }
}
