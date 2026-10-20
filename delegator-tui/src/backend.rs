//! The wallet task. Owns the `LightClient`, runs the sync lifecycle, and translates wallet
//! state into [`Action`]s for the reducer. The UI never touches the client directly.
//!
//! pepper-sync on this branch publishes no event stream, so the task reads the wallet's sync
//! status on every tick and reports what changed since the last one.

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pepper_sync::keys::transparent::TransparentScope;
use pepper_sync::sync::{ScanPriority, ScanRange};
use pepper_sync::wallet::{OrchardNote, OutputInterface, SaplingNote, TransparentCoin};
use tokio::sync::mpsc;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;
use zingolib::config::{ChainType, ZingoConfig, chain_from_str};
use zingolib::data::PollReport;
use zingolib::lightclient::LightClient;
use zingolib::wallet::error::BalanceError;
use zingolib::wallet::keys::unified::ReceiverSelection;
use zingolib::wallet::output::SpendStatus;
use zingolib::wallet::summary::data::{
    SelfSendValueTransfer, SendType, SentValueTransfer, TransactionKind, TransactionSummary,
    ValueTransfer, ValueTransferKind,
};
use zingolib::wallet::{LightWallet, WalletBase, WalletSettings};

use crate::app::{
    Action, AddressKind, AddressRow, Balance, ChainRange, Command, InputRow, Network, OpenSpec,
    OutputRole, OutputRow, Pool, PoolBalance, RangePriority, SpendState, SyncMode, SyncNote,
    TxKind, TxRow, TxStatus, ValueTransferRow, WalletData, WalletInfo,
};

/// Pause between the end of one sync session and the start of the next.
const RESYNC_INTERVAL: Duration = Duration::from_secs(20);
/// Pause before retrying after a failed session.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(500);
/// Longest we wait for a running session to stop on quit.
const STOP_TIMEOUT: Duration = Duration::from_secs(120);
const SAVE_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed facts the backend needs before a wallet exists.
#[derive(Clone, Debug)]
pub struct BackendConfig {
    pub network: Network,
    pub data_dir: Option<PathBuf>,
    pub wallet_name: String,
    pub server: Option<String>,
}

impl BackendConfig {
    /// The wallet directory for `network`: the flag if given, else zingolib's default.
    pub fn wallet_dir(&self, network: Network) -> PathBuf {
        match &self.data_dir {
            Some(dir) => dir.clone(),
            None => ZingoConfig::build(chain_type(network))
                .create()
                .get_zingo_wallet_dir()
                .to_path_buf(),
        }
    }

    pub fn wallet_path(&self, network: Network) -> PathBuf {
        self.wallet_dir(network).join(&self.wallet_name)
    }

    /// The wallet file itself does not record its server, so it is kept in a file beside it.
    fn server_path(&self, network: Network) -> PathBuf {
        self.wallet_dir(network)
            .join(format!("{}.server", self.wallet_name))
    }

    fn saved_server(&self, network: Network) -> Option<String> {
        std::fs::read_to_string(self.server_path(network))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn save_server(&self, network: Network, server: &str) -> std::io::Result<()> {
        std::fs::write(self.server_path(network), format!("{server}\n"))
    }

    /// The server to use, in order: the one given for this call, the command line flag, the
    /// one saved with the wallet, and the network default.
    fn server_uri(
        &self,
        network: Network,
        override_server: Option<&str>,
    ) -> Result<http::Uri, String> {
        let raw = override_server
            .map(str::to_string)
            .or_else(|| self.server.clone())
            .or_else(|| self.saved_server(network))
            .unwrap_or_else(|| network.default_server().to_string());
        parse_server(&raw)
    }

    fn client_config(&self, network: Network, server: http::Uri) -> ZingoConfig {
        let mut config = ZingoConfig::build(chain_type(network))
            .set_lightwalletd_uri(server)
            .set_wallet_dir(self.wallet_dir(network))
            .set_wallet_settings(wallet_settings())
            .create();
        config.wallet_name = PathBuf::from(&self.wallet_name);
        config
    }
}

/// Reads a server URL the way zingo-cli does: a bare host gets `http://`, and plain http
/// without a port gets lightwalletd's default port. It never panics on bad input.
fn parse_server(raw: &str) -> Result<http::Uri, String> {
    let with_scheme = if raw.starts_with("http") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let uri: http::Uri = with_scheme
        .parse()
        .map_err(|e| format!("invalid server URL {raw}: {e}"))?;
    if uri.port().is_none() && uri.scheme_str() == Some("http") {
        format!("{with_scheme}:9067")
            .parse()
            .map_err(|e| format!("invalid server URL {raw}: {e}"))
    } else {
        Ok(uri)
    }
}

/// The chain parameters zingolib uses on this branch. Its testnet is the crosslink testnet,
/// where every upgrade is active from block 1, not the public Zcash testnet.
pub fn chain_type(network: Network) -> ChainType {
    match network {
        Network::Mainnet => ChainType::Mainnet,
        Network::Testnet => chain_from_str("testnet").expect("a known chain name"),
        Network::Regtest => chain_from_str("regtest").expect("a known chain name"),
    }
}

fn wallet_settings() -> WalletSettings {
    use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
    WalletSettings {
        sync_config: SyncConfig {
            transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            performance_level: PerformanceLevel::High,
        },
        min_confirmations: NonZeroU32::new(3).expect("non-zero"),
    }
}

/// Spawns the backend task. Commands arrive on `commands`, actions leave on `actions`.
pub fn spawn(
    config: BackendConfig,
    commands: mpsc::UnboundedReceiver<Command>,
    actions: mpsc::UnboundedSender<Action>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(Backend::new(config, actions).run(commands))
}

/// What the wallet says about the sync, read on one tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SyncSnapshot {
    /// The sync engine has set up a session at least once.
    started: bool,
    birthday: Option<u32>,
    tip: Option<u32>,
    ranges: Vec<ChainRange>,
    /// Estimated from the percentage the engine reports; zero until it reports one.
    total_outputs: u64,
    scanned_outputs: u64,
    scanned_blocks: u64,
    /// Every transaction the wallet knows, with whether it is confirmed.
    txs: Vec<(String, bool)>,
}

