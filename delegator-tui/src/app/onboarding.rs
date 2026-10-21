//! The first-run flow: the wallet list, the setup menu, and the three ways to make a wallet.
//!
//! Every screen is a step on [`Onboarding`]. Keys and pastes move between them; the backend is
//! asked for the wallet list, the chain tip, and a fresh seed, and for the final open.

use super::action::{Command, Key, OpenSpec};
use super::state::{
    BirthdayMode, Evidence, KeyMaterial, MenuItem, Network, Onboarding, Phase, State, Step, Tip,
};
use crate::format;

/// Blocks before the chain tip a fresh wallet is born at, so a reorg cannot hide its first
/// transaction.
const FRESH_WALLET_MARGIN: u32 = 100;
/// How far before a date the birthday is placed, so a guessed date still catches everything.
const DATE_MARGIN_SECS: u64 = 14 * 24 * 60 * 60;

/// Keys and pastes while the flow is on screen.
pub fn key(state: &mut State, key: Key) -> Vec<Command> {
    if matches!(key, Key::CtrlC) {
        state.exit = true;
        return vec![Command::ForceQuit];
    }
    let Phase::Onboarding(o) = &mut state.phase else {
        return Vec::new();
    };
    o.error = None;
    match o.step {
        Step::Wallets => wallets_key(o, key),
        Step::Menu => menu_key(o, key),
        Step::Create => create_key(o, key),
        Step::Restore => restore_key(o, key),
        Step::Watch => watch_key(o, key),
        Step::Birthday => birthday_key(o, key),
        Step::Name => name_key(state, key),
    }
}

