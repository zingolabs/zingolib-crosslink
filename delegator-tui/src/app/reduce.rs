//! The reducer: folds one [`Action`] into the [`State`] and returns the [`Command`]s to run.

use std::time::Instant;

use super::action::{Action, Command, Key, SyncNote};
use super::onboarding;
use super::state::{DetailMode, FoundTx, Overlay, Phase, ReceiveKind, Screen, State};
use crate::format;

pub fn reduce(state: &mut State, action: Action) -> Vec<Command> {
    match action {
        Action::Tick(now) => {
            state.now = now;
            state.ticks = state.ticks.wrapping_add(1);
            Vec::new()
        }
        Action::Resize(w, h) => {
            state.size = (w, h);
            Vec::new()
        }
        Action::Key(key) => on_key(state, key),
        Action::Paste(text) => on_paste(state, &text),
        Action::WalletsListed(wallets) => onboarding::wallets_listed(state, wallets),
        Action::TipFetched(result) => {
            onboarding::tip_fetched(state, result);
            Vec::new()
        }
        Action::SeedGenerated(phrase) => {
            onboarding::seed_generated(state, phrase);
            Vec::new()
        }
        Action::WalletOpened(info) => {
            state.sync.error = None;
            state.wallet = Some(info);
            state.onboarding_backup = None;
            state.phase = Phase::Main;
            state.screen = Screen::Home;
            state.overlay = Overlay::None;
            vec![Command::Refresh]
        }
        Action::OpenFailed(message) => {
            onboarding::open_failed(state, message);
            Vec::new()
        }
        Action::DataLoaded(data) => {
            state.balance = data.balance;
            state.txs = data.txs;
            state.addresses = data.addresses;
            clamp_selections(state);
            Vec::new()
        }
        Action::Sync(note, at) => {
            on_sync(state, note, at);
            Vec::new()
        }
        Action::Notice(message) => {
            state.set_notice(message);
            Vec::new()
        }
        Action::Copied(ok) => {
            state.set_notice(if ok {
                "copied to clipboard"
            } else {
                "clipboard copy failed"
            });
            Vec::new()
        }
        Action::Shutdown => {
            state.exit = true;
            Vec::new()
        }
    }
}

fn clamp_selections(state: &mut State) {
    state.tx_selected = state.tx_selected.min(state.txs.len().saturating_sub(1));
    state.addr_selected = state
        .addr_selected
        .min(state.addresses.len().saturating_sub(1));
}

fn on_sync(state: &mut State, note: SyncNote, at: Instant) {
    let sync = &mut state.sync;
    match note {
        SyncNote::SessionStarted {
            tip,
            birthday,
            total_outputs,
            scanned_outputs,
            scanned_blocks,
        } => {
            sync.birthday = Some(birthday);
            sync.tip = Some(tip);
            sync.outputs_total = total_outputs;
            sync.outputs_scanned = scanned_outputs;
            sync.blocks_scanned = scanned_blocks;
            sync.error = None;
            sync.next_launch = None;
            sync.progress_log.clear();
            sync.begin_session();
            sync.record_progress(at);
            sync.log(format!(
                "session started at tip {}, {} of {} outputs already scanned",
                format::group(u64::from(tip)),
                format::group(scanned_outputs),
                format::group(total_outputs),
            ));
        }
        SyncNote::ScanPlanUpdated { ranges } => sync.set_scan_plan(ranges),
        SyncNote::TxDiscovered { txid, confirmed } => {
            sync.txs_found += 1;
            let status = if confirmed { "confirmed" } else { "pending" };
            sync.log(format!(
                "transaction {status}: {}",
                format::truncate_middle(&txid, 24)
            ));
            sync.last_found = Some(FoundTx {
                txid,
                confirmed,
                at,
            });
        }
        SyncNote::TipMoved { to } => {
            sync.tip = Some(to);
            sync.log(format!("new block {}", format::group(u64::from(to))));
        }
        SyncNote::Progress {
            total_outputs,
            scanned_outputs,
            scanned_blocks,
        } => {
            if total_outputs > 0 {
                sync.outputs_total = total_outputs;
            }
            sync.outputs_scanned = scanned_outputs;
            sync.blocks_scanned = scanned_blocks;
            sync.record_progress(at);
        }
        SyncNote::Finished { blocks_scanned } => {
            sync.sessions_done += 1;
            sync.outputs_scanned = sync.outputs_total;
            sync.error = None;
            sync.log(format!(
                "session finished, {} blocks scanned",
                format::group(blocks_scanned)
            ));
        }
        SyncNote::Failed(message) => {
            sync.log(format!("sync failed: {message}"));
            sync.error = Some(message);
        }
        SyncNote::Mode(mode) => {
            sync.mode = mode;
        }
        SyncNote::Auto(auto) => {
            sync.auto = auto;
            if !auto {
                sync.next_launch = None;
            }
        }
        SyncNote::Scheduled { at } => {
            sync.next_launch = Some(at);
        }
        SyncNote::Reset => {
            sync.reset_progress();
            sync.txs_found = 0;
            sync.last_found = None;
            sync.sessions_done = 0;
            sync.error = None;
            sync.log("rescan started from birthday".into());
        }
    }
}