struct Backend {
    config: BackendConfig,
    actions: mpsc::UnboundedSender<Action>,
    lc: Option<LightClient>,
    network: Network,
    auto: bool,
    next_launch: Option<Instant>,
    dirty: bool,
    last_mode: SyncMode,
    /// A session was launched and its start has not been reported yet.
    session_pending: bool,
    known_txids: HashSet<String>,
    last: SyncSnapshot,
    /// The bonded total from the last time the server answered.
    last_staked: u64,
}

impl Backend {
    fn new(config: BackendConfig, actions: mpsc::UnboundedSender<Action>) -> Self {
        Self {
            network: config.network,
            config,
            actions,
            lc: None,
            auto: true,
            next_launch: None,
            dirty: false,
            last_mode: SyncMode::NotRunning,
            session_pending: false,
            known_txids: HashSet::new(),
            last: SyncSnapshot::default(),
            last_staked: 0,
        }
    }

    fn send(&self, action: Action) {
        let _ = self.actions.send(action);
    }

    fn sync_note(&self, note: SyncNote) {
        self.send(Action::Sync(note, Instant::now()));
    }

    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    None => break,
                    Some(cmd) => {
                        if self.handle(cmd).await {
                            break;
                        }
                    }
                },
                _ = tick.tick() => self.on_tick().await,
            }
        }
    }

    /// Returns `true` when the task should exit.
    async fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Open(spec) => {
                if self.lc.is_some() {
                    return false;
                }
                match self.open(spec).await {
                    Ok((lc, info)) => {
                        self.lc = Some(lc);
                        self.send(Action::WalletOpened(info));
                        if let Some(lc) = self.lc.as_mut() {
                            lc.save_task().await;
                        }
                        self.launch().await;
                        self.refresh().await;
                    }
                    Err(message) => self.send(Action::OpenFailed(message)),
                }
            }
            Command::Refresh => self.refresh().await,
            Command::NewUnifiedAddress { orchard, sapling } => {
                if let Some(lc) = self.lc.as_mut() {
                    match lc
                        .generate_unified_address(
                            ReceiverSelection { orchard, sapling },
                            zip32::AccountId::ZERO,
                        )
                        .await
                    {
                        Ok((id, _)) => {
                            self.send(Action::Notice(format!(
                                "new unified address #{}",
                                id.address_index
                            )));
                            self.refresh().await;
                        }
                        Err(e) => self.send(Action::Notice(format!("cannot derive address: {e}"))),
                    }
                }
            }
            Command::NewTransparentAddress => {
                if let Some(lc) = self.lc.as_mut() {
                    match lc
                        .generate_transparent_address(zip32::AccountId::ZERO, false)
                        .await
                    {
                        Ok((id, _)) => {
                            self.send(Action::Notice(format!(
                                "new transparent address #{}",
                                id.address_index().index()
                            )));
                            self.refresh().await;
                        }
                        Err(e) => self.send(Action::Notice(format!("cannot derive address: {e}"))),
                    }
                }
            }
            Command::PauseSync => {
                if let Some(lc) = self.lc.as_ref() {
                    match lc.sync_mode() {
                        pepper_sync::wallet::SyncMode::Running => {
                            if let Err(e) = lc.pause_sync() {
                                self.send(Action::Notice(format!("cannot pause: {e}")));
                            }
                        }
                        pepper_sync::wallet::SyncMode::NotRunning => {
                            self.auto = false;
                            self.next_launch = None;
                            self.sync_note(SyncNote::Auto(false));
                        }
                        _ => {}
                    }
                }
            }
            Command::ResumeSync => {
                if let Some(lc) = self.lc.as_ref() {
                    match lc.sync_mode() {
                        pepper_sync::wallet::SyncMode::Paused => {
                            if let Err(e) = lc.resume_sync() {
                                self.send(Action::Notice(format!("cannot resume: {e}")));
                            }
                        }
                        pepper_sync::wallet::SyncMode::NotRunning => {
                            self.auto = true;
                            self.sync_note(SyncNote::Auto(true));
                            self.launch().await;
                        }
                        _ => {}
                    }
                }
            }
            Command::Rescan => {
                if self.lc.is_none() {
                    return false;
                }
                self.sync_note(SyncNote::Reset);
                self.auto = true;
                self.next_launch = None;
                self.known_txids.clear();
                self.last = SyncSnapshot::default();
                self.sync_note(SyncNote::Auto(true));
                let rescanned = self.lc.as_mut().expect("checked above").rescan().await;
                match rescanned {
                    Ok(()) => {
                        self.session_pending = true;
                        self.dirty = true;
                    }
                    Err(e) => {
                        self.sync_note(SyncNote::Failed(e.to_string()));
                        let at = Instant::now() + RETRY_INTERVAL;
                        self.next_launch = Some(at);
                        self.sync_note(SyncNote::Scheduled { at });
                    }
                }
                self.refresh().await;
            }
            Command::Quit => {
                self.shutdown().await;
                self.send(Action::Shutdown);
                return true;
            }
            Command::Copy(_) | Command::SaveTheme(_) | Command::ForceQuit => {}
        }
        false
    }

    async fn open(&mut self, spec: OpenSpec) -> Result<(LightClient, WalletInfo), String> {
        let (lc, network) = match spec {
            OpenSpec::Read => {
                let network = self.config.network;
                let server = self.config.server_uri(network, None)?;
                let config = self.config.client_config(network, server);
                let lc = LightClient::create_from_wallet_path(config)
                    .map_err(|e| format!("cannot open wallet: {e}"))?;
                (lc, network)
            }
            OpenSpec::Create {
                network,
                server,
                ufvk,
                birthday,
            } => {
                let path = self.config.wallet_path(network);
                if path.exists() {
                    return Err(format!(
                        "a wallet file already exists at {}",
                        path.display()
                    ));
                }
                let server_uri = self.config.server_uri(network, Some(&server))?;
                let config = self.config.client_config(network, server_uri);
                if let Err(e) = self.config.save_server(network, &server) {
                    tracing::warn!("cannot save the server beside the wallet: {e}");
                }
                let wallet = LightWallet::new(
                    config.chain,
                    WalletBase::Ufvk(ufvk),
                    BlockHeight::from_u32(birthday),
                    config.wallet_settings.clone(),
                )
                .map_err(|e| format!("cannot create wallet: {e}"))?;
                let lc = LightClient::create_from_wallet(wallet, config, false)
                    .map_err(|e| format!("cannot create wallet: {e}"))?;
                (lc, network)
            }
        };
        let (has_orchard, birthday, known) = {
            let wallet = lc.wallet.read().await;
            if wallet
                .unified_key_store
                .values()
                .any(|k| k.is_spending_key())
            {
                return Err(
                    "this wallet file holds spending keys; delegator-tui only opens watch-only wallets"
                        .into(),
                );
            }
            let Some(keys) = wallet.unified_key_store.get(&zip32::AccountId::ZERO) else {
                return Err("the wallet file has no viewing key".into());
            };
            (
                keys.default_receivers().is_some_and(|r| r.orchard),
                u32::from(wallet.birthday),
                wallet
                    .wallet_transactions
                    .keys()
                    .map(ToString::to_string)
                    .collect::<HashSet<_>>(),
            )
        };
        self.network = network;
        self.known_txids = known;
        self.last = SyncSnapshot::default();
        let info = WalletInfo {
            network,
            wallet_name: self.config.wallet_name.clone(),
            wallet_path: lc.config.get_wallet_pathbuf().display().to_string(),
            birthday,
            server: lc.indexer_uri().to_string(),
            has_orchard,
        };
        Ok((lc, info))
    }

    async fn launch(&mut self) {
        if self.lc.is_none() {
            return;
        }
        self.next_launch = None;
        let launched = self.lc.as_mut().expect("checked above").sync().await;
        match launched {
            Ok(()) => {
                tracing::info!("sync session launched");
                self.session_pending = true;
            }
            Err(e) => {
                tracing::warn!("sync could not start: {e}");
                self.sync_note(SyncNote::Failed(e.to_string()));
                if self.auto {
                    let at = Instant::now() + RETRY_INTERVAL;
                    self.next_launch = Some(at);
                    self.sync_note(SyncNote::Scheduled { at });
                }
            }
        }
    }

    async fn on_tick(&mut self) {
        let Some(poll) = self.lc.as_mut().map(LightClient::poll_sync) else {
            return;
        };
        match poll {
            PollReport::Ready(Ok(result)) => {
                tracing::info!("sync session finished: {result}");
                // the session's last progress, before the counters stop moving
                self.observe().await;
                self.session_pending = false;
                self.sync_note(SyncNote::Finished {
                    blocks_scanned: u64::from(result.blocks_scanned),
                });
                self.dirty = true;
                if self.auto {
                    let at = Instant::now() + RESYNC_INTERVAL;
                    self.next_launch = Some(at);
                    self.sync_note(SyncNote::Scheduled { at });
                }
            }
            PollReport::Ready(Err(e)) => {
                tracing::warn!("sync session failed: {e}");
                self.session_pending = false;
                self.sync_note(SyncNote::Failed(e.to_string()));
                self.dirty = true;
                if self.auto {
                    let at = Instant::now() + RETRY_INTERVAL;
                    self.next_launch = Some(at);
                    self.sync_note(SyncNote::Scheduled { at });
                }
            }
            PollReport::NotReady => self.observe().await,
            PollReport::NoHandle => {
                if self.auto && self.next_launch.is_some_and(|at| at <= Instant::now()) {
                    self.launch().await;
                }
            }
        }
        if let Some(mode) = self.lc.as_ref().map(|lc| match lc.sync_mode() {
            pepper_sync::wallet::SyncMode::NotRunning => SyncMode::NotRunning,
            pepper_sync::wallet::SyncMode::Running => SyncMode::Running,
            pepper_sync::wallet::SyncMode::Paused => SyncMode::Paused,
            pepper_sync::wallet::SyncMode::Shutdown => SyncMode::Stopping,
        }) && mode != self.last_mode
        {
            self.last_mode = mode;
            self.sync_note(SyncNote::Mode(mode));
        }
        let save = match self.lc.as_mut() {
            Some(lc) => lc.check_save_error().await,
            None => Ok(()),
        };
        if let Err(e) = save {
            self.send(Action::Notice(format!("wallet save failed: {e}")));
        }
        if self.dirty {
            self.dirty = false;
            self.refresh().await;
        }
    }

    /// Reads the sync state while a session runs and reports what moved since the last read.
    async fn observe(&mut self) {
        let Some(lc) = self.lc.as_ref() else { return };
        let now = snapshot(lc).await;
        if self.session_pending && now.started && now.tip.is_some() {
            self.session_pending = false;
            self.sync_note(SyncNote::SessionStarted {
                tip: now.tip.unwrap_or(0),
                birthday: now.birthday.unwrap_or(0),
                total_outputs: now.total_outputs,
                scanned_outputs: now.scanned_outputs,
                scanned_blocks: now.scanned_blocks,
            });
            // the session start cleared the plan; report it again below
            self.last.ranges.clear();
            self.last.tip = now.tip;
        }
        if now.ranges != self.last.ranges {
            self.sync_note(SyncNote::ScanPlanUpdated {
                ranges: now.ranges.clone(),
            });
        }
        if let Some(to) = now.tip
            && self.last.tip.is_some_and(|last| last != to)
        {
            self.dirty = true;
            self.sync_note(SyncNote::TipMoved { to });
        }
        let progress = |s: &SyncSnapshot| (s.total_outputs, s.scanned_outputs, s.scanned_blocks);
        if now.started && progress(&now) != progress(&self.last) {
            self.sync_note(SyncNote::Progress {
                total_outputs: now.total_outputs,
                scanned_outputs: now.scanned_outputs,
                scanned_blocks: now.scanned_blocks,
            });
        }
        for (txid, confirmed) in &now.txs {
            if self.known_txids.insert(txid.clone()) {
                self.dirty = true;
                self.sync_note(SyncNote::TxDiscovered {
                    txid: txid.clone(),
                    confirmed: *confirmed,
                });
            }
        }
        self.last = now;
    }

    async fn refresh(&mut self) {
        let Some(lc) = self.lc.as_ref() else { return };
        match load(lc, self.last_staked).await {
            Ok(data) => {
                self.last_staked = data.balance.staked;
                self.send(Action::DataLoaded(data));
            }
            Err(e) => self.send(Action::Notice(format!("cannot read wallet: {e}"))),
        }
    }

    async fn shutdown(&mut self) {
        self.auto = false;
        let Some(lc) = self.lc.as_mut() else { return };
        if lc.sync_mode() != pepper_sync::wallet::SyncMode::NotRunning {
            let _ = lc.stop_sync();
            let deadline = Instant::now() + STOP_TIMEOUT;
            while matches!(lc.poll_sync(), PollReport::NotReady) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        let _ = tokio::time::timeout(SAVE_TIMEOUT, lc.wait_for_save()).await;
        if let Err(e) = lc.shutdown_save_task().await {
            tracing::error!("final wallet save failed: {e}");
        }
    }
}