pub fn paste(state: &mut State, text: &str) -> Vec<Command> {
    let Phase::Onboarding(o) = &mut state.phase else {
        return Vec::new();
    };
    o.error = None;
    let text = text.trim();
    match o.step {
        Step::Wallets | Step::Menu => {
            let mut cmds = jump(o, text);
            if o.step == Step::Watch || o.step == Step::Restore {
                cmds.extend(enter_menu_item(o, None));
            }
            cmds
        }
        Step::Restore => {
            let (words, number) = split_trailing_number(text);
            o.words.extend(words.iter().map(|w| w.to_lowercase()));
            o.words.truncate(24);
            if let Some(n) = number {
                o.birthday_hint = Some(n);
            }
            Vec::new()
        }
        Step::Watch => {
            o.key.push_str(text);
            Vec::new()
        }
        Step::Birthday if o.birthday_mode.is_some() => {
            o.birthday_input.push_str(text);
            Vec::new()
        }
        Step::Name => {
            o.name.extend(text.chars().filter(|c| name_char(*c)));
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// The list came back, the tip came back, or the seed did.
pub fn wallets_listed(state: &mut State, wallets: Vec<super::state::WalletEntry>) -> Vec<Command> {
    let Phase::Onboarding(o) = &mut state.phase else {
        return Vec::new();
    };
    o.wallets = wallets;
    o.listed = true;
    o.wallet_sel = o.wallet_sel.min(o.wallets.len());
    if o.step == Step::Wallets && o.wallets.is_empty() {
        o.step = Step::Menu;
        return fetch_tip(o);
    }
    Vec::new()
}

pub fn tip_fetched(state: &mut State, result: Result<Tip, (Network, String)>) {
    let Phase::Onboarding(o) = &mut state.phase else {
        return;
    };
    o.tip_pending = None;
    match result {
        Ok(tip) => {
            o.tip = Some(tip);
            o.tip_error = None;
        }
        Err((network, message)) => {
            if o.tip.is_none_or(|t| t.network == network) {
                o.tip = None;
                o.tip_error = Some(message);
            }
        }
    }
}

pub fn seed_generated(state: &mut State, phrase: String) {
    if let Phase::Onboarding(o) = &mut state.phase {
        o.seed = Some(phrase);
    }
}

/// The open failed: back to the name screen with the message, everything else kept.
pub fn open_failed(state: &mut State, message: String) {
    if let Some(mut o) = state.onboarding_backup.take() {
        o.step = if o.material.is_some() {
            Step::Name
        } else {
            Step::Wallets
        };
        o.error = Some(message);
        state.phase = Phase::Onboarding(Box::new(o));
    } else {
        state.phase = Phase::Fatal(message);
    }
}

/// Asks for the tip once per network: not again while an answer is pending.
fn fetch_tip(o: &mut Onboarding) -> Vec<Command> {
    if o.tip_for(o.network).is_some() || o.tip_pending == Some(o.network) {
        Vec::new()
    } else {
        o.tip_pending = Some(o.network);
        vec![Command::FetchTip(o.network)]
    }
}

fn move_sel(sel: &mut usize, len: usize, key: Key) {
    if len == 0 {
        *sel = 0;
        return;
    }
    match key {
        Key::Down | Key::Char('j') => *sel = (*sel + 1) % len,
        Key::Up | Key::Char('k') => *sel = (*sel + len - 1) % len,
        Key::Home | Key::Char('g') => *sel = 0,
        Key::End | Key::Char('G') => *sel = len - 1,
        _ => {}
    }
}

fn wallets_key(o: &mut Onboarding, key: Key) -> Vec<Command> {
    let rows = o.wallets.len() + 1;
    move_sel(&mut o.wallet_sel, rows, key);
    match key {
        Key::Char('q') | Key::Esc => vec![Command::ForceQuit],
        Key::Char('n') => {
            o.step = Step::Menu;
            fetch_tip(o)
        }
        Key::Enter | Key::Right => {
            if let Some(entry) = o.wallets.get(o.wallet_sel) {
                let spec = OpenSpec::Read {
                    network: entry.network,
                    name: entry.name.clone(),
                    verified: entry.evidence != Evidence::Unverified,
                };
                vec![Command::Open(spec)]
            } else {
                o.step = Step::Menu;
                fetch_tip(o)
            }
        }
        _ => Vec::new(),
    }
}

fn menu_key(o: &mut Onboarding, key: Key) -> Vec<Command> {
    move_sel(&mut o.menu_sel, MenuItem::ALL.len(), key);
    match key {
        Key::Esc | Key::Char('q') => {
            if o.wallets.is_empty() {
                vec![Command::ForceQuit]
            } else {
                o.step = Step::Wallets;
                Vec::new()
            }
        }
        Key::Char('n') => {
            o.network = match o.network {
                Network::Stable => Network::Nightly,
                Network::Nightly => Network::Stable,
            };
            o.tip_error = None;
            o.tip_pending = None;
            fetch_tip(o)
        }
        Key::Char(c @ '1'..='3') => {
            o.menu_sel = usize::from(c as u8 - b'1');
            enter_menu_item(o, Some(o.menu_item()))
        }
        Key::Enter | Key::Right => enter_menu_item(o, Some(o.menu_item())),
        _ => Vec::new(),
    }
}

/// Opens a branch. `None` means the step is already set by a paste.
fn enter_menu_item(o: &mut Onboarding, item: Option<MenuItem>) -> Vec<Command> {
    match item {
        Some(MenuItem::Quit) => return vec![Command::ForceQuit],
        Some(MenuItem::Create) => {
            o.step = Step::Create;
            o.material = None;
            let mut cmds = fetch_tip(o);
            if o.seed.is_none() {
                cmds.push(Command::GenerateSeed);
            }
            return cmds;
        }
        Some(MenuItem::Restore) => o.step = Step::Restore,
        Some(MenuItem::Watch) => o.step = Step::Watch,
        None => {}
    }
    o.material = None;
    Vec::new()
}

/// A paste on the list or the menu: a seed phrase opens restore, a viewing key opens watch.
fn jump(o: &mut Onboarding, text: &str) -> Vec<Command> {
    let (words, number) = split_trailing_number(text);
    if looks_like_phrase(&words) {
        o.step = Step::Restore;
        o.words = words.iter().map(|w| w.to_lowercase()).collect();
        o.words.truncate(24);
        o.draft.clear();
        o.birthday_hint = number;
    } else if let Some(first) = words.first()
        && first.starts_with("uview")
    {
        o.step = Step::Watch;
        o.key = text.to_string();
    } else {
        o.error = Some("that is not a seed phrase or a viewing key".into());
    }
    Vec::new()
}

fn looks_like_phrase(words: &[&str]) -> bool {
    words.len() >= 12
        && words
            .iter()
            .all(|w| w.chars().all(|c| c.is_ascii_alphabetic()))
}

/// Splits a paste into its words and a trailing number, the way the app puts the key and the
/// birthday on the clipboard together.
fn split_trailing_number(text: &str) -> (Vec<&str>, Option<u32>) {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    let number = match words.last() {
        Some(last) if words.len() > 1 => last.parse::<u32>().ok(),
        _ => None,
    };
    if number.is_some() {
        words.pop();
    }
    (words, number)
}

/// The birthday of a fresh wallet: a little below the tip, or the Ironwood activation height
/// when the indexer keeps no tree state to start from.
pub fn fresh_birthday(network: Network, tip: &Tip) -> u32 {
    tip.settle_birthday(
        tip.height
            .saturating_sub(FRESH_WALLET_MARGIN)
            .max(network.sapling_activation()),
    )
}

/// The birthday the user chose, as this indexer allows it.
fn settle(o: &Onboarding, wanted: u32) -> u32 {
    let network = o.target_network();
    match o.tip_for(network) {
        Some(tip) => tip.settle_birthday(wanted),
        None => wanted,
    }
}

fn create_key(o: &mut Onboarding, key: Key) -> Vec<Command> {
    match key {
        Key::Esc | Key::Left => {
            o.step = Step::Menu;
            Vec::new()
        }
        Key::Char('r') => {
            o.seed_line = !o.seed_line;
            Vec::new()
        }
        Key::Char('R') if o.tip_for(o.network).is_none() => {
            o.tip_error = None;
            o.tip_pending = None;
            fetch_tip(o)
        }
        Key::Char('y') => match o.tip_for(o.network) {
            Some(tip) => vec![Command::Copy(fresh_birthday(o.network, &tip).to_string())],
            None => Vec::new(),
        },
        Key::Enter => {
            let (Some(seed), Some(tip)) = (o.seed.clone(), o.tip_for(o.network)) else {
                o.error = Some(if o.tip_error.is_some() {
                    "the chain tip is needed for the birthday · R tries the server again".into()
                } else {
                    "one moment: the seed and the chain tip are still coming".into()
                });
                return Vec::new();
            };
            o.material = Some((o.network, KeyMaterial::Seed(seed)));
            o.birthday = Some(fresh_birthday(o.network, &tip));
            go_to_name(o);
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn restore_key(o: &mut Onboarding, key: Key) -> Vec<Command> {
    match key {
        Key::Esc => {
            if o.words.is_empty() && o.draft.is_empty() {
                o.step = Step::Menu;
            } else {
                o.words.clear();
                o.draft.clear();
                o.birthday_hint = None;
            }
        }
        Key::Backspace => {
            if o.draft.pop().is_none() {
                o.words.pop();
            }
        }
        Key::Char(' ') | Key::Enter => {
            if !o.draft.is_empty() {
                if o.words.len() < 24 {
                    let word = std::mem::take(&mut o.draft).to_lowercase();
                    o.words.push(word);
                } else {
                    o.draft.clear();
                }
            }
            if key == Key::Enter {
                match check_phrase(&o.words) {
                    Ok(phrase) => {
                        o.material = Some((o.network, KeyMaterial::Seed(phrase)));
                        go_to_birthday(o);
                    }
                    Err(e) => o.error = Some(e),
                }
            }
        }
        Key::Char(c) if c.is_ascii_alphabetic() => o.draft.push(c),
        _ => {}
    }
    Vec::new()
}

/// A phrase with a valid word count and checksum, joined by single spaces.
pub fn check_phrase(words: &[String]) -> Result<String, String> {
    if ![12, 15, 18, 21, 24].contains(&words.len()) {
        return Err(format!(
            "{} words · a seed phrase has 12, 15, 18, 21 or 24",
            words.len()
        ));
    }
    let phrase = words.join(" ");
    match bip0039::Mnemonic::<bip0039::English>::from_phrase(&phrase) {
        Ok(_) => Ok(phrase),
        Err(bip0039::Error::UnknownWord(word)) => {
            Err(format!("\"{word}\" is not on the BIP-39 list"))
        }
        Err(bip0039::Error::InvalidChecksum) => {
            Err("the checksum does not match · a word is wrong or out of order".into())
        }
        Err(e) => Err(format!("not a valid seed phrase: {e}")),
    }
}

fn watch_key(o: &mut Onboarding, key: Key) -> Vec<Command> {
    match key {
        Key::Esc => {
            if o.key.is_empty() {
                o.step = Step::Menu;
            } else {
                o.key.clear();
            }
        }
        Key::Backspace => {
            o.key.pop();
        }
        Key::Enter => {
            let (words, number) = split_trailing_number(&o.key);
            let Some(ufvk) = words.first().copied() else {
                o.error = Some("paste a unified full viewing key".into());
                return Vec::new();
            };
            match check_ufvk(ufvk) {
                Ok(()) => {
                    o.material = Some((o.network, KeyMaterial::Ufvk(ufvk.to_string())));
                    o.birthday_hint = number;
                    go_to_birthday(o);
                }
                Err(e) => o.error = Some(e),
            }
        }
        Key::Char(c) if !c.is_whitespace() => o.key.push(c),
        _ => {}
    }
    Vec::new()
}

/// Whether a viewing key is one these chains can use, or what the text is instead.
pub fn check_ufvk(s: &str) -> Result<(), String> {
    if s.starts_with("zxview") {
        return Err(
            "a legacy Sapling key · export the unified key (uviewtest1…) from the app".into(),
        );
    }
    if s.starts_with("u1") || s.starts_with("utest1") {
        return Err(
            "that is an address, not a viewing key · in the app: settings › viewing key".into(),
        );
    }
    Network::check_ufvk_prefix(s)
}

fn go_to_birthday(o: &mut Onboarding) {
    o.step = Step::Birthday;
    o.birthday_input = o.birthday_hint.map(|n| n.to_string()).unwrap_or_default();
    o.birthday_mode = o.birthday_hint.map(|_| BirthdayMode::FromApp);
    o.birthday_sel = if o.birthday_hint.is_some() { 0 } else { 1 };
}

fn birthday_key(o: &mut Onboarding, key: Key) -> Vec<Command> {
    let network = o.target_network();
    let Some(mode) = o.birthday_mode else {
        move_sel(&mut o.birthday_sel, BirthdayMode::ALL.len(), key);
        match key {
            Key::Esc | Key::Left => {
                o.step = if o.is_watch() {
                    Step::Watch
                } else {
                    Step::Restore
                };
            }
            Key::Char(c @ '1'..='4') => {
                o.birthday_sel = usize::from(c as u8 - b'1');
                choose_birthday_mode(o);
            }
            Key::Enter | Key::Right => choose_birthday_mode(o),
            _ => {}
        }
        return Vec::new();
    };
    match key {
        Key::Esc | Key::Left => o.birthday_mode = None,
        Key::Char('y') | Key::Enter if mode == BirthdayMode::Unknown => {
            o.birthday = Some(network.sapling_activation());
            go_to_name(o);
        }
        Key::Char('n') if mode == BirthdayMode::Unknown => o.birthday_mode = None,
        Key::Enter => match parse_birthday(o, mode) {
            Ok(height) => {
                o.birthday = Some(settle(o, height));
                go_to_name(o);
            }
            Err(e) => o.error = Some(e),
        },
        Key::Backspace => {
            o.birthday_input.pop();
        }
        Key::Char(c) if c.is_ascii_digit() || c == '-' || c == ' ' => o.birthday_input.push(c),
        _ => {}
    }
    Vec::new()
}

fn choose_birthday_mode(o: &mut Onboarding) {
    let mode = BirthdayMode::ALL[o.birthday_sel];
    o.birthday_mode = Some(mode);
    o.birthday_input = match (mode, o.birthday_hint) {
        (BirthdayMode::FromApp, Some(n)) => n.to_string(),
        _ => String::new(),
    };
}

/// What the input says once parsed in `mode`, with the reasons it cannot be used.
pub fn parse_birthday(o: &Onboarding, mode: BirthdayMode) -> Result<u32, String> {
    let network = o.target_network();
    let input = o.birthday_input.trim();
    let floor = network.sapling_activation();
    let tip = o.tip_for(network);
    let height = match mode {
        BirthdayMode::FromApp => {
            let number = input.split_whitespace().find_map(|w| w.parse::<u32>().ok());
            number.ok_or(
                "no number in the paste · in the app, tap the birthday under the viewing key",
            )?
        }
        BirthdayMode::Height => {
            if input.is_empty() {
                return Err("enter the block height the wallet was created at".into());
            }
            if input.contains('-') {
                return Err("this one takes a height · pick \"I know the date\" for a date".into());
            }
            input
                .parse::<u32>()
                .map_err(|_| "a block height is a number, like 2604500".to_string())?
        }
        BirthdayMode::Date => {
            let date = parse_date(input)?;
            let Some(tip) = tip else {
                return Err(match &o.tip_error {
                    Some(e) => format!("the chain tip is not known ({e}) · enter a height instead"),
                    None => "the chain tip is still coming · try again in a moment".into(),
                });
            };
            height_for_date(network, tip, date).ok_or("that date is in the future")?
        }
        BirthdayMode::Unknown => floor,
    };
    if height < floor {
        return Err(format!(
            "must be at least {}, where Sapling activated on {}",
            format::group(u64::from(floor)),
            network.name()
        ));
    }
    if let Some(tip) = tip
        && height > tip.height
    {
        return Err(format!(
            "{} is above the chain tip, {}",
            format::group(u64::from(height)),
            format::group(u64::from(tip.height))
        ));
    }
    Ok(height)
}

/// `YYYY-MM` or `YYYY-MM-DD` as seconds since the epoch, at the start of that day.
pub fn parse_date(s: &str) -> Result<u64, String> {
    use chrono::{NaiveDate, TimeZone, Utc};
    let full = match s.len() {
        7 => format!("{s}-01"),
        10 => s.to_string(),
        _ => return Err("a date looks like 2024-05 or 2024-05-17".into()),
    };
    let date = NaiveDate::parse_from_str(&full, "%Y-%m-%d")
        .map_err(|_| "that date does not exist".to_string())?;
    let at = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"));
    u64::try_from(at.timestamp()).map_err(|_| "a date after 1970, please".to_string())
}

/// The height the chain was at on `date_unix`, two weeks early for safety. `None` for a date
/// after the tip was seen.
pub fn height_for_date(network: Network, tip: Tip, date_unix: u64) -> Option<u32> {
    if date_unix > tip.at_unix {
        return None;
    }
    let behind = (tip.at_unix - date_unix + DATE_MARGIN_SECS) / tip.spacing_secs.max(1);
    let blocks = u32::try_from(behind).unwrap_or(u32::MAX);
    Some(
        tip.height
            .saturating_sub(blocks)
            .max(network.sapling_activation()),
    )
}

fn go_to_name(o: &mut Onboarding) {
    o.step = Step::Name;
    if o.name.is_empty() {
        o.name = default_name(o);
    }
}

/// `main` or `watch` on the stable chain, `nightly` or `nightly-watch` on the other, then
/// `-2`, `-3`, … past the names in use.
pub fn default_name(o: &Onboarding) -> String {
    let base = match (o.target_network(), o.is_watch()) {
        (Network::Stable, false) => "main".to_string(),
        (Network::Stable, true) => "watch".to_string(),
        (Network::Nightly, false) => "nightly".to_string(),
        (Network::Nightly, true) => "nightly-watch".to_string(),
    };
    let taken = |name: &str| o.wallets.iter().any(|w| w.name == name);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|i| format!("{base}-{i}"))
        .find(|name| !taken(name))
        .expect("an unused name")
}

pub fn name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

fn name_key(state: &mut State, key: Key) -> Vec<Command> {
    let Phase::Onboarding(o) = &mut state.phase else {
        return Vec::new();
    };
    match key {
        Key::Esc | Key::Left => {
            o.step = if o.seed.is_some() && !o.is_watch() {
                Step::Create
            } else {
                Step::Birthday
            };
            if o.step == Step::Birthday {
                o.birthday_mode = None;
            }
            Vec::new()
        }
        Key::Backspace => {
            o.name.pop();
            Vec::new()
        }
        Key::Enter => {
            let name = o.name.trim().to_string();
            if name.is_empty()
                || !name.chars().all(name_char)
                || !name.starts_with(|c: char| c.is_ascii_alphanumeric())
            {
                o.error = Some(
                    "lowercase letters, digits and dashes, starting with a letter or digit".into(),
                );
                return Vec::new();
            }
            if o.wallets.iter().any(|w| w.name == name) {
                o.error = Some(format!(
                    "\"{name}\" is taken · esc and open it from the list, or pick another"
                ));
                return Vec::new();
            }
            let (Some((network, material)), Some(birthday)) = (o.material.clone(), o.birthday)
            else {
                return Vec::new();
            };
            let spec = OpenSpec::Create {
                network,
                name,
                material,
                birthday,
                server: o.server.clone(),
            };
            let backup = (**o).clone();
            state.onboarding_backup = Some(backup);
            state.phase = Phase::Loading("Creating wallet".into());
            vec![Command::Open(spec)]
        }
        Key::Char(c) if name_char(c.to_ascii_lowercase()) => {
            o.name.push(c.to_ascii_lowercase());
            Vec::new()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::app::action::Action;
    use crate::app::reduce::reduce;
    use crate::app::state::{Evidence, WalletEntry, WalletKind};

    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    fn start() -> State {
        State::new(
            Phase::Onboarding(Box::new(Onboarding::new(Network::Stable, None))),
            Instant::now(),
        )
    }

    fn o(state: &State) -> &Onboarding {
        match &state.phase {
            Phase::Onboarding(o) => o,
            other => panic!("not onboarding: {other:?}"),
        }
    }

    fn keys(state: &mut State, keys: &[Key]) -> Vec<Command> {
        keys.iter()
            .flat_map(|k| reduce(state, Action::Key(*k)))
            .collect()
    }

    fn type_str(state: &mut State, text: &str) -> Vec<Command> {
        text.chars()
            .flat_map(|c| reduce(state, Action::Key(Key::Char(c))))
            .collect()
    }

    fn listed(state: &mut State, names: &[&str]) -> Vec<Command> {
        let wallets = names
            .iter()
            .map(|n| WalletEntry {
                name: n.to_string(),
                network: Network::Stable,
                kind: WalletKind::Seed,
                birthday: Some(1),
                age_secs: None,
                evidence: Evidence::Meta,
            })
            .collect();
        reduce(state, Action::WalletsListed(wallets))
    }

    fn tip(network: Network, height: u32) -> Action {
        Action::TipFetched(Ok(Tip {
            network,
            height,
            at_unix: 1_800_000_000,
            spacing_secs: 75,
            ironwood_tree: true,
        }))
    }

    #[test]
    fn an_empty_directory_skips_the_list_and_asks_for_the_tip() {
        let mut s = start();
        assert_eq!(o(&s).step, Step::Wallets);
        let cmds = listed(&mut s, &[]);
        assert_eq!(o(&s).step, Step::Menu);
        assert_eq!(cmds, vec![Command::FetchTip(Network::Stable)]);
        // esc on the menu with nothing to go back to quits
        assert_eq!(keys(&mut s, &[Key::Esc]), vec![Command::ForceQuit]);
    }

    #[test]
    fn the_list_opens_a_wallet_or_leads_to_the_menu() {
        let mut s = start();
        listed(&mut s, &["main", "test"]);
        assert_eq!(o(&s).step, Step::Wallets);
        let cmds = keys(&mut s, &[Key::Char('j'), Key::Enter]);
        assert_eq!(
            cmds,
            vec![Command::Open(OpenSpec::Read {
                network: Network::Stable,
                name: "test".into(),
                verified: true,
            })]
        );
        // the row after the last wallet is "new"
        let cmds = keys(&mut s, &[Key::Char('j'), Key::Enter]);
        assert_eq!(o(&s).step, Step::Menu);
        assert_eq!(cmds, vec![Command::FetchTip(Network::Stable)]);
        // and esc goes back to the list this time
        keys(&mut s, &[Key::Esc]);
        assert_eq!(o(&s).step, Step::Wallets);
        keys(&mut s, &[Key::Char('n')]);
        assert_eq!(o(&s).step, Step::Menu);
    }

    #[test]
    fn create_shows_the_seed_and_enter_names_then_creates() {
        let mut s = start();
        listed(&mut s, &["main"]);
        keys(&mut s, &[Key::Char('n')]);
        let cmds = keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).step, Step::Create);
        assert_eq!(cmds, vec![Command::GenerateSeed]);
        // nothing to create until the seed and the tip are in
        keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).step, Step::Create);
        assert!(o(&s).error.is_some());
        reduce(&mut s, Action::SeedGenerated(PHRASE.into()));
        reduce(&mut s, tip(Network::Stable, 3_000_000));
        assert_eq!(
            keys(&mut s, &[Key::Char('y')]),
            vec![Command::Copy("2999900".into())],
            "y copies the birthday, never the words"
        );
        keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).step, Step::Name);
        assert_eq!(o(&s).name, "main-2", "main is taken");
        assert_eq!(o(&s).birthday, Some(2_999_900));
        keys(&mut s, &[Key::Backspace, Key::Backspace]);
        let cmds = type_str(&mut s, "X!");
        assert_eq!(o(&s).name, "mainx", "lowercased, punctuation dropped");
        assert!(cmds.is_empty());
        let cmds = keys(&mut s, &[Key::Enter]);
        assert_eq!(
            cmds,
            vec![Command::Open(OpenSpec::Create {
                network: Network::Stable,
                name: "mainx".into(),
                material: KeyMaterial::Seed(PHRASE.into()),
                birthday: 2_999_900,
                server: None,
            })]
        );
        assert!(matches!(s.phase, Phase::Loading(_)));
        // a failure comes back to the name screen with everything kept
        reduce(&mut s, Action::OpenFailed("disk full".into()));
        assert_eq!(o(&s).step, Step::Name);
        assert_eq!(o(&s).error.as_deref(), Some("disk full"));
        assert_eq!(o(&s).name, "mainx");
    }

    #[test]
    fn a_taken_name_is_refused() {
        let mut s = start();
        listed(&mut s, &["main"]);
        keys(&mut s, &[Key::Char('n'), Key::Enter]);
        reduce(&mut s, Action::SeedGenerated(PHRASE.into()));
        reduce(&mut s, tip(Network::Stable, 3_000_000));
        keys(&mut s, &[Key::Enter]);
        for _ in 0..10 {
            keys(&mut s, &[Key::Backspace]);
        }
        type_str(&mut s, "main");
        assert!(keys(&mut s, &[Key::Enter]).is_empty());
        assert!(o(&s).error.as_deref().unwrap().contains("taken"));
        assert_eq!(o(&s).step, Step::Name);
    }

    #[test]
    fn restore_takes_words_and_a_trailing_birthday() {
        let mut s = start();
        listed(&mut s, &[]);
        // a paste on the menu jumps straight in
        reduce(&mut s, Action::Paste(format!("{PHRASE} 2604500")));
        assert_eq!(o(&s).step, Step::Restore);
        assert_eq!(o(&s).words.len(), 24);
        assert_eq!(o(&s).birthday_hint, Some(2_604_500));
        keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).step, Step::Birthday);
        assert_eq!(o(&s).birthday_mode, Some(BirthdayMode::FromApp));
        assert_eq!(o(&s).birthday_input, "2604500");
        keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).step, Step::Name);
        assert_eq!(o(&s).birthday, Some(2_604_500));
        assert_eq!(o(&s).name, "main");
        let cmds = keys(&mut s, &[Key::Enter]);
        assert!(matches!(
            cmds.as_slice(),
            [Command::Open(OpenSpec::Create { material: KeyMaterial::Seed(p), birthday: 2_604_500, .. })] if p == PHRASE
        ));
    }

    #[test]
    fn restore_rejects_bad_phrases_with_a_reason() {
        let mut s = start();
        listed(&mut s, &[]);
        keys(&mut s, &[Key::Char('2')]);
        assert_eq!(o(&s).step, Step::Restore);
        type_str(&mut s, "abandon ability");
        keys(&mut s, &[Key::Enter]);
        assert!(o(&s).error.as_deref().unwrap().contains("2 words"));
        // backspace eats the draft first, then whole words
        type_str(&mut s, "zz");
        keys(&mut s, &[Key::Backspace, Key::Backspace, Key::Backspace]);
        assert_eq!(o(&s).words, vec!["abandon"]);
        // twelve words with a bad checksum
        keys(&mut s, &[Key::Esc]);
        assert!(o(&s).words.is_empty(), "esc clears first");
        reduce(&mut s, Action::Paste("abandon ".repeat(12)));
        keys(&mut s, &[Key::Enter]);
        assert!(
            o(&s).error.as_deref().unwrap().contains("checksum"),
            "{:?}",
            o(&s).error
        );
        keys(&mut s, &[Key::Esc, Key::Esc]);
        assert_eq!(o(&s).step, Step::Menu, "esc on an empty grid goes back");
        reduce(&mut s, Action::Paste("abandon zzzz ".repeat(6)));
        keys(&mut s, &[Key::Enter]);
        assert!(o(&s).error.as_deref().unwrap().contains("zzzz"));
    }

    #[test]
    fn watch_reads_the_key_and_asks_for_the_birthday() {
        let mut s = start();
        listed(&mut s, &[]);
        keys(&mut s, &[Key::Char('3')]);
        assert_eq!(o(&s).step, Step::Watch);
        reduce(&mut s, Action::Paste("u1notakey".into()));
        keys(&mut s, &[Key::Enter]);
        assert!(o(&s).error.as_deref().unwrap().contains("address"));
        keys(&mut s, &[Key::Esc]);
        assert!(o(&s).key.is_empty());
        reduce(&mut s, Action::Paste("uview1mainnetkey".into()));
        keys(&mut s, &[Key::Enter]);
        assert!(o(&s).error.as_deref().unwrap().contains("mainnet"));
        keys(&mut s, &[Key::Esc]);
        reduce(&mut s, Action::Paste("uviewtest1xyz".into()));
        keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).step, Step::Birthday);
        assert_eq!(o(&s).birthday_mode, None);
        assert_eq!(o(&s).target_network(), Network::Stable, "the menu's chain");
        // "I don't know" scans from the floor, which is block 1 on the crosslink testnet
        keys(&mut s, &[Key::Char('4'), Key::Char('y')]);
        assert_eq!(o(&s).step, Step::Name);
        assert_eq!(o(&s).birthday, Some(1));
        assert_eq!(o(&s).name, "watch");
    }

    #[test]
    fn birthday_modes_check_their_input() {
        let mut s = start();
        listed(&mut s, &[]);
        keys(&mut s, &[Key::Char('3')]);
        reduce(&mut s, Action::Paste("uviewtest1xyz".into()));
        keys(&mut s, &[Key::Enter]);
        // height below sapling
        keys(&mut s, &[Key::Char('2')]);
        type_str(&mut s, "0");
        keys(&mut s, &[Key::Enter]);
        assert!(o(&s).error.as_deref().unwrap().contains("at least 1"));
        // a date in height mode
        keys(&mut s, &[Key::Esc, Key::Char('2')]);
        type_str(&mut s, "2024-05");
        keys(&mut s, &[Key::Enter]);
        assert!(o(&s).error.as_deref().unwrap().contains("date"));
        // a date without a tip
        keys(&mut s, &[Key::Esc, Key::Char('3')]);
        type_str(&mut s, "2024-05");
        keys(&mut s, &[Key::Enter]);
        assert!(o(&s).error.as_deref().unwrap().contains("tip"));
        // with the tip: two weeks early, never above the tip
        reduce(&mut s, tip(Network::Stable, 3_000_000));
        keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).step, Step::Name);
        let b = o(&s).birthday.unwrap();
        assert!(b < 3_000_000 && b > 1, "{b}");
        // the paste mode finds the number in whatever came with it
        let mut s = start();
        listed(&mut s, &[]);
        keys(&mut s, &[Key::Char('3')]);
        reduce(&mut s, Action::Paste("uviewtest1xyz".into()));
        keys(&mut s, &[Key::Enter, Key::Char('1')]);
        reduce(&mut s, Action::Paste("birthday 2604500".into()));
        keys(&mut s, &[Key::Enter]);
        assert_eq!(o(&s).birthday, Some(2_604_500));
    }

    #[test]
    fn dates_become_heights_two_weeks_early() {
        let t = Tip {
            network: Network::Stable,
            height: 3_000_000,
            at_unix: 1_800_000_000,
            spacing_secs: 75,
            ironwood_tree: true,
        };
        let day = 24 * 60 * 60;
        // the moment of the tip itself: only the margin
        assert_eq!(
            height_for_date(Network::Stable, t, t.at_unix),
            Some(3_000_000 - 14 * day / 75)
        );
        assert_eq!(height_for_date(Network::Stable, t, t.at_unix + 1), None);
        // far back, the floor holds
        assert_eq!(height_for_date(Network::Stable, t, 0), Some(1));
        assert!(parse_date("2024-05").is_ok());
        assert!(parse_date("2024-05-17").is_ok());
        assert!(parse_date("2024-13-01").is_err());
        assert!(parse_date("yesterday").is_err());
    }

    #[test]
    fn the_menu_cycles_the_network_and_pastes_jump() {
        let mut s = start();
        listed(&mut s, &[]);
        assert_eq!(
            keys(&mut s, &[Key::Char('n')]),
            vec![Command::FetchTip(Network::Nightly)]
        );
        assert_eq!(o(&s).network, Network::Nightly);
        reduce(&mut s, Action::Paste("hello there".into()));
        assert_eq!(o(&s).step, Step::Menu);
        assert!(o(&s).error.is_some());
        reduce(&mut s, Action::Paste("uviewtest1abc 123".into()));
        assert_eq!(o(&s).step, Step::Watch);
        assert_eq!(o(&s).key, "uviewtest1abc 123");
    }

    #[test]
    fn default_names_avoid_what_exists() {
        let mut o = Onboarding::new(Network::Stable, None);
        o.material = Some((Network::Stable, KeyMaterial::Ufvk("k".into())));
        assert_eq!(default_name(&o), "watch");
        o.wallets.push(WalletEntry {
            name: "watch".into(),
            network: Network::Stable,
            kind: WalletKind::Watch,
            birthday: None,
            age_secs: None,
            evidence: Evidence::Meta,
        });
        assert_eq!(default_name(&o), "watch-2");
        o.material = Some((Network::Nightly, KeyMaterial::Seed("s".into())));
        assert_eq!(default_name(&o), "nightly");
    }

    #[test]
    fn an_indexer_without_ironwood_state_starts_wallets_at_the_activation_height() {
        let bare = Tip {
            network: Network::Stable,
            height: 73_284,
            at_unix: 1_800_000_000,
            spacing_secs: 15,
            ironwood_tree: false,
        };
        assert_eq!(fresh_birthday(Network::Stable, &bare), 1);
        assert_eq!(bare.settle_birthday(50_000), 1);
        let full = Tip {
            ironwood_tree: true,
            ..bare
        };
        assert_eq!(fresh_birthday(Network::Stable, &full), 73_184);
        assert_eq!(full.settle_birthday(50_000), 50_000);
        // a restore on the bare indexer is settled the same way
        let mut s = start();
        listed(&mut s, &[]);
        reduce(&mut s, Action::Paste(format!("{PHRASE} 50000")));
        keys(&mut s, &[Key::Char('1')]);
        reduce(&mut s, Action::TipFetched(Ok(bare)));
        keys(&mut s, &[Key::Enter, Key::Enter]);
        assert_eq!(o(&s).step, Step::Name);
        assert_eq!(o(&s).birthday, Some(1));
    }
}
