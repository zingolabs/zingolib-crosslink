//! A backend with no wallet and no network: made-up data for every screen.
//!
//! `--fake` swaps this in for the real task. It answers the same commands with the same
//! actions, so the reducer and the views cannot tell. The wallet list has three entries, the
//! open wallet has a history of every transaction kind, addresses of both kinds, bonded funds,
//! and a sync that scans, finds a transaction now and then, finishes, and starts again. All of
//! it comes from a fixed seed, so a screen looks the same on every run. Nothing touches disk.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;

use crate::app::RosterRow;
use crate::app::{
    Action, AddressKind, AddressRow, Balance, BondRow, BondStatus, ChainRange, Command, Evidence,
    InputRow, KeyMaterial, Network, OpenSpec, OutputRole, OutputRow, Pool, PoolBalance,
    RangePriority, SpendState, SyncMode, SyncNote, Tip, TxKind, TxRow, TxStatus, WalletData,
    WalletEntry, WalletEventRow, WalletInfo, WalletKind,
};

const TICK: Duration = Duration::from_millis(500);
/// Outputs the made-up chain holds between the birthday and the tip.
const OUTPUTS: u64 = 12_000;
/// Outputs scanned per tick while a session runs.
const PACE: u64 = 180;
/// Rest between sessions.
const RESYNC: Duration = Duration::from_secs(15);
const SEED_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

pub fn spawn(
    network: Network,
    commands: mpsc::UnboundedReceiver<Command>,
    actions: mpsc::UnboundedSender<Action>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(Fake::new(network, actions).run(commands))
}

/// One made-up transaction: days ago, hours, kind, value, fee, memo.
type Story = (u64, u64, TxKind, u64, Option<u64>, &'static str);

/// A held staking action: a new bond's finalizer and amount, or the key of the bond to act on.
enum FakeStaking {
    Stake { finalizer: String, amount: u64 },
    Unbond { bond: String },
    Redelegate { bond: String, to: String },
}

/// What every made-up staking transaction pays.
const STAKE_FEE: u64 = 20_000;

/// A small deterministic generator, so the data is the same every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn hex(&mut self, len: usize) -> String {
        let mut s = String::with_capacity(len);
        while s.len() < len {
            s.push_str(&format!("{:016x}", self.next()));
        }
        s.truncate(len);
        s
    }

    /// A finalizer address of the right shape: `zfinv1` and 128 base64url characters.
    fn finalizer(&mut self) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let body: String = (0..128)
            .map(|_| CHARS[self.below(64) as usize] as char)
            .collect();
        format!("zfinv1{body}")
    }

    /// Bech32-looking text for addresses.
    fn bech(&mut self, len: usize) -> String {
        const CHARS: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
        (0..len)
            .map(|_| CHARS[self.below(32) as usize] as char)
            .collect()
    }
}

struct Fake {
    network: Network,
    actions: mpsc::UnboundedSender<Action>,
    rng: Rng,
    tip: u32,
    birthday: u32,
    name: String,
    kind: WalletKind,
    data: WalletData,
    opened: bool,
    /// A send waiting for confirmation: to, amount, memo, fee.
    proposal: Option<(String, u64, Option<String>, u64)>,
    /// A staking action waiting for confirmation.
    staking: Option<FakeStaking>,
    mode: SyncMode,
    auto: bool,
    scanned: u64,
    sessions: u64,
    next_launch: Option<Instant>,
    /// Ticks since the session started, for pacing discoveries.
    ticks: u64,
}

impl Fake {
    fn new(network: Network, actions: mpsc::UnboundedSender<Action>) -> Self {
        Self {
            network,
            actions,
            rng: Rng(0x5eed_1234_abcd_0001),
            tip: 612_480,
            birthday: 118_000,
            name: String::new(),
            kind: WalletKind::Seed,
            data: WalletData::default(),
            opened: false,
            proposal: None,
            staking: None,
            mode: SyncMode::NotRunning,
            auto: true,
            scanned: 0,
            sessions: 0,
            next_launch: None,
            ticks: 0,
        }
    }

    fn send(&self, action: Action) {
        let _ = self.actions.send(action);
    }