fn on_paste(state: &mut State, text: &str) -> Vec<Command> {
    if matches!(state.phase, Phase::Onboarding(_)) {
        return onboarding::paste(state, text);
    }
    Vec::new()
}

fn on_key(state: &mut State, key: Key) -> Vec<Command> {
    match &state.phase {
        Phase::Fatal(_) => {
            if matches!(key, Key::Char('q') | Key::Esc | Key::Enter | Key::CtrlC) {
                state.exit = true;
                return vec![Command::ForceQuit];
            }
            Vec::new()
        }
        Phase::Loading(_) => {
            if matches!(key, Key::CtrlC) {
                state.exit = true;
                return vec![Command::ForceQuit];
            }
            Vec::new()
        }
        Phase::ShuttingDown => {
            if matches!(key, Key::Char('q') | Key::CtrlC) {
                state.exit = true;
                return vec![Command::ForceQuit];
            }
            Vec::new()
        }
        Phase::Onboarding(_) => onboarding::key(state, key),
        Phase::Main => main_key(state, key),
    }
}

fn request_quit(state: &mut State) -> Vec<Command> {
    if state.quit_requested {
        state.exit = true;
        return vec![Command::ForceQuit];
    }
    state.quit_requested = true;
    state.phase = Phase::ShuttingDown;
    vec![Command::Quit]
}

fn main_key(state: &mut State, key: Key) -> Vec<Command> {
    if matches!(key, Key::CtrlC | Key::Char('q')) {
        return request_quit(state);
    }
    match state.overlay.clone() {
        Overlay::Help => {
            if matches!(key, Key::Esc | Key::Char('?')) {
                state.overlay = Overlay::None;
            }
            Vec::new()
        }
        Overlay::TxDetail { mode } => {
            match key {
                Key::Esc => {
                    state.overlay = Overlay::None;
                    state.detail_scroll = 0;
                }
                Key::Char('v') => {
                    state.overlay = Overlay::TxDetail {
                        mode: match mode {
                            DetailMode::InputsOutputs => DetailMode::ValueTransfers,
                            DetailMode::ValueTransfers => DetailMode::InputsOutputs,
                        },
                    };
                    state.detail_scroll = 0;
                }
                Key::Down | Key::Char('j') => {
                    state.detail_scroll = state.detail_scroll.saturating_add(1)
                }
                Key::Up | Key::Char('k') => {
                    state.detail_scroll = state.detail_scroll.saturating_sub(1)
                }
                Key::PageDown => state.detail_scroll = state.detail_scroll.saturating_add(10),
                Key::PageUp => state.detail_scroll = state.detail_scroll.saturating_sub(10),
                Key::Char('y') => {
                    if let Some(tx) = state.selected_tx() {
                        return vec![Command::Copy(tx.txid.clone())];
                    }
                }
                _ => {}
            }
            Vec::new()
        }
        Overlay::RescanConfirm => {
            match key {
                Key::Char('y') => {
                    state.overlay = Overlay::None;
                    return vec![Command::Rescan];
                }
                Key::Char('n') | Key::Esc => state.overlay = Overlay::None,
                _ => {}
            }
            Vec::new()
        }
        Overlay::NewUnifiedAddress { orchard_only } => {
            match key {
                Key::Up | Key::Down | Key::Char('j') | Key::Char('k') => {
                    state.overlay = Overlay::NewUnifiedAddress {
                        orchard_only: !orchard_only,
                    };
                }
                Key::Enter => {
                    state.overlay = Overlay::None;
                    return vec![Command::NewUnifiedAddress {
                        orchard: true,
                        sapling: !orchard_only,
                    }];
                }
                Key::Esc => state.overlay = Overlay::None,
                _ => {}
            }
            Vec::new()
        }
        Overlay::AddressQr => {
            match key {
                Key::Esc => state.overlay = Overlay::None,
                Key::Char('y') => {
                    if let Some(a) = state.selected_address() {
                        return vec![Command::Copy(a.address.clone())];
                    }
                }
                _ => {}
            }
            Vec::new()
        }
        Overlay::ThemePicker { original } => {
            let count = crate::theme::THEMES.len();
            match key {
                Key::Down | Key::Char('j') => state.theme = (state.theme + 1) % count,
                Key::Up | Key::Char('k') => state.theme = (state.theme + count - 1) % count,
                Key::Home | Key::Char('g') => state.theme = 0,
                Key::End | Key::Char('G') => state.theme = count - 1,
                Key::Enter => {
                    state.overlay = Overlay::None;
                    let def = &crate::theme::THEMES[state.theme];
                    state.set_notice(format!("theme set to {}", def.name));
                    return vec![Command::SaveTheme(def.id)];
                }
                Key::Esc => {
                    state.theme = original;
                    state.overlay = Overlay::None;
                }
                _ => {}
            }
            Vec::new()
        }
        Overlay::None => screen_key(state, key),
    }
}