/// One read of everything the Sync screen is told about.
async fn snapshot(lc: &LightClient) -> SyncSnapshot {
    let wallet = lc.wallet.read().await;
    let status = pepper_sync::sync_status(&*wallet).await.ok();
    let txs = wallet
        .wallet_transactions
        .iter()
        .map(|(txid, tx)| (txid.to_string(), tx.status().is_confirmed()))
        .collect();
    let birthday = wallet.sync_state.wallet_birthday().map(u32::from);
    let tip = wallet.sync_state.last_known_chain_height().map(u32::from);
    let mut out = SyncSnapshot {
        birthday,
        tip,
        txs,
        ..SyncSnapshot::default()
    };
    if let Some(status) = status {
        let scanned = u64::from(status.total_sapling_outputs_scanned)
            + u64::from(status.total_orchard_outputs_scanned);
        let percent = f64::from(status.percentage_total_outputs_scanned);
        let total = if percent > 0.0 {
            (scanned as f64 * 100.0 / percent).round() as u64
        } else {
            0
        };
        out.started = u32::from(status.sync_start_height) != 0;
        out.ranges = status.scan_ranges.iter().map(chain_range).collect();
        out.total_outputs = total.max(scanned);
        out.scanned_outputs = scanned;
        out.scanned_blocks = u64::from(status.total_blocks_scanned);
    }
    out
}