    fn note(&self, note: SyncNote) {
        self.send(Action::Sync(note, Instant::now()));
    }

    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    None => break,
                    Some(cmd) => if self.handle(cmd).await { break; },
                },
                _ = tick.tick() => self.on_tick(),
            }
        }
    }

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1_800_000_000)
    }

    async fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::ListWallets => {
                tokio::time::sleep(Duration::from_millis(150)).await;
                self.send(Action::WalletsListed(vec![
                    WalletEntry {
                        name: "main".into(),
                        network: Network::Stable,
                        kind: WalletKind::Seed,
                        birthday: Some(118_000),
                        age_secs: Some(130),
                        evidence: Evidence::Meta,
                    },
                    WalletEntry {
                        name: "phone-watch".into(),
                        network: Network::Stable,
                        kind: WalletKind::Watch,
                        birthday: Some(118_000),
                        age_secs: Some(3_700),
                        evidence: Evidence::Server,
                    },
                    WalletEntry {
                        name: "nightly".into(),
                        network: Network::Nightly,
                        kind: WalletKind::Seed,
                        birthday: Some(44_210),
                        age_secs: Some(3 * 86_400),
                        evidence: Evidence::Unverified,
                    },
                ]));
            }
            Command::FetchTip(network) => {
                tokio::time::sleep(Duration::from_millis(400)).await;
                self.send(Action::TipFetched(Ok(Tip {
                    network,
                    height: self.tip,
                    at_unix: Self::now_unix(),
                    spacing_secs: 15,
                    ironwood_tree: true,
                })));
            }
            Command::GenerateSeed => self.send(Action::SeedGenerated(SEED_PHRASE.into())),
            Command::Open(spec) => {
                if self.opened {
                    return false;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                let (network, name, kind, birthday) = match spec {
                    OpenSpec::Read { network, name, .. } => {
                        let kind = if name.contains("watch") {
                            WalletKind::Watch
                        } else {
                            WalletKind::Seed
                        };
                        (network, name, kind, self.birthday)
                    }
                    OpenSpec::Create {
                        network,
                        name,
                        material,
                        birthday,
                        ..
                    } => {
                        let kind = match material {
                            KeyMaterial::Seed(_) => WalletKind::Seed,
                            KeyMaterial::Ufvk(_) => WalletKind::Watch,
                        };
                        (network, name, kind, birthday)
                    }
                };
                self.network = network;
                self.name = name.clone();
                self.kind = kind;
                self.birthday = birthday;
                self.opened = true;
                self.data = self.make_data();
                self.send(Action::WalletOpened(WalletInfo {
                    network,
                    wallet_name: name,
                    wallet_path: "(fake, nothing on disk)".into(),
                    birthday,
                    server: network.default_server(),
                    has_orchard: true,
                    spacing_secs: 15,
                    kind,
                }));
                self.send(Action::DataLoaded(self.data.clone()));
                self.launch();
            }
            Command::Refresh => {
                if self.opened {
                    self.send(Action::DataLoaded(self.data.clone()));
                }
            }
            Command::NewUnifiedAddress { orchard, sapling } => {
                let index = self
                    .data
                    .addresses
                    .iter()
                    .filter(|a| a.kind.is_unified())
                    .count() as u32;
                let address = format!("utest1{}", self.rng.bech(140));
                self.data.addresses.push(AddressRow {
                    kind: AddressKind::Unified {
                        orchard,
                        sapling,
                        transparent: true,
                    },
                    index,
                    address,
                    received: 0,
                });
                self.send(Action::Notice(format!("new unified address #{index}")));
                self.send(Action::DataLoaded(self.data.clone()));
            }
            Command::NewTransparentAddress => {
                let index = self
                    .data
                    .addresses
                    .iter()
                    .filter(|a| !a.kind.is_unified())
                    .count() as u32;
                let address = format!("tm{}", self.rng.bech(33));
                self.data.addresses.push(AddressRow {
                    kind: AddressKind::Transparent {
                        scope: "External".into(),
                    },
                    index,
                    address,
                    received: 0,
                });
                self.send(Action::Notice(format!("new transparent address #{index}")));
                self.send(Action::DataLoaded(self.data.clone()));
            }
            Command::PauseSync => match self.mode {
                SyncMode::Running => {
                    self.mode = SyncMode::Paused;
                    self.note(SyncNote::Mode(SyncMode::Paused));
                }
                SyncMode::NotRunning => {
                    self.auto = false;
                    self.next_launch = None;
                    self.note(SyncNote::Auto(false));
                }
                _ => {}
            },
            Command::ResumeSync => match self.mode {
                SyncMode::Paused => {
                    self.mode = SyncMode::Running;
                    self.note(SyncNote::Mode(SyncMode::Running));
                }
                SyncMode::NotRunning => {
                    self.auto = true;
                    self.note(SyncNote::Auto(true));
                    self.launch();
                }
                _ => {}
            },
            Command::Rescan => {
                if !self.opened {
                    return false;
                }
                self.note(SyncNote::Reset);
                self.auto = true;
                self.next_launch = None;
                self.note(SyncNote::Auto(true));
                self.launch();
            }
            Command::Quit => {
                self.send(Action::Shutdown);
                return true;
            }
            Command::Faucet { address } => {
                tokio::time::sleep(Duration::from_millis(700)).await;
                const DROP: u64 = 1_000_000_000;
                let height = self.tip + 1;
                let tx = self.payment(
                    TxKind::Received,
                    DROP,
                    None,
                    Some("from the faucet"),
                    Some(address),
                    height,
                );
                self.data.txs.insert(0, tx);
                self.data.balance.orchard.pending += DROP;
                self.send(Action::FaucetDone(Ok(DROP)));
                self.send(Action::DataLoaded(self.data.clone()));
            }
            Command::ProposeSend { to, amount, memo } => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                const FEE: u64 = 15_000;
                let spendable = self.data.balance.total() - self.data.balance.pending();
                let result = if !(to.starts_with("u") || to.starts_with("t") || to.starts_with("z"))
                    || to.len() < 20
                {
                    Err("that is not a zcash address".to_string())
                } else if amount + FEE > spendable {
                    Err(format!(
                        "not enough spendable funds: {} ZEC after the fee",
                        crate::format::zec(spendable.saturating_sub(FEE))
                    ))
                } else {
                    self.proposal = Some((to, amount, memo, FEE));
                    Ok((FEE, amount + FEE))
                };
                self.send(Action::SendProposed(result));
            }
            Command::ConfirmSend => {
                tokio::time::sleep(Duration::from_millis(900)).await;
                let result = match self.proposal.take() {
                    Some((to, amount, memo, fee)) => {
                        let height = self.tip + 1;
                        let tx = self.payment(
                            TxKind::Sent,
                            amount,
                            Some(fee),
                            memo.as_deref(),
                            Some(to),
                            height,
                        );
                        let txid = tx.txid.clone();
                        self.data.txs.insert(0, tx);
                        self.data.balance.orchard.confirmed -= amount + fee;
                        Ok(vec![txid])
                    }
                    None => Err("nothing is proposed".to_string()),
                };
                self.send(Action::SendDone(result));
                self.send(Action::DataLoaded(self.data.clone()));
            }
            Command::CancelSend => {
                self.proposal = None;
                self.staking = None;
            }
            Command::ProposeStake { finalizer, amount } => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let spendable = self.data.balance.total() - self.data.balance.pending();
                let result = if !zcash_primitives::transaction::is_power_of_10(amount) {
                    Err(super::not_a_power_of_ten(amount))
                } else if finalizer.len() != 134 {
                    Err("that finalizer address fails: invalid finalizer address encoding".into())
                } else if amount + STAKE_FEE > spendable {
                    Err(format!(
                        "not enough spendable funds: {} ZEC after the fee",
                        crate::format::zec(spendable.saturating_sub(STAKE_FEE))
                    ))
                } else {
                    self.staking = Some(FakeStaking::Stake { finalizer, amount });
                    Ok(STAKE_FEE)
                };
                self.send(Action::StakeProposed(result));
            }
            Command::ProposeUnbond { bond } => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                self.staking = Some(FakeStaking::Unbond { bond });
                self.send(Action::StakeProposed(Ok(STAKE_FEE)));
            }
            Command::ProposeRedelegate { bond, to } => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                self.staking = Some(FakeStaking::Redelegate { bond, to });
                self.send(Action::StakeProposed(Ok(STAKE_FEE)));
            }
            Command::ConfirmStake => {
                tokio::time::sleep(Duration::from_millis(900)).await;
                let result = match self.staking.take() {
                    Some(staking) => Ok(vec![self.stake(staking)]),
                    None => Err("nothing is proposed".to_string()),
                };
                self.send(Action::StakeDone(result));
                self.send(Action::DataLoaded(self.data.clone()));
            }
            Command::Withdraw { bond } => {
                tokio::time::sleep(Duration::from_millis(900)).await;
                let result = match self.data.bonds.iter().position(|b| b.key == bond) {
                    Some(i) if self.data.bonds[i].status == BondStatus::Unbonding => {
                        let bond = self.data.bonds.remove(i);
                        self.data.balance.unbonding -= bond.amount;
                        self.data.balance.ironwood.pending += bond.amount;
                        self.data.balance.orchard.confirmed -= STAKE_FEE;
                        Ok(vec![self.staking_tx(TxKind::WithdrawBond, bond.amount)])
                    }
                    Some(_) => Err("the bond is not unbonding".to_string()),
                    None => Err("the node has no such bond".to_string()),
                };
                self.send(Action::StakeDone(result));
                self.send(Action::DataLoaded(self.data.clone()));
            }
            Command::Copy(_) | Command::SaveTheme(_) | Command::ForceQuit => {}
        }
        false
    }

    /// Applies a confirmed staking action to the made-up wallet and chain. The answer is the
    /// txid.
    fn stake(&mut self, staking: FakeStaking) -> String {
        let height = self.tip + 1;
        self.data.balance.orchard.confirmed -= STAKE_FEE;
        match staking {
            FakeStaking::Stake { finalizer, amount } => {
                self.data.balance.orchard.confirmed -= amount;
                self.data.balance.staked += amount;
                self.data.bonds.push(BondRow {
                    key: self.rng.hex(64),
                    finalizer: finalizer.clone(),
                    amount,
                    initial: amount,
                    status: BondStatus::Bonded,
                    since: height,
                });
                self.move_stake(None, Some(&finalizer), amount);
                self.staking_tx(TxKind::CreateBond, amount)
            }
            FakeStaking::Unbond { bond } => {
                let Some(row) = self.data.bonds.iter_mut().find(|b| b.key == bond) else {
                    return self.staking_tx(TxKind::BeginUnbond, 0);
                };
                row.status = BondStatus::Unbonding;
                row.since = height;
                let (amount, finalizer) = (row.amount, row.finalizer.clone());
                self.data.balance.staked -= amount;
                self.data.balance.unbonding += amount;
                self.move_stake(Some(&finalizer), None, amount);
                self.staking_tx(TxKind::BeginUnbond, amount)
            }
            FakeStaking::Redelegate { bond, to } => {
                let Some(row) = self.data.bonds.iter_mut().find(|b| b.key == bond) else {
                    return self.staking_tx(TxKind::RetargetBond, 0);
                };
                let from = std::mem::replace(&mut row.finalizer, to.clone());
                row.since = height;
                let amount = row.amount;
                self.move_stake(Some(&from), Some(&to), amount);
                self.staking_tx(TxKind::RetargetBond, amount)
            }
        }
    }

    /// Moves `amount` of stake on the roster from one finalizer to another, either end
    /// optional, and marks the ones the wallet has a bond with, as the real backend does.
    fn move_stake(&mut self, from: Option<&str>, to: Option<&str>, amount: u64) {
        let roster = &mut self.data.roster;
        if let Some(row) = from.and_then(|f| roster.iter_mut().find(|r| r.finalizer == f)) {
            row.stake = row.stake.saturating_sub(amount);
            row.bonds = row.bonds.saturating_sub(1);
        }
        if let Some(to) = to {
            match roster.iter_mut().find(|r| r.finalizer == to) {
                Some(row) => {
                    row.stake += amount;
                    row.bonds += 1;
                }
                None => roster.push(RosterRow {
                    finalizer: to.to_string(),
                    stake: amount,
                    bonds: 1,
                    ours: true,
                }),
            }
        }
        let bonds = &self.data.bonds;
        for row in roster.iter_mut() {
            row.ours = bonds.iter().any(|b| b.finalizer == row.finalizer);
        }
        roster.sort_by(|a, b| b.stake.cmp(&a.stake));
    }

    /// A staking transaction just sent, at the head of the history. The answer is its txid.
    fn staking_tx(&mut self, kind: TxKind, value: u64) -> String {
        let mut tx = self.payment(kind, value, Some(STAKE_FEE), None, None, self.tip + 1);
        for event in &mut tx.wallet_events {
            event.kind = kind.long().into();
        }
        let txid = tx.txid.clone();
        self.data.txs.insert(0, tx);
        txid
    }

    /// A payment just made or just received, still in the mempool.
    fn payment(
        &mut self,
        kind: TxKind,
        value: u64,
        fee: Option<u64>,
        memo: Option<&str>,
        address: Option<String>,
        height: u32,
    ) -> TxRow {
        let role = if kind == TxKind::Received {
            OutputRole::Received
        } else {
            OutputRole::Sent
        };
        TxRow {
            txid: self.rng.hex(64),
            datetime: Self::now_unix() as u32,
            height,
            status: TxStatus::Pending,
            kind,
            value,
            fee,
            inputs: Vec::new(),
            outputs: vec![OutputRow {
                pool: Pool::Orchard,
                role,
                value,
                index: 0,
                address,
                memo: memo.map(str::to_string),
                spend: SpendState::Unspent,
                scope: Some("External".into()),
            }],
            wallet_events: vec![WalletEventRow {
                kind: if kind == TxKind::Received {
                    "received".into()
                } else {
                    "sent".into()
                },
                value,
                recipient: None,
                pool_received: Some("orchard".into()),
                memos: memo.map(str::to_string).into_iter().collect(),
            }],
        }
    }

    /// Starts a session: the plan, then progress on every tick.
    fn launch(&mut self) {
        self.next_launch = None;
        self.scanned = 0;
        self.ticks = 0;
        self.mode = SyncMode::Running;
        self.note(SyncNote::Mode(SyncMode::Running));
        self.note(SyncNote::SessionStarted {
            tip: self.tip,
            birthday: self.birthday,
            total_blocks: u64::from(self.tip - self.birthday),
            scanned_blocks: 0,
            total_outputs: OUTPUTS,
            scanned_outputs: 0,
        });
        self.note(SyncNote::ScanPlanUpdated {
            ranges: self.plan(0.0),
        });
    }

    /// The scheduler's plan at `fraction` of the way: scanned behind, live now, the rest ahead.
    fn plan(&self, fraction: f64) -> Vec<ChainRange> {
        let span = u64::from(self.tip - self.birthday);
        let at = self.birthday + (span as f64 * fraction) as u32;
        let live_end = (at + 2_000).min(self.tip);
        let mut ranges = vec![ChainRange {
            start: self.tip.saturating_sub(100),
            end: self.tip,
            priority: RangePriority::ChainTip,
        }];
        if at > self.birthday {
            ranges.push(ChainRange {
                start: self.birthday,
                end: at,
                priority: RangePriority::Scanned,
            });
        }
        if live_end > at {
            ranges.push(ChainRange {
                start: at,
                end: live_end,
                priority: RangePriority::Scanning,
            });
        }
        if self.tip.saturating_sub(100) > live_end {
            ranges.push(ChainRange {
                start: live_end,
                end: self.tip.saturating_sub(100),
                priority: RangePriority::Historic,
            });
        }
        ranges
    }

    fn on_tick(&mut self) {
        if !self.opened {
            return;
        }
        match self.mode {
            SyncMode::Running => {
                self.ticks += 1;
                self.scanned = (self.scanned + PACE).min(OUTPUTS);
                let fraction = self.scanned as f64 / OUTPUTS as f64;
                let blocks = (u64::from(self.tip - self.birthday) as f64 * fraction) as u64;
                self.note(SyncNote::Progress {
                    total_blocks: u64::from(self.tip - self.birthday),
                    scanned_blocks: blocks,
                    total_outputs: OUTPUTS,
                    scanned_outputs: self.scanned,
                });
                if self.ticks.is_multiple_of(4) {
                    self.note(SyncNote::ScanPlanUpdated {
                        ranges: self.plan(fraction),
                    });
                }
                // a find now and then, on the first session only, so the list grows as you watch
                if self.sessions == 0 && self.ticks.is_multiple_of(9) && self.data.txs.len() < 40 {
                    let tx = self.fresh_tx(fraction);
                    self.note(SyncNote::TxDiscovered {
                        txid: tx.txid.clone(),
                        confirmed: true,
                    });
                    self.data.txs.insert(0, tx);
                    self.data.txs.sort_by(|a, b| b.datetime.cmp(&a.datetime));
                    self.send(Action::DataLoaded(self.data.clone()));
                }
                if self.scanned >= OUTPUTS {
                    self.mode = SyncMode::NotRunning;
                    self.sessions += 1;
                    self.note(SyncNote::Finished {
                        blocks_scanned: u64::from(self.tip - self.birthday),
                    });
                    self.note(SyncNote::Mode(SyncMode::NotRunning));
                    if self.auto {
                        let at = Instant::now() + RESYNC;
                        self.next_launch = Some(at);
                        self.note(SyncNote::Scheduled { at });
                    }
                }
            }
            SyncMode::NotRunning => {
                if self.auto && self.next_launch.is_some_and(|at| at <= Instant::now()) {
                    // the chain moved on while we rested
                    self.tip += 60;
                    self.note(SyncNote::TipMoved { to: self.tip });
                    self.launch();
                }
            }
            SyncMode::Paused | SyncMode::Stopping => {}
        }
    }

    /// A transaction found mid-scan: received, with a memo.
    fn fresh_tx(&mut self, fraction: f64) -> TxRow {
        let height = self.birthday + (f64::from(self.tip - self.birthday) * fraction) as u32;
        let value = 50_000_000 + self.rng.below(400_000_000);
        let memos = [
            "staking reward, epoch 412",
            "thanks for running a finalizer",
            "coffee money",
            "for the nightly test",
            "",
        ];
        let memo = memos[self.rng.below(memos.len() as u64) as usize];
        let txid = self.rng.hex(64);
        TxRow {
            txid: txid.clone(),
            datetime: (Self::now_unix() - u64::from(self.tip - height) * 15) as u32,
            height,
            status: TxStatus::Confirmed(height),
            kind: TxKind::Received,
            value,
            fee: None,
            inputs: Vec::new(),
            outputs: vec![OutputRow {
                pool: Pool::Orchard,
                role: OutputRole::Received,
                value,
                index: 0,
                address: None,
                memo: (!memo.is_empty()).then(|| memo.to_string()),
                spend: SpendState::Unspent,
                scope: Some("External".into()),
            }],
            wallet_events: vec![WalletEventRow {
                kind: "received".into(),
                value,
                recipient: None,
                pool_received: Some("orchard".into()),
                memos: if memo.is_empty() {
                    Vec::new()
                } else {
                    vec![memo.to_string()]
                },
            }],
        }
    }

    /// The wallet's history, balance and addresses.
    fn make_data(&mut self) -> WalletData {
        let unified = format!("utest1{}", self.rng.bech(140));
        let unified2 = format!("utest1{}", self.rng.bech(140));
        let transparent = format!("tm{}", self.rng.bech(33));
        let addresses = vec![
            AddressRow {
                kind: AddressKind::Unified {
                    orchard: true,
                    sapling: true,
                    transparent: true,
                },
                index: 0,
                address: unified.clone(),
                received: 2_312_500_000,
            },
            AddressRow {
                kind: AddressKind::Unified {
                    orchard: true,
                    sapling: false,
                    transparent: false,
                },
                index: 1,
                address: unified2,
                received: 150_000_000,
            },
            AddressRow {
                kind: AddressKind::Transparent {
                    scope: "External".into(),
                },
                index: 0,
                address: transparent.clone(),
                received: 30_000_000,
            },
        ];
        // a story over the last two months, newest first
        let now = Self::now_unix();
        let day = 86_400u64;
        let mut txs = Vec::new();
        let at = |days_ago: u64, hours: u64| -> (u32, u32) {
            let t = now - days_ago * day - hours * 3600;
            let height = self.tip.saturating_sub(((now - t) / 15) as u32);
            (t as u32, height)
        };
        let steps: Vec<Story> = vec![
            (
                0,
                1,
                TxKind::Received,
                42_000_000,
                None,
                "staking reward, epoch 418",
            ),
            (0, 5, TxKind::CreateBond, 10_000_000_000, Some(20_000), ""),
            (
                1,
                3,
                TxKind::Sent,
                125_000_000,
                Some(15_000),
                "rent, october",
            ),
            (2, 9, TxKind::Received, 500_000_000, None, "from the faucet"),
            (3, 2, TxKind::RetargetBond, 5_000_000_000, Some(20_000), ""),
            (5, 7, TxKind::SentToSelf, 80_000_000, Some(10_000), ""),
            (6, 1, TxKind::Shield, 30_000_000, Some(10_000), ""),
            (8, 4, TxKind::WithdrawBond, 2_500_000_000, Some(20_000), ""),
            (9, 6, TxKind::BeginUnbond, 2_500_000_000, Some(20_000), ""),
            (
                12,
                2,
                TxKind::Received,
                1_000_000_000,
                None,
                "welcome to crosslink",
            ),
            (15, 8, TxKind::Sent, 7_500_000, Some(10_000), "coffee"),
            (19, 3, TxKind::Received, 95_000_000, None, ""),
            (23, 5, TxKind::CreateBond, 15_000_000_000, Some(20_000), ""),
            (
                27,
                1,
                TxKind::Received,
                20_000_000_000,
                None,
                "delegator stake",
            ),
            (
                31,
                7,
                TxKind::Sent,
                60_000_000,
                Some(10_000),
                "thanks for the review",
            ),
            (36, 4, TxKind::Received, 310_000_000, None, "reward"),
            (41, 9, TxKind::Received, 12_500_000, None, ""),
            (48, 2, TxKind::Sent, 200_000_000, Some(15_000), ""),
            (
                55,
                6,
                TxKind::Received,
                2_500_000_000,
                None,
                "initial funding",
            ),
        ];
        for (i, (days, hours, kind, value, fee, memo)) in steps.into_iter().enumerate() {
            let (datetime, height) = at(days, hours);
            let txid = self.rng.hex(64);
            let status = if i == 0 {
                TxStatus::Pending
            } else {
                TxStatus::Confirmed(height)
            };
            let pool = match i % 3 {
                0 => Pool::Ironwood,
                1 => Pool::Sapling,
                _ => Pool::Orchard,
            };
            let memo_opt = (!memo.is_empty()).then(|| memo.to_string());
            let (inputs, outputs, transfers) = match kind {
                TxKind::Received => (
                    Vec::new(),
                    vec![OutputRow {
                        pool,
                        role: OutputRole::Received,
                        value,
                        index: 0,
                        address: Some(unified.clone()),
                        memo: memo_opt.clone(),
                        spend: if i % 4 == 1 {
                            SpendState::Spent(self.rng.hex(64))
                        } else {
                            SpendState::Unspent
                        },
                        scope: Some("External".into()),
                    }],
                    vec![WalletEventRow {
                        kind: "received".into(),
                        value,
                        recipient: None,
                        pool_received: Some(pool.name().into()),
                        memos: memo_opt.clone().into_iter().collect(),
                    }],
                ),
                TxKind::Sent => (
                    vec![InputRow {
                        pool,
                        value: value + fee.unwrap_or(0) + 1_250_000,
                        source_txid: self.rng.hex(64),
                        output_index: 0,
                        memo: None,
                        pending: false,
                    }],
                    vec![
                        OutputRow {
                            pool,
                            role: OutputRole::Sent,
                            value,
                            index: 0,
                            address: Some(format!("utest1{}", self.rng.bech(140))),
                            memo: memo_opt.clone(),
                            spend: SpendState::Unspent,
                            scope: None,
                        },
                        OutputRow {
                            pool,
                            role: OutputRole::Change,
                            value: 1_250_000,
                            index: 1,
                            address: None,
                            memo: None,
                            spend: SpendState::Unspent,
                            scope: Some("Internal".into()),
                        },
                    ],
                    vec![WalletEventRow {
                        kind: "sent".into(),
                        value,
                        recipient: Some(format!("utest1{}", self.rng.bech(140))),
                        pool_received: None,
                        memos: memo_opt.clone().into_iter().collect(),
                    }],
                ),
                _ => (
                    vec![InputRow {
                        pool: Pool::Orchard,
                        value: value + fee.unwrap_or(0),
                        source_txid: self.rng.hex(64),
                        output_index: 0,
                        memo: None,
                        pending: false,
                    }],
                    vec![OutputRow {
                        pool: Pool::Orchard,
                        role: OutputRole::ToSelf,
                        value: if matches!(kind, TxKind::CreateBond | TxKind::RetargetBond) {
                            0
                        } else {
                            value
                        },
                        index: 0,
                        address: None,
                        memo: None,
                        spend: SpendState::Unspent,
                        scope: Some("Internal".into()),
                    }],
                    vec![WalletEventRow {
                        kind: kind.long().into(),
                        value,
                        recipient: None,
                        pool_received: Some("orchard".into()),
                        memos: Vec::new(),
                    }],
                ),
            };
            txs.push(TxRow {
                txid,
                datetime,
                height,
                status,
                kind,
                value,
                fee,
                inputs,
                outputs,
                wallet_events: transfers,
            });
        }
        let balance = Balance {
            ironwood: PoolBalance {
                confirmed: 3_000_000_000,
                pending: 0,
            },
            orchard: PoolBalance {
                confirmed: 1_250_000_000,
                pending: 42_000_000,
            },
            sapling: PoolBalance {
                confirmed: 98_000_000,
                pending: 0,
            },
            transparent: PoolBalance {
                confirmed: 30_000_000,
                pending: 0,
            },
            // set from the bonds below
            staked: 0,
            unbonding: 0,
        };
        // three finalizers: two bonds earning, one on its way out; each created with a power
        // of ten and grown by its rewards since
        let bonds: Vec<BondRow> = [
            (10_000_000_000, 42_000_000, BondStatus::Bonded, 23u64),
            (10_000_000_000, 3_100_000, BondStatus::Bonded, 0),
            (1_000_000_000, 12_500_000, BondStatus::Unbonding, 9),
        ]
        .into_iter()
        .map(|(initial, earned, status, days_ago)| BondRow {
            key: self.rng.hex(64),
            finalizer: self.rng.finalizer(),
            amount: initial + earned,
            initial,
            status,
            since: at(days_ago, 0).1,
        })
        .collect();
        let sum = |status: BondStatus| -> u64 {
            bonds
                .iter()
                .filter(|b| b.status == status)
                .map(|b| b.amount)
                .sum()
        };
        let balance = Balance {
            staked: sum(BondStatus::Bonded),
            unbonding: sum(BondStatus::Unbonding),
            ..balance
        };
        // the chain's roster: ours among seven, with everyone's stake
        let mut roster: Vec<RosterRow> = bonds
            .iter()
            .zip([180_000_000_000u64, 95_000_000_000, 41_000_000_000])
            .map(|(bond, stake)| RosterRow {
                finalizer: bond.finalizer.clone(),
                stake,
                bonds: 2,
                ours: true,
            })
            .collect();
        for stake in [
            260_000_000_000u64,
            120_000_000_000,
            64_000_000_000,
            8_000_000_000,
        ] {
            roster.push(RosterRow {
                finalizer: self.rng.finalizer(),
                stake,
                bonds: 1,
                ours: false,
            });
        }
        roster.sort_by(|a, b| b.stake.cmp(&a.stake));
        WalletData {
            balance,
            txs,
            bonds,
            roster,
            addresses,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::app::{Phase, State, reduce};

    /// Pumps backend actions through the reducer until `done` holds, forwarding the
    /// commands the reducer asks for.
    async fn drive(
        state: &mut State,
        actions: &mut mpsc::UnboundedReceiver<Action>,
        commands: &mpsc::UnboundedSender<Command>,
        done: impl Fn(&State) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(state) {
            let left = deadline.saturating_duration_since(Instant::now());
            let action = tokio::time::timeout(left, actions.recv())
                .await
                .expect("the fake backend went quiet")
                .expect("the fake backend closed its channel");
            for command in reduce(state, action) {
                if !matches!(command, Command::Copy(_) | Command::ForceQuit) {
                    commands.send(command).unwrap();
                }
            }
        }
    }

    #[tokio::test]
    async fn the_fake_backend_walks_the_whole_flow_without_a_network() {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (act_tx, mut act_rx) = mpsc::unbounded_channel();
        let task = spawn(Network::Stable, cmd_rx, act_tx);
        let mut s = State::new(
            Phase::Onboarding(Box::new(crate::app::Onboarding::new(Network::Stable, None))),
            Instant::now(),
        );
        cmd_tx.send(Command::ListWallets).unwrap();
        drive(
            &mut s,
            &mut act_rx,
            &cmd_tx,
            |s| matches!(&s.phase, Phase::Onboarding(o) if o.listed),
        )
        .await;
        let Phase::Onboarding(o) = &s.phase else {
            panic!()
        };
        assert_eq!(o.wallets.len(), 3);
        assert_eq!(o.wallets[2].network, Network::Nightly);
        // the menu's create branch gets a seed and a tip
        cmd_tx.send(Command::FetchTip(Network::Stable)).unwrap();
        cmd_tx.send(Command::GenerateSeed).unwrap();
        drive(
            &mut s,
            &mut act_rx,
            &cmd_tx,
            |s| matches!(&s.phase, Phase::Onboarding(o) if o.tip.is_some() && o.seed.is_some()),
        )
        .await;
        // opening the first wallet lands on home with data and a running sync
        cmd_tx
            .send(Command::Open(OpenSpec::Read {
                network: Network::Stable,
                name: "main".into(),
                verified: true,
            }))
            .unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.phase == Phase::Main && !s.txs.is_empty() && s.sync.mode == SyncMode::Running
        })
        .await;
        let info = s.wallet.as_ref().unwrap();
        assert_eq!(info.wallet_name, "main");
        assert_eq!(info.kind, WalletKind::Seed);
        assert!(s.balance.staked > 0, "bonded funds show");
        assert!(s.balance.unbonding > 0, "unbonding funds show");
        assert_eq!(s.bonds.len(), 3, "the bonds are listed");
        assert_eq!(s.roster.len(), 7, "the roster is listed");
        assert_eq!(s.roster.iter().filter(|r| r.ours).count(), 3);
        assert!(s.txs.iter().any(|t| t.kind == TxKind::CreateBond));
        assert!(s.txs.iter().any(|t| t.first_memo().is_some()));
        assert!(s.addresses.iter().any(|a| a.kind.is_unified()));
        assert!(s.addresses.iter().any(|a| !a.kind.is_unified()));
        // progress moves, and a new address arrives on request
        let before = s.sync.blocks_scanned;
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.sync.blocks_scanned > before
        })
        .await;
        let addresses = s.addresses.len();
        // the faucet pays, and a send goes through the proposal
        cmd_tx
            .send(Command::Faucet {
                address: "u1faucet".into(),
            })
            .unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].first_memo() == Some("from the faucet")
        })
        .await;
        cmd_tx
            .send(Command::ProposeSend {
                to: "u1".to_string() + &"x".repeat(40),
                amount: 100_000_000,
                memo: Some("rent".into()),
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmSend).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].kind == TxKind::Sent && s.txs[0].first_memo() == Some("rent")
        })
        .await;
        // the faucet pays, and a send goes through the proposal
        cmd_tx
            .send(Command::Faucet {
                address: "u1faucet".into(),
            })
            .unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].first_memo() == Some("from the faucet")
        })
        .await;
        cmd_tx
            .send(Command::ProposeSend {
                to: "u1".to_string() + &"x".repeat(40),
                amount: 100_000_000,
                memo: Some("rent".into()),
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmSend).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].kind == TxKind::Sent && s.txs[0].first_memo() == Some("rent")
        })
        .await;
        // the faucet pays, and a send goes through the proposal
        cmd_tx
            .send(Command::Faucet {
                address: "u1faucet".into(),
            })
            .unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].first_memo() == Some("from the faucet")
        })
        .await;
        cmd_tx
            .send(Command::ProposeSend {
                to: "u1".to_string() + &"x".repeat(40),
                amount: 100_000_000,
                memo: Some("rent".into()),
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmSend).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].kind == TxKind::Sent && s.txs[0].first_memo() == Some("rent")
        })
        .await;
        // the faucet pays, and a send goes through the proposal
        cmd_tx
            .send(Command::Faucet {
                address: "u1faucet".into(),
            })
            .unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].first_memo() == Some("from the faucet")
        })
        .await;
        cmd_tx
            .send(Command::ProposeSend {
                to: "u1".to_string() + &"x".repeat(40),
                amount: 100_000_000,
                memo: Some("rent".into()),
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmSend).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].kind == TxKind::Sent && s.txs[0].first_memo() == Some("rent")
        })
        .await;
        // the faucet pays, and a send goes through the proposal
        cmd_tx
            .send(Command::Faucet {
                address: "u1faucet".into(),
            })
            .unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].first_memo() == Some("from the faucet")
        })
        .await;
        cmd_tx
            .send(Command::ProposeSend {
                to: "u1".to_string() + &"x".repeat(40),
                amount: 100_000_000,
                memo: Some("rent".into()),
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmSend).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.txs[0].kind == TxKind::Sent && s.txs[0].first_memo() == Some("rent")
        })
        .await;
        // a bond of any amount but a power of ten is refused
        let to = s.roster.iter().find(|r| !r.ours).unwrap().finalizer.clone();
        let (fake_tx, mut fake_rx) = mpsc::unbounded_channel();
        let mut fake = Fake::new(Network::Stable, fake_tx);
        fake.handle(Command::ProposeStake {
            finalizer: to.clone(),
            amount: 150_000_000,
        })
        .await;
        assert!(matches!(
            fake_rx.recv().await,
            Some(Action::StakeProposed(Err(e))) if e.contains("not a power of ten")
        ));
        // a new bond, then its unbond, a redelegation of another, and a withdrawal
        cmd_tx
            .send(Command::ProposeStake {
                finalizer: to.clone(),
                amount: 100_000_000,
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmStake).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| s.bonds.len() == 4).await;
        assert_eq!(s.txs[0].kind, TxKind::CreateBond);
        assert!(s.roster.iter().any(|r| r.finalizer == to && r.ours));
        let fresh = s.bonds[3].key.clone();
        cmd_tx
            .send(Command::ProposeUnbond {
                bond: fresh.clone(),
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmStake).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.bonds[3].status == BondStatus::Unbonding
        })
        .await;
        let first = s.bonds[0].clone();
        cmd_tx
            .send(Command::ProposeRedelegate {
                bond: first.key.clone(),
                to: to.clone(),
            })
            .unwrap();
        cmd_tx.send(Command::ConfirmStake).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| s.bonds[0].finalizer == to).await;
        assert_eq!(s.txs[0].kind, TxKind::RetargetBond);
        cmd_tx.send(Command::Withdraw { bond: fresh }).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| s.bonds.len() == 3).await;
        assert_eq!(s.txs[0].kind, TxKind::WithdrawBond);
        cmd_tx.send(Command::NewTransparentAddress).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| {
            s.addresses.len() > addresses
        })
        .await;
        cmd_tx.send(Command::Quit).unwrap();
        drive(&mut s, &mut act_rx, &cmd_tx, |s| s.exit).await;
        task.await.unwrap();
    }
}