fn screen_key(state: &mut State, key: Key) -> Vec<Command> {
    match key {
        Key::Char('?') => {
            state.overlay = Overlay::Help;
            return Vec::new();
        }
        Key::Char('c') => {
            state.overlay = Overlay::ThemePicker {
                original: state.theme,
            };
            return Vec::new();
        }
        // each tab's key is its first letter, and works from every screen
        Key::Char('h') => {
            state.screen = Screen::Home;
            return Vec::new();
        }
        Key::Char('t') => {
            state.screen = Screen::Transactions;
            return Vec::new();
        }
        Key::Char('r') => {
            state.screen = Screen::Receive;
            return Vec::new();
        }
        Key::Char('a') => {
            state.screen = Screen::Addresses;
            return Vec::new();
        }
        Key::Char('s') => {
            state.screen = Screen::Sync;
            return Vec::new();
        }
        Key::Esc => {
            state.screen = Screen::Home;
            return Vec::new();
        }
        _ => {}
    }
    match state.screen {
        // Home's recent transactions are the same list, so they move and open the same way
        Screen::Home | Screen::Transactions => {
            let len = state.txs.len();
            move_selection(&mut state.tx_selected, len, key, state.size.1);
            if matches!(key, Key::Enter) && len > 0 {
                state.overlay = Overlay::TxDetail {
                    mode: DetailMode::InputsOutputs,
                };
                state.detail_scroll = 0;
            }
            Vec::new()
        }
        Screen::Receive => match key {
            Key::Char('v') => {
                state.receive_kind = match state.receive_kind {
                    ReceiveKind::Unified => ReceiveKind::Transparent,
                    ReceiveKind::Transparent => ReceiveKind::Unified,
                };
                Vec::new()
            }
            Key::Char('n') => new_address(state, state.receive_kind == ReceiveKind::Unified),
            Key::Char('y') => match state.receive_address() {
                Some(a) => vec![Command::Copy(a.address.clone())],
                None => Vec::new(),
            },
            _ => Vec::new(),
        },
        Screen::Addresses => {
            let len = state.addresses.len();
            move_selection(&mut state.addr_selected, len, key, state.size.1);
            match key {
                Key::Enter if len > 0 => {
                    state.overlay = Overlay::AddressQr;
                    Vec::new()
                }
                Key::Char('n') => {
                    let unified = state.selected_address().is_none_or(|a| a.kind.is_unified());
                    new_address(state, unified)
                }
                Key::Char('y') => match state.selected_address() {
                    Some(a) => vec![Command::Copy(a.address.clone())],
                    None => Vec::new(),
                },
                _ => Vec::new(),
            }
        }
        Screen::Sync => match key {
            Key::Char('p') => match state.sync.p_pauses() {
                Some(true) => vec![Command::PauseSync],
                Some(false) => vec![Command::ResumeSync],
                None => Vec::new(),
            },
            Key::Char('m') => {
                state.sync_map = !state.sync_map;
                Vec::new()
            }
            Key::Char('R') => {
                state.overlay = Overlay::RescanConfirm;
                Vec::new()
            }
            _ => Vec::new(),
        },
    }
}