fn chain_range(range: &ScanRange) -> ChainRange {
    ChainRange {
        start: u32::from(range.block_range().start),
        end: u32::from(range.block_range().end),
        priority: range_priority(range.priority()),
    }
}

fn range_priority(priority: ScanPriority) -> RangePriority {
    match priority {
        ScanPriority::RefetchingNullifiers => RangePriority::RefetchingNullifiers,
        ScanPriority::Scanning => RangePriority::Scanning,
        ScanPriority::Scanned => RangePriority::Scanned,
        ScanPriority::ScannedWithoutMapping => RangePriority::ScannedWithoutMapping,
        ScanPriority::Historic => RangePriority::Historic,
        ScanPriority::OpenAdjacent => RangePriority::OpenAdjacent,
        ScanPriority::FoundNote => RangePriority::FoundNote,
        ScanPriority::ChainTip => RangePriority::ChainTip,
        ScanPriority::Verify => RangePriority::Verify,
    }
}

/// A pool's confirmed and pending balance. A pool the key cannot view counts as empty.
fn pool_balance<Op: OutputInterface>(
    wallet: &LightWallet,
    account: zip32::AccountId,
) -> PoolBalance {
    fn zats(balance: Result<Zatoshis, BalanceError>) -> u64 {
        balance.map(Zatoshis::into_u64).unwrap_or(0)
    }
    PoolBalance {
        confirmed: zats(wallet.confirmed_balance_excluding_dust::<Op>(account)),
        pending: zats(wallet.unconfirmed_balance_excluding_dust::<Op>(account)),
    }
}