fn new_address(state: &mut State, unified: bool) -> Vec<Command> {
    if !unified {
        return vec![Command::NewTransparentAddress];
    }
    if state.has_orchard() {
        state.overlay = Overlay::NewUnifiedAddress {
            orchard_only: false,
        };
        Vec::new()
    } else {
        vec![Command::NewUnifiedAddress {
            orchard: false,
            sapling: true,
        }]
    }
}

/// Moves a list cursor. `rows` is the terminal height, used for page jumps.
fn move_selection(selected: &mut usize, len: usize, key: Key, rows: u16) {
    if len == 0 {
        *selected = 0;
        return;
    }
    let last = len - 1;
    let page = usize::from(rows.saturating_sub(6).max(1));
    *selected = match key {
        Key::Down | Key::Char('j') => (*selected + 1).min(last),
        Key::Up | Key::Char('k') => selected.saturating_sub(1),
        Key::PageDown => (*selected + page).min(last),
        Key::PageUp => selected.saturating_sub(page),
        Key::Home | Key::Char('g') => 0,
        Key::End | Key::Char('G') => last,
        _ => *selected,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::{
        AddressKind, AddressRow, ChainRange, Network, RangePriority, Stage, SyncMode, TxKind,
        TxRow, TxStatus, WalletData, WalletInfo, WalletKind,
    };
    use std::time::Duration;

    fn now() -> Instant {
        Instant::now()
    }

    fn main_state() -> State {
        let mut s = State::new(Phase::Main, now());
        s.wallet = Some(WalletInfo {
            network: Network::Stable,
            wallet_name: "w.dat".into(),
            wallet_path: "/tmp/w.dat".into(),
            birthday: 100,
            server: "https://zec.rocks:443".into(),
            has_orchard: true,
            kind: WalletKind::Watch,
        });
        s
    }

    fn tx(i: u32) -> TxRow {
        TxRow {
            txid: format!("txid{i}"),
            datetime: 1_700_000_000 + i,
            height: 1000 + i,
            status: TxStatus::Confirmed(1000 + i),
            kind: TxKind::Received,
            value: 1000,
            fee: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            value_transfers: Vec::new(),
        }
    }

    fn keys(state: &mut State, keys: &[Key]) -> Vec<Command> {
        keys.iter()
            .flat_map(|k| reduce(state, Action::Key(*k)))
            .collect()
    }

    #[test]
    fn screen_keys_navigate_and_esc_returns_to_overview() {
        let mut s = main_state();
        keys(&mut s, &[Key::Char('t')]);
        assert_eq!(s.screen, Screen::Transactions);
        keys(&mut s, &[Key::Char('s')]);
        assert_eq!(s.screen, Screen::Sync);
        keys(&mut s, &[Key::Esc]);
        assert_eq!(s.screen, Screen::Home);
        assert!(!s.exit);
        keys(&mut s, &[Key::Char('r'), Key::Char('t')]);
        assert_eq!(
            s.screen,
            Screen::Transactions,
            "t switches tabs even from Receive"
        );
        keys(&mut s, &[Key::Char('s'), Key::Char('r')]);
        assert_eq!(s.screen, Screen::Receive, "r switches tabs even from Sync");
        keys(&mut s, &[Key::Char('h')]);
        assert_eq!(s.screen, Screen::Home);
    }

    #[test]
    fn q_quits_once_and_forces_on_second_press() {
        let mut s = main_state();
        let cmds = keys(&mut s, &[Key::Char('q')]);
        assert_eq!(cmds, vec![Command::Quit]);
        assert_eq!(s.phase, Phase::ShuttingDown);
        assert!(!s.exit);
        let cmds = keys(&mut s, &[Key::Char('q')]);
        assert_eq!(cmds, vec![Command::ForceQuit]);
        assert!(s.exit);
    }

    #[test]
    fn shutdown_from_backend_exits() {
        let mut s = main_state();
        keys(&mut s, &[Key::Char('q')]);
        reduce(&mut s, Action::Shutdown);
        assert!(s.exit);
    }

    #[test]
    fn help_opens_and_closes_with_esc() {
        let mut s = main_state();
        keys(&mut s, &[Key::Char('?')]);
        assert_eq!(s.overlay, Overlay::Help);
        keys(&mut s, &[Key::Char('t')]);
        assert_eq!(s.screen, Screen::Home, "keys are swallowed under help");
        keys(&mut s, &[Key::Esc]);
        assert_eq!(s.overlay, Overlay::None);
    }

    #[test]
    fn transaction_list_selection_and_detail() {
        let mut s = main_state();
        reduce(
            &mut s,
            Action::DataLoaded(WalletData {
                txs: (0..5).map(tx).collect(),
                ..Default::default()
            }),
        );
        keys(
            &mut s,
            &[
                Key::Char('t'),
                Key::Char('j'),
                Key::Char('j'),
                Key::Char('k'),
            ],
        );
        assert_eq!(s.tx_selected, 1);
        keys(&mut s, &[Key::Char('G')]);
        assert_eq!(s.tx_selected, 4);
        keys(&mut s, &[Key::Char('j')]);
        assert_eq!(s.tx_selected, 4, "clamped at the end");
        keys(&mut s, &[Key::Enter]);
        assert_eq!(
            s.overlay,
            Overlay::TxDetail {
                mode: DetailMode::InputsOutputs
            }
        );
        keys(&mut s, &[Key::Char('v')]);
        assert_eq!(
            s.overlay,
            Overlay::TxDetail {
                mode: DetailMode::ValueTransfers
            }
        );
        keys(&mut s, &[Key::Esc]);
        assert_eq!(s.overlay, Overlay::None);
        assert_eq!(s.screen, Screen::Transactions, "esc closes the detail only");
    }

    #[test]
    fn data_reload_clamps_selection() {
        let mut s = main_state();
        reduce(
            &mut s,
            Action::DataLoaded(WalletData {
                txs: (0..5).map(tx).collect(),
                ..Default::default()
            }),
        );
        s.tx_selected = 4;
        reduce(
            &mut s,
            Action::DataLoaded(WalletData {
                txs: (0..2).map(tx).collect(),
                ..Default::default()
            }),
        );
        assert_eq!(s.tx_selected, 1);
    }

    #[test]
    fn receive_screen_toggles_kind_and_derives_addresses() {
        let mut s = main_state();
        keys(&mut s, &[Key::Char('r')]);
        assert_eq!(s.screen, Screen::Receive);
        let cmds = keys(&mut s, &[Key::Char('n')]);
        assert!(cmds.is_empty());
        assert_eq!(
            s.overlay,
            Overlay::NewUnifiedAddress {
                orchard_only: false
            }
        );
        keys(&mut s, &[Key::Char('j')]);
        let cmds = keys(&mut s, &[Key::Enter]);
        assert_eq!(
            cmds,
            vec![Command::NewUnifiedAddress {
                orchard: true,
                sapling: false
            }]
        );
        keys(&mut s, &[Key::Char('v')]);
        assert_eq!(s.receive_kind, ReceiveKind::Transparent);
        let cmds = keys(&mut s, &[Key::Char('n')]);
        assert_eq!(cmds, vec![Command::NewTransparentAddress]);
    }

    #[test]
    fn sapling_only_wallet_skips_receiver_picker() {
        let mut s = main_state();
        s.wallet.as_mut().unwrap().has_orchard = false;
        keys(&mut s, &[Key::Char('r')]);
        let cmds = keys(&mut s, &[Key::Char('n')]);
        assert_eq!(
            cmds,
            vec![Command::NewUnifiedAddress {
                orchard: false,
                sapling: true
            }]
        );
    }

    #[test]
    fn receive_address_is_newest_of_kind() {
        let mut s = main_state();
        let row = |kind, index| AddressRow {
            kind,
            index,
            address: format!("addr{index}"),
            received: 0,
        };
        let unified = |i| {
            row(
                AddressKind::Unified {
                    orchard: true,
                    sapling: true,
                    transparent: false,
                },
                i,
            )
        };
        s.addresses = vec![
            unified(0),
            unified(2),
            unified(1),
            row(
                AddressKind::Transparent {
                    scope: "External".into(),
                },
                0,
            ),
            row(
                AddressKind::Transparent {
                    scope: "Internal".into(),
                },
                7,
            ),
        ];
        assert_eq!(s.receive_address().unwrap().index, 2);
        s.receive_kind = ReceiveKind::Transparent;
        assert_eq!(s.receive_address().unwrap().index, 0);
        s.screen = Screen::Receive;
        let cmds = keys(&mut s, &[Key::Char('y')]);
        assert_eq!(cmds, vec![Command::Copy("addr0".into())]);
    }

    #[test]
    fn sync_screen_pause_resume_and_rescan() {
        let mut s = main_state();
        keys(&mut s, &[Key::Char('s')]);
        s.sync.mode = SyncMode::Running;
        assert_eq!(keys(&mut s, &[Key::Char('p')]), vec![Command::PauseSync]);
        s.sync.mode = SyncMode::Paused;
        assert_eq!(keys(&mut s, &[Key::Char('p')]), vec![Command::ResumeSync]);
        s.sync.mode = SyncMode::NotRunning;
        s.sync.auto = false;
        assert_eq!(keys(&mut s, &[Key::Char('p')]), vec![Command::ResumeSync]);
        keys(&mut s, &[Key::Char('R')]);
        assert_eq!(s.overlay, Overlay::RescanConfirm);
        assert!(keys(&mut s, &[Key::Char('n')]).is_empty());
        keys(&mut s, &[Key::Char('R')]);
        assert_eq!(keys(&mut s, &[Key::Char('y')]), vec![Command::Rescan]);
        assert_eq!(s.overlay, Overlay::None);
        assert!(keys(&mut s, &[Key::Char('m')]).is_empty());
        assert!(s.sync_map);
        keys(&mut s, &[Key::Char('m')]);
        assert!(!s.sync_map);
    }

    #[test]
    fn sync_progress_and_eta() {
        let mut s = main_state();
        let t0 = now();
        reduce(
            &mut s,
            Action::Sync(
                SyncNote::SessionStarted {
                    tip: 2_000_000,
                    birthday: 1,
                    total_outputs: 1000,
                    scanned_outputs: 100,
                    scanned_blocks: 5,
                },
                t0,
            ),
        );
        assert!((s.sync.fraction() - 0.1).abs() < 1e-9);
        assert_eq!(s.sync.tip, Some(2_000_000));
        reduce(
            &mut s,
            Action::Sync(
                SyncNote::ScanPlanUpdated {
                    ranges: vec![ChainRange {
                        start: 1,
                        end: 1_000_000,
                        priority: RangePriority::Historic,
                    }],
                },
                t0,
            ),
        );
        assert_eq!(s.sync.stage(100, 200), Some(Stage::NotYet));
        assert_eq!(s.sync.stage(1_000_000, 1_000_100), None, "outside the plan");
        reduce(
            &mut s,
            Action::Sync(
                SyncNote::Progress {
                    total_outputs: 1000,
                    scanned_outputs: 500,
                    scanned_blocks: 15,
                },
                t0 + Duration::from_secs(2),
            ),
        );
        assert!((s.sync.fraction() - 0.5).abs() < 1e-9);
        assert_eq!(s.sync.blocks_scanned, 15);
        let eta = s.sync.eta_secs().expect("rate known after two samples");
        assert!((2..=3).contains(&eta), "500 left at 200/s, got {eta}");
        // a range the engine is scanning now shows as live
        reduce(
            &mut s,
            Action::Sync(
                SyncNote::ScanPlanUpdated {
                    ranges: vec![
                        ChainRange {
                            start: 1,
                            end: 500_000,
                            priority: RangePriority::Scanned,
                        },
                        ChainRange {
                            start: 500_000,
                            end: 1_000_000,
                            priority: RangePriority::Scanning,
                        },
                    ],
                },
                t0,
            ),
        );
        assert_eq!(s.sync.stage(0, 1_000), Some(Stage::Scanned));
        assert_eq!(
            s.sync.stage(600_000, 600_100),
            Some(Stage::Live),
            "live work shows in any stretch that holds it"
        );
        // the total is an estimate; an unknown one leaves the last known in place
        reduce(
            &mut s,
            Action::Sync(
                SyncNote::Progress {
                    total_outputs: 0,
                    scanned_outputs: 700,
                    scanned_blocks: 20,
                },
                t0 + Duration::from_secs(3),
            ),
        );
        assert!((s.sync.fraction() - 0.7).abs() < 1e-9);
        reduce(
            &mut s,
            Action::Sync(SyncNote::Finished { blocks_scanned: 20 }, t0),
        );
        assert!((s.sync.fraction() - 1.0).abs() < 1e-9);
        assert_eq!(s.sync.sessions_done, 1);
        reduce(&mut s, Action::Sync(SyncNote::Reset, t0));
        assert_eq!(s.sync.outputs_scanned, 0);
        assert_eq!(s.sync.sessions_done, 0);
        assert!(s.sync.chain_ranges.is_empty());
    }

    #[test]
    fn sync_label_reflects_state() {
        let mut s = main_state();
        assert_eq!(s.sync.label(), "starting sync");
        s.sync.mode = SyncMode::Running;
        s.sync.outputs_total = 10;
        s.sync.outputs_scanned = 5;
        assert_eq!(s.sync.label(), "syncing 50%");
        s.sync.mode = SyncMode::NotRunning;
        s.sync.sessions_done = 1;
        s.sync.next_launch = Some(now() + Duration::from_secs(20));
        assert_eq!(
            s.sync.label(),
            "up to date",
            "no countdown to the next check"
        );
        s.sync.auto = false;
        assert_eq!(s.sync.label(), "sync paused");
        s.sync.auto = true;
        s.sync.error = Some("boom".into());
        assert_eq!(s.sync.label(), "sync failed");
    }

    #[test]
    fn only_testnet_keys_pass_the_prefix_check() {
        assert!(Network::check_ufvk_prefix("uviewtest1abc").is_ok());
        assert!(
            Network::check_ufvk_prefix("uview1abc")
                .unwrap_err()
                .contains("mainnet")
        );
        assert!(
            Network::check_ufvk_prefix("uviewregtest1abc")
                .unwrap_err()
                .contains("regtest")
        );
        assert!(Network::check_ufvk_prefix("u1abc").is_err());
        assert_eq!(Network::parse("featurenet"), Some(Network::Stable));
        assert_eq!(Network::parse("NIGHTLY"), Some(Network::Nightly));
        assert_eq!(Network::parse("mainnet"), None);
    }

    #[test]
    fn open_failure_without_prompt_is_fatal() {
        let mut s = State::new(Phase::Loading("Opening wallet".into()), now());
        reduce(&mut s, Action::OpenFailed("boom".into()));
        assert_eq!(s.phase, Phase::Fatal("boom".into()));
        let cmds = keys(&mut s, &[Key::Char('q')]);
        assert_eq!(cmds, vec![Command::ForceQuit]);
        assert!(s.exit);
    }

    #[test]
    fn wallet_opened_enters_main_and_refreshes() {
        let mut s = State::new(Phase::Loading("Opening wallet".into()), now());
        let cmds = reduce(&mut s, Action::WalletOpened(main_state().wallet.unwrap()));
        assert_eq!(cmds, vec![Command::Refresh]);
        assert_eq!(s.phase, Phase::Main);
    }

    #[test]
    fn theme_picker_previews_keeps_and_reverts() {
        use crate::theme::{THEMES, index_of};
        let mut s = main_state();
        let start = index_of("gruvbox-dark").unwrap();
        s.theme = start;

        keys(&mut s, &[Key::Char('c')]);
        assert_eq!(s.overlay, Overlay::ThemePicker { original: start });
        keys(&mut s, &[Key::Char('j')]);
        assert_eq!(s.theme, (start + 1) % THEMES.len(), "moving previews live");
        keys(&mut s, &[Key::Esc]);
        assert_eq!(
            s.theme, start,
            "Esc restores the theme the picker opened with"
        );
        assert_eq!(s.overlay, Overlay::None);

        keys(&mut s, &[Key::Char('c'), Key::Char('g')]);
        assert_eq!(s.theme, 0);
        keys(&mut s, &[Key::Char('k')]);
        assert_eq!(s.theme, THEMES.len() - 1, "moving up from the top wraps");
        let cmds = keys(&mut s, &[Key::Enter]);
        assert_eq!(cmds, vec![Command::SaveTheme(THEMES[THEMES.len() - 1].id)]);
        assert_eq!(
            s.theme,
            THEMES.len() - 1,
            "Enter keeps the highlighted theme"
        );
        assert!(
            s.notice_text()
                .unwrap()
                .contains(THEMES[THEMES.len() - 1].name)
        );
    }

    #[test]
    fn notices_expire() {
        let mut s = main_state();
        reduce(&mut s, Action::Copied(true));
        assert_eq!(s.notice_text(), Some("copied to clipboard"));
        let later = s.now + Duration::from_secs(10);
        reduce(&mut s, Action::Tick(later));
        assert_eq!(s.notice_text(), None);
    }
}