/// Value locked in the wallet's delegation bonds: the server's view of every bond the wallet
/// created and has not withdrawn. The wallet lock is released before the server is asked, and
/// `last` stands in when it cannot be reached.
async fn staked(lc: &LightClient, last: u64) -> u64 {
    use zcash_client_backend::proto::service::BondInfoRequest;
    use zcash_primitives::transaction::StakingActionKind;

    let bonds: Vec<([u8; 32], u64)> = {
        let wallet = lc.wallet.read().await;
        let actions: Vec<_> = wallet
            .wallet_transactions
            .values()
            .filter_map(|tx| tx.staking_data())
            .collect();
        let withdrawn: HashSet<[u8; 32]> = actions
            .iter()
            .filter(|a| a.kind() == StakingActionKind::WithdrawDelegationBond)
            .map(|a| a.bond_key())
            .collect();
        // `bond_terms` is `Some` only for the kinds that mint a bond.
        actions
            .iter()
            .filter(|a| !withdrawn.contains(&a.bond_key()))
            .filter_map(|a| {
                a.bond_terms()
                    .map(|(_, amount_zats, _)| (a.bond_key(), amount_zats))
            })
            .collect()
    };
    if bonds.is_empty() {
        return 0;
    }
    let mut client = match zingo_netutils::get_client(lc.indexer_uri()).await {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("cannot ask the server about bonds: {e:?}");
            return last;
        }
    };
    let mut total = 0u64;
    for (key, created_with) in bonds {
        match client
            .get_bond_info(BondInfoRequest {
                bond_key: key.to_vec(),
            })
            .await
        {
            // 2 means withdrawn on chain, whatever the wallet has seen so far
            Ok(info) => {
                let info = info.into_inner();
                if info.status != 2 {
                    total = total.saturating_add(info.amount);
                }
            }
            Err(e) => {
                tracing::warn!("cannot read a bond, using its creation amount: {e}");
                total = total.saturating_add(created_with);
            }
        }
    }
    total
}

/// Reads everything the screens show. `last_staked` is shown again if the server is silent.
async fn load(lc: &LightClient, last_staked: u64) -> Result<WalletData, String> {
    let account = zip32::AccountId::ZERO;
    let balance = {
        let wallet = lc.wallet.read().await;
        Balance {
            orchard: pool_balance::<OrchardNote>(&wallet, account),
            sapling: pool_balance::<SaplingNote>(&wallet, account),
            transparent: pool_balance::<TransparentCoin>(&wallet, account),
            staked: 0,
        }
    };
    let balance = Balance {
        staked: staked(lc, last_staked).await,
        ..balance
    };

    let summaries = lc
        .transaction_summaries(false)
        .await
        .map_err(|e| e.to_string())?;
    let transfers = lc.value_transfers(false).await.map_err(|e| e.to_string())?;
    let mut by_txid: std::collections::HashMap<String, Vec<ValueTransferRow>> =
        std::collections::HashMap::new();
    for transfer in transfers.iter() {
        by_txid
            .entry(transfer.txid.to_string())
            .or_default()
            .push(value_transfer_row(transfer));
    }
    let mut index = output_index(lc).await;
    let mut txs: Vec<TxRow> = summaries
        .iter()
        .map(|s| tx_row(s, &mut by_txid, &mut index))
        .collect();
    txs.sort_by(|a, b| b.height.cmp(&a.height).then(b.datetime.cmp(&a.datetime)));

    let received = lc
        .do_total_value_to_address()
        .await
        .map(|t| t.0)
        .unwrap_or_default();
    let mut addresses = Vec::new();
    let wallet = lc.wallet.read().await;
    for (id, unified_address) in wallet.unified_addresses() {
        let address = unified_address.encode(&wallet.network);
        addresses.push(AddressRow {
            kind: AddressKind::Unified {
                orchard: unified_address.has_orchard(),
                sapling: unified_address.has_sapling(),
                transparent: unified_address.has_transparent(),
            },
            index: id.address_index,
            received: received.get(&address).copied().unwrap_or(0),
            address,
        });
    }
    // change and refund addresses are the wallet's own plumbing; only receiving addresses
    // mean anything to the user
    for (id, address) in wallet
        .transparent_addresses()
        .iter()
        .filter(|(id, _)| id.scope() == TransparentScope::External)
    {
        addresses.push(AddressRow {
            kind: AddressKind::Transparent {
                scope: id.scope().to_string(),
            },
            index: id.address_index().index(),
            received: received.get(address).copied().unwrap_or(0),
            address: address.clone(),
        });
    }
    drop(wallet);

    Ok(WalletData {
        balance,
        txs,
        addresses,
    })
}

/// Wallet output facts the per-transaction summaries do not carry: which transaction consumed
/// each output, and whether an output was received externally or is change.
#[derive(Default)]
struct OutputIndex {
    /// Inputs keyed by the transaction that spent them.
    spent_by: std::collections::HashMap<String, Vec<InputRow>>,
    /// Scope keyed by the transaction that created the output, its pool, and its index.
    scopes: std::collections::HashMap<(String, Pool, u32), String>,
}

/// Walks every wallet output once, in every pool, to build [`OutputIndex`].
async fn output_index(lc: &LightClient) -> OutputIndex {
    use pepper_sync::wallet::{OrchardNote, SaplingNote};

    let wallet = lc.wallet.read().await;
    let mut index = OutputIndex::default();

    let pools = [
        (Pool::Orchard, wallet.note_summaries::<OrchardNote>(true)),
        (Pool::Sapling, wallet.note_summaries::<SaplingNote>(true)),
    ];
    for (pool, summaries) in pools {
        for note in summaries.iter() {
            let source_txid = note.txid.to_string();
            let output_index = u32::from(note.output_index);
            index.scopes.insert(
                (source_txid.clone(), pool, output_index),
                note.scope.to_string(),
            );
            if let Some((spender, pending)) = spend_target(&note.spend_status) {
                index.spent_by.entry(spender).or_default().push(InputRow {
                    pool,
                    value: note.value,
                    source_txid,
                    output_index,
                    memo: note.memo.clone(),
                    pending,
                });
            }
        }
    }
    for coin in wallet.coin_summaries(true) {
        let source_txid = coin.txid.to_string();
        let output_index = u32::from(coin.output_index);
        index.scopes.insert(
            (source_txid.clone(), Pool::Transparent, output_index),
            coin.scope.to_string(),
        );
        if let Some((spender, pending)) = spend_target(&coin.spend_status) {
            index.spent_by.entry(spender).or_default().push(InputRow {
                pool: Pool::Transparent,
                value: coin.value,
                source_txid,
                output_index,
                memo: None,
                pending,
            });
        }
    }
    index
}

/// The transaction consuming an output, and whether that spend is still unconfirmed.
fn spend_target(status: &SpendStatus) -> Option<(String, bool)> {
    match status {
        SpendStatus::Unspent => None,
        SpendStatus::Spent(txid) => Some((txid.to_string(), false)),
        SpendStatus::CalculatedSpent(txid)
        | SpendStatus::TransmittedSpent(txid)
        | SpendStatus::MempoolSpent(txid) => Some((txid.to_string(), true)),
    }
}

fn spend_state(status: &SpendStatus) -> SpendState {
    match spend_target(status) {
        None => SpendState::Unspent,
        Some((txid, true)) => SpendState::PendingSpent(txid),
        Some((txid, false)) => SpendState::Spent(txid),
    }
}

fn tx_kind(kind: TransactionKind) -> TxKind {
    match kind {
        TransactionKind::Received => TxKind::Received,
        TransactionKind::Sent(SendType::Send) => TxKind::Sent,
        TransactionKind::Sent(SendType::Shield) => TxKind::Shield,
        TransactionKind::Sent(SendType::SendToSelf) => TxKind::SentToSelf,
        TransactionKind::Sent(SendType::CreateBond) => TxKind::CreateBond,
        TransactionKind::Sent(SendType::BeginUnbond) => TxKind::BeginUnbond,
        TransactionKind::Sent(SendType::WithdrawBond) => TxKind::WithdrawBond,
        TransactionKind::Sent(SendType::RetargetDelegationBond) => TxKind::RetargetBond,
    }
}

fn tx_row(
    s: &TransactionSummary,
    transfers: &mut std::collections::HashMap<String, Vec<ValueTransferRow>>,
    index: &mut OutputIndex,
) -> TxRow {
    let txid = s.txid.to_string();
    let status = if s.status.is_confirmed() {
        TxStatus::Confirmed(u32::from(s.status.get_height()))
    } else if s.status.is_failed() {
        TxStatus::Failed
    } else {
        TxStatus::Pending
    };
    let kind = tx_kind(s.kind);

    let scope_of = |pool: Pool, output_index: u32| {
        index
            .scopes
            .get(&(txid.clone(), pool, output_index))
            .cloned()
    };
    let mut received = Vec::new();
    let notes = [
        (Pool::Orchard, &s.orchard_notes),
        (Pool::Sapling, &s.sapling_notes),
    ];
    for (pool, notes) in notes {
        for n in notes {
            received.push(OutputRow {
                pool,
                role: OutputRole::Received,
                value: n.value,
                index: n.output_index,
                address: None,
                memo: n.memo.clone(),
                spend: spend_state(&n.spend_status),
                scope: scope_of(pool, n.output_index),
            });
        }
    }
    for c in &s.transparent_coins {
        received.push(OutputRow {
            pool: Pool::Transparent,
            role: OutputRole::Received,
            value: c.value,
            index: c.output_index,
            address: None,
            memo: None,
            spend: spend_state(&c.spend_summary),
            scope: scope_of(Pool::Transparent, c.output_index),
        });
    }

    let mut outgoing = Vec::new();
    let notes = [
        (Pool::Orchard, &s.outgoing_orchard_notes),
        (Pool::Sapling, &s.outgoing_sapling_notes),
    ];
    for (pool, notes) in notes {
        for n in notes {
            outgoing.push(OutputRow {
                pool,
                role: OutputRole::Sent,
                value: n.value,
                index: u32::from(n.output_index),
                address: Some(
                    n.recipient_unified_address
                        .clone()
                        .unwrap_or_else(|| n.recipient.clone()),
                ),
                memo: n.memo.clone(),
                spend: SpendState::Unspent,
                scope: Some(n.scope.to_string()),
            });
        }
    }
    for c in &s.outgoing_transparent_coins {
        outgoing.push(OutputRow {
            pool: Pool::Transparent,
            role: OutputRole::Sent,
            value: c.value,
            index: u32::from(c.output_index),
            address: Some(c.recipient.clone()),
            memo: None,
            spend: SpendState::Unspent,
            scope: None,
        });
    }

    let mut inputs = index.spent_by.remove(&txid).unwrap_or_default();
    inputs.sort_by(|a, b| {
        a.source_txid
            .cmp(&b.source_txid)
            .then(a.output_index.cmp(&b.output_index))
    });
    let outputs = crate::app::roles::assign_roles(kind, received, outgoing);

    TxRow {
        value_transfers: transfers.remove(&txid).unwrap_or_default(),
        txid,
        datetime: s.datetime,
        height: u32::from(s.blockheight),
        status,
        kind,
        value: s.value,
        fee: s.fee,
        inputs,
        outputs,
    }
}

fn value_transfer_row(transfer: &ValueTransfer) -> ValueTransferRow {
    let kind = match transfer.kind {
        ValueTransferKind::Received => "received",
        ValueTransferKind::Sent(SentValueTransfer::Send) => "sent",
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(to_self)) => match to_self {
            SelfSendValueTransfer::Basic => "sent to self",
            SelfSendValueTransfer::Shield => "shield",
            SelfSendValueTransfer::MemoToSelf => "memo to self",
            SelfSendValueTransfer::Refund => "refund",
            SelfSendValueTransfer::Stake => "bond created",
            SelfSendValueTransfer::BeginUnbond => "unbond begun",
            SelfSendValueTransfer::WithdrawBond => "bond withdrawn",
            SelfSendValueTransfer::RetargetDelegationBond => "bond retargeted",
        },
    };
    ValueTransferRow {
        kind: kind.to_string(),
        value: transfer.value,
        recipient: transfer.recipient_address.clone(),
        pool_received: transfer.pool_received.clone(),
        memos: transfer.memos.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_are_completed_but_never_panic() {
        assert_eq!(
            parse_server("127.0.0.1").unwrap().to_string(),
            "http://127.0.0.1:9067/"
        );
        assert_eq!(
            parse_server("https://zec.rocks:443").unwrap().to_string(),
            "https://zec.rocks:443/"
        );
        assert_eq!(
            parse_server("https://testnet.zec.rocks")
                .unwrap()
                .to_string(),
            "https://testnet.zec.rocks/",
            "https keeps its default port"
        );
        assert!(parse_server("http://[bad").is_err());
    }

    /// Asks the default testnet server who it is. Needs the network, so ignored by default.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn probe_default_testnet_server() {
        use zcash_client_backend::proto::service::Empty;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let uri = parse_server(Network::Testnet.default_server()).unwrap();
        let mut client = zingo_netutils::get_client(uri).await.expect("connect");
        let info = client
            .get_lightd_info(Empty {})
            .await
            .expect("info")
            .into_inner();
        eprintln!("{info:#?}");
    }

    /// Syncs a fresh testnet wallet from near the tip and prints how it ends. Network, ignored.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn probe_testnet_sync() {
        use zcash_client_backend::proto::service::Empty;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let config = BackendConfig {
            network: Network::Testnet,
            data_dir: Some(dir.path().to_path_buf()),
            wallet_name: "probe.dat".into(),
            server: None,
        };
        let uri = config.server_uri(Network::Testnet, None).unwrap();
        let mut client = zingo_netutils::get_client(uri.clone())
            .await
            .expect("connect");
        let tip = client
            .get_lightd_info(Empty {})
            .await
            .unwrap()
            .into_inner()
            .block_height;
        let zc = config.client_config(Network::Testnet, uri);
        let mut lc = LightClient::new(
            zc,
            BlockHeight::from_u32(tip as u32),
            true,
            zingolib::wallet::SeedDerivation::Zip32Standard,
        )
        .unwrap();
        match lc.sync_and_await().await {
            Ok(r) => eprintln!("SYNC OK: {r}"),
            Err(e) => eprintln!("SYNC ERR: {e}\nDEBUG: {e:?}"),
        }
    }

    /// Opens a copy of the wallet file named by `DELEGATOR_PROBE_WALLET` and syncs it for up to
    /// two minutes, printing how it ends. Network, ignored.
    #[tokio::test]
    #[ignore = "needs the network and a wallet file"]
    async fn probe_wallet_sync() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let source = std::env::var("DELEGATOR_PROBE_WALLET").expect("DELEGATOR_PROBE_WALLET");
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(&source, dir.path().join("probe.dat")).unwrap();
        let config = BackendConfig {
            network: Network::Testnet,
            data_dir: Some(dir.path().to_path_buf()),
            wallet_name: "probe.dat".into(),
            server: std::env::var("DELEGATOR_PROBE_SERVER").ok(),
        };
        let uri = config.server_uri(Network::Testnet, None).unwrap();
        let zc = config.client_config(Network::Testnet, uri);
        let mut lc = LightClient::create_from_wallet_path(zc).unwrap();
        eprintln!("birthday {}", lc.wallet.read().await.birthday);
        let result = tokio::time::timeout(Duration::from_secs(120), lc.sync_and_await()).await;
        match result {
            Ok(Ok(r)) => eprintln!("SYNC OK: {r}"),
            Ok(Err(e)) => eprintln!("SYNC ERR: {e}\nDEBUG: {e:?}"),
            Err(_) => eprintln!("SYNC STILL RUNNING after 120s"),
        }
    }

    #[test]
    fn the_wizard_server_is_remembered_for_later_launches() {
        let dir = tempfile::tempdir().unwrap();
        let config = BackendConfig {
            network: Network::Regtest,
            data_dir: Some(dir.path().to_path_buf()),
            wallet_name: "w.dat".into(),
            server: None,
        };
        assert_eq!(
            config
                .server_uri(Network::Regtest, None)
                .unwrap()
                .to_string(),
            "http://127.0.0.1:9067/"
        );
        config
            .save_server(Network::Regtest, "my.node:1234")
            .unwrap();
        assert_eq!(
            config
                .server_uri(Network::Regtest, None)
                .unwrap()
                .to_string(),
            "http://my.node:1234/"
        );
        // a flag still wins
        let flagged = BackendConfig {
            server: Some("https://other:443".into()),
            ..config
        };
        assert_eq!(
            flagged
                .server_uri(Network::Regtest, None)
                .unwrap()
                .to_string(),
            "https://other:443/"
        );
    }

    #[test]
    fn regtest_and_testnet_chains_resolve() {
        use zcash_protocol::consensus::{NetworkUpgrade, Parameters};
        assert!(matches!(
            chain_type(Network::Regtest),
            ChainType::Regtest(_)
        ));
        assert_eq!(chain_type(Network::Mainnet), ChainType::Mainnet);
        for network in Network::ALL {
            assert_eq!(
                chain_type(network)
                    .activation_height(NetworkUpgrade::Sapling)
                    .map(u32::from),
                Some(network.sapling_activation()),
                "{network:?}: the wizard's floor matches the chain"
            );
        }
    }
}
