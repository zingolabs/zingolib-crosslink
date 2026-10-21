//! The wallet task. Owns the `LightClient`, runs the sync lifecycle, and translates wallet
//! state into [`Action`]s for the reducer. The UI never touches the client directly.
//!
//! pepper-sync on this branch publishes no event stream, so the task reads the wallet's sync
//! status on every tick and reports what changed since the last one.

pub mod fake;

use std::collections::HashSet;
use std::io::Read as _;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::str::FromStr as _;
use std::str::FromStr as _;
use std::str::FromStr as _;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pepper_sync::keys::transparent::TransparentScope;
use pepper_sync::sync::{ScanPriority, ScanRange};
use pepper_sync::wallet::{
    IronwoodNote, OrchardNote, OutputInterface, SaplingNote, TransparentCoin,
};
use tokio::sync::mpsc;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;
use zingolib::config::{ChainType, IndexerList, IndexerNetwork, ZingoConfig, chain_from_str};
use zingolib::data::PollReport;
use zingolib::lightclient::LightClient;
use zingolib::wallet::error::BalanceError;
use zingolib::wallet::keys::unified::ReceiverSelection;
use zingolib::wallet::output::SpendStatus;
use zingolib::wallet::summary::data::{
    SelfSendWalletEvent, SendType, SentWalletEvent, TransactionKind, TransactionSummary,
    WalletEvent, WalletEventKind,
};
use zingolib::wallet::{LightWallet, SeedDerivation, WalletBase, WalletSettings};

use crate::app::RosterRow;
use crate::app::{
    Action, AddressKind, AddressRow, Balance, BondRow, BondStatus, ChainRange, Command, Evidence,
    InputRow, KeyMaterial, Network, OpenSpec, OutputRole, OutputRow, Pool, PoolBalance,
    RangePriority, SpendState, SyncMode, SyncNote, Tip, TxKind, TxRow, TxStatus, WalletData,
    WalletEntry, WalletEventRow, WalletInfo, WalletKind,
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
    pub data_dir: Option<PathBuf>,
    pub server: Option<String>,
    pub seed_derivation: SeedDerivation,
}

/// The wallet file for `name`.
pub fn wallet_file(name: &str) -> String {
    format!("{name}.dat")
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

    pub fn wallet_path(&self, network: Network, name: &str) -> PathBuf {
        self.wallet_dir(network).join(wallet_file(name))
    }

    /// The wallet file itself does not record its server, so it is kept in a file beside it.
    fn server_path(&self, network: Network, name: &str) -> PathBuf {
        self.wallet_dir(network).join(format!("{name}.server"))
    }

    /// What the list screen shows without opening the wallet: network, kind, birthday.
    fn meta_path(&self, network: Network, name: &str) -> PathBuf {
        self.wallet_dir(network).join(format!("{name}.meta"))
    }

    fn saved_server(&self, network: Network, name: &str) -> Option<String> {
        std::fs::read_to_string(self.server_path(network, name))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn save_server(&self, network: Network, name: &str, server: &str) -> std::io::Result<()> {
        std::fs::write(self.server_path(network, name), format!("{server}\n"))
    }

    fn save_meta(&self, info: &WalletInfo) -> std::io::Result<()> {
        std::fs::write(
            self.meta_path(info.network, &info.wallet_name),
            format!(
                "network = {}\nkind = {}\nbirthday = {}\n",
                info.network.name(),
                info.kind.name(),
                info.birthday
            ),
        )
    }

    /// The server to use, in order: the one given for this call, the command line flag, the
    /// one saved with the wallet, and the network default.
    fn server_uri(
        &self,
        network: Network,
        name: &str,
        override_server: Option<&str>,
    ) -> Result<http::Uri, String> {
        let raw = override_server
            .map(str::to_string)
            .or_else(|| self.server.clone())
            .or_else(|| self.saved_server(network, name))
            .unwrap_or_else(|| network.default_server());
        parse_server(&raw)
    }

    fn client_config(&self, network: Network, name: &str, server: http::Uri) -> ZingoConfig {
        let mut config = ZingoConfig::build(chain_type(network))
            .set_indexer_network(network.indexer_network())
            .set_lightwalletd_uri(server)
            .set_wallet_dir(self.wallet_dir(network))
            .set_wallet_settings(wallet_settings())
            .create();
        config.wallet_name = PathBuf::from(wallet_file(name));
        config
    }

    /// Every crosslink wallet file under the data directory, newest first. Files for other
    /// chains are left out: see [`classify`].
    pub fn list_wallets(&self) -> Vec<WalletEntry> {
        let mut seen_dirs = Vec::new();
        let mut out = Vec::new();
        for network in Network::ALL {
            let dir = self.wallet_dir(network);
            if seen_dirs.contains(&dir) {
                continue;
            }
            seen_dirs.push(dir.clone());
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .and_then(|f| f.strip_suffix(".dat"))
                else {
                    continue;
                };
                if !path.is_file() {
                    continue;
                }
                let age_secs = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .map(|d| d.as_secs());
                match classify(
                    &path,
                    &self.meta_path(network, name),
                    &self.server_path(network, name),
                ) {
                    Ok(Some((found_network, kind, birthday, evidence))) => out.push(WalletEntry {
                        name: name.to_string(),
                        network: found_network,
                        kind,
                        birthday,
                        age_secs,
                        evidence,
                    }),
                    Ok(None) => {
                        tracing::info!("{} is not a crosslink wallet; not listed", path.display())
                    }
                    Err(e) => {
                        tracing::warn!("{} is not readable as a wallet: {e}", path.display())
                    }
                }
            }
        }
        out.sort_by_key(|w| (w.age_secs.unwrap_or(u64::MAX), w.name.clone()));
        out
    }
}

/// The chain name a wallet file was written for: `main`, `test`, or `regtest`. Files older
/// than format 32 keep the name further in and are not read; none of them are crosslink
/// wallets anyway.
pub fn file_chain(path: &Path) -> std::io::Result<Option<String>> {
    let mut file = std::fs::File::open(path)?;
    let mut version = [0u8; 8];
    file.read_exact(&mut version)?;
    if u64::from_le_bytes(version) < 32 {
        return Ok(None);
    }
    // a little-endian u64 length and the UTF-8 name, as zingolib's `write_string` does it
    let mut len = [0u8; 8];
    file.read_exact(&mut len)?;
    let len = u64::from_le_bytes(len);
    if len > 32 {
        return Ok(None);
    }
    let mut name = vec![0u8; len as usize];
    file.read_exact(&mut name)?;
    Ok(String::from_utf8(name).ok())
}

/// Whether `path` is a crosslink wallet, and which chain it is on.
///
/// The file's own chain name rules out mainnet and regtest. Both crosslink chains write the
/// testnet name, as does the public Zcash testnet, so the chain comes from the meta file beside
/// the wallet, else from the indexer saved beside it when that is a known crosslink node (a
/// public-testnet indexer rules the file out). With neither, the file is listed as unverified
/// and checked against the indexer's blocks when it opens.
#[allow(clippy::type_complexity)]
pub fn classify(
    path: &Path,
    meta: &Path,
    server: &Path,
) -> std::io::Result<Option<(Network, WalletKind, Option<u32>, Evidence)>> {
    if file_chain(path)?.as_deref() != Some("test") {
        return Ok(None);
    }
    let mut entry = WalletEntry {
        name: String::new(),
        network: Network::Stable,
        kind: WalletKind::Unknown,
        birthday: None,
        age_secs: None,
        evidence: Evidence::Unverified,
    };
    if read_meta(meta, &mut entry) {
        return Ok(Some((
            entry.network,
            entry.kind,
            entry.birthday,
            Evidence::Meta,
        )));
    }
    if let Ok(saved) = std::fs::read_to_string(server)
        && let Ok(uri) = parse_server(saved.trim())
    {
        match network_of_indexer(&uri) {
            Some(Ok(network)) => {
                return Ok(Some((
                    network,
                    entry.kind,
                    entry.birthday,
                    Evidence::Server,
                )));
            }
            Some(Err(())) => return Ok(None),
            None => {}
        }
    }
    Ok(Some((
        entry.network,
        entry.kind,
        entry.birthday,
        Evidence::Unverified,
    )))
}

/// Which crosslink chain a built-in indexer serves, `Err` for a known indexer of another
/// network, `None` for an indexer the library does not list.
fn network_of_indexer(uri: &http::Uri) -> Option<Result<Network, ()>> {
    let same = |a: &http::Uri| a.host() == uri.host() && a.port_u16() == uri.port_u16();
    if let Some(network) = Network::ALL.into_iter().find(|network| {
        network
            .servers()
            .iter()
            .any(|s| parse_server(s).is_ok_and(|a| same(&a)))
    }) {
        return Some(Ok(network));
    }
    IndexerList::builtin()
        .iter()
        .find(|(_, indexer)| same(&indexer.uri))
        .map(|(network, _)| match network {
            IndexerNetwork::CrosslinkFeaturenet => Ok(Network::Stable),
            n if n == IndexerNetwork::CROSSLINK_NIGHTLY => Ok(Network::Nightly),
            _ => Err(()),
        })
}

/// Fills `entry` from its meta file, when there is one. Returns whether the file named the
/// chain.
fn read_meta(path: &Path, entry: &mut WalletEntry) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut named = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (key.trim(), value.trim()) {
            ("network", v) => {
                if let Some(n) = Network::parse(v) {
                    entry.network = n;
                    named = true;
                }
            }
            ("kind", v) => {
                if let Some(k) = WalletKind::parse(v) {
                    entry.kind = k;
                }
            }
            ("birthday", v) => entry.birthday = v.parse().ok(),
            _ => {}
        }
    }
    named
}

/// Asks `server` for its chain tip, and measures the block spacing from the times of the tip
/// and a block a thousand back.
async fn fetch_tip(network: Network, server: http::Uri) -> Result<Tip, String> {
    use zcash_client_backend::proto::service::{BlockId, Empty};
    let mut client = zingo_netutils::get_client(server.clone())
        .await
        .map_err(|e| format!("cannot connect to {server}: {e}"))?;
    let info = client
        .get_lightd_info(Empty {})
        .await
        .map_err(|e| format!("{server} did not answer: {}", e.message()))?
        .into_inner();
    let height = u32::try_from(info.block_height).map_err(|_| "absurd tip height")?;
    let at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut spacing_secs = network.block_spacing_secs();
    let back = height.saturating_sub(1000).max(1);
    if back < height {
        let block = |h: u32| BlockId {
            height: u64::from(h),
            hash: Vec::new(),
        };
        if let (Ok(newest), Ok(oldest)) = (
            client.get_block(block(height)).await,
            client.get_block(block(back)).await,
        ) {
            let (newest, oldest) = (newest.into_inner(), oldest.into_inner());
            let seconds = u64::from(newest.time.saturating_sub(oldest.time));
            let blocks = u64::from(height - back);
            if seconds > 0 {
                spacing_secs = (seconds / blocks).clamp(1, 600);
            }
        }
    }
    // a node that serves no Ironwood tree state at the tip forces the birthday down to the
    // activation height; a node that cannot be asked is treated the same, the safe side
    let ironwood_tree =
        match zingolib::grpc_connector::get_trees(server.clone(), u64::from(height)).await {
            Ok(trees) => !trees.ironwood_tree.is_empty(),
            Err(e) => {
                tracing::warn!("cannot read the tree state at {height} from {server}: {e}");
                false
            }
        };
    Ok(Tip {
        network,
        height,
        at_unix,
        spacing_secs,
        ironwood_tree,
    })
}

/// The chain a wallet file's blocks are on: asks each crosslink chain's indexer for the block
/// at the wallet's newest synced height and compares hashes.
async fn chain_of_blocks(
    config: &BackendConfig,
    name: &str,
    newest: (u32, [u8; 32]),
) -> Result<Network, String> {
    use zcash_client_backend::proto::service::BlockId;
    let (height, hash) = newest;
    let mut failures = Vec::new();
    for network in Network::ALL {
        let Ok(uri) = config.server_uri(network, name, None) else {
            continue;
        };
        let Ok(mut client) = zingo_netutils::get_client(uri.clone()).await else {
            failures.push(format!("{} did not answer", network.name()));
            continue;
        };
        match client
            .get_block(BlockId {
                height: u64::from(height),
                hash: Vec::new(),
            })
            .await
        {
            Ok(block) => {
                if block.into_inner().hash == hash {
                    return Ok(network);
                }
            }
            Err(e) => failures.push(format!("{}: {}", network.name(), e.message())),
        }
    }
    Err(if failures.len() == Network::ALL.len() {
        format!("no crosslink indexer answered ({})", failures.join("; "))
    } else {
        format!(
            "block {height} of this wallet is on neither crosslink chain; the file is from another network"
        )
    })
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
    // both crosslink chains share the featurenet schedule
    let _ = network;
    chain_from_str("testnet").expect("a known chain name")
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
    /// Blocks from the birthday to the tip; from the engine's percentage when it has one,
    /// else from the heights themselves. Zero until either is known.
    total_blocks: u64,
    scanned_blocks: u64,
    /// Estimated from the percentage the engine reports; zero until it reports one.
    total_outputs: u64,
    scanned_outputs: u64,
    /// Every transaction the wallet knows, with whether it is confirmed.
    txs: Vec<(String, bool)>,
}

struct Backend {
    config: BackendConfig,
    actions: mpsc::UnboundedSender<Action>,
    lc: Option<LightClient>,
    network: Network,
    /// The open wallet's name, without `.dat`.
    wallet_name: String,
    auto: bool,
    next_launch: Option<Instant>,
    dirty: bool,
    last_mode: SyncMode,
    /// A session was launched and its start has not been reported yet.
    session_pending: bool,
    known_txids: HashSet<String>,
    last: SyncSnapshot,
    /// The bonds from the last time the server answered, shown again while it is silent.
    last_bonds: Vec<BondRow>,
    /// The roster from the last time the server answered.
    last_roster: Vec<RosterRow>,
}

impl Backend {
    fn new(config: BackendConfig, actions: mpsc::UnboundedSender<Action>) -> Self {
        Self {
            network: Network::Stable,
            wallet_name: String::new(),
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
            last_bonds: Vec::new(),
            last_roster: Vec::new(),
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
            Command::ListWallets => {
                let config = self.config.clone();
                let wallets = tokio::task::spawn_blocking(move || config.list_wallets())
                    .await
                    .unwrap_or_default();
                self.send(Action::WalletsListed(wallets));
            }
            Command::FetchTip(network) => {
                let actions = self.actions.clone();
                let server = self.config.server_uri(network, "", None);
                tokio::spawn(async move {
                    let result = match server {
                        Ok(uri) => fetch_tip(network, uri).await,
                        Err(e) => Err(e),
                    };
                    let _ = actions.send(Action::TipFetched(result.map_err(|e| (network, e))));
                });
            }
            Command::GenerateSeed => {
                let phrase =
                    bip0039::Mnemonic::<bip0039::English>::generate(bip0039::Count::Words24)
                        .into_phrase();
                self.send(Action::SeedGenerated(phrase));
            }
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
            Command::Faucet { address } => {
                let Some(lc) = self.lc.as_ref() else {
                    return false;
                };
                let uri = lc.indexer_uri();
                let actions = self.actions.clone();
                tokio::spawn(async move {
                    let _ = actions.send(Action::FaucetDone(request_faucet(uri, address).await));
                });
            }
            Command::ProposeSend { to, amount, memo } => {
                let result = match self.lc.as_mut() {
                    Some(lc) => propose_send(lc, &to, amount, memo.as_deref()).await,
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendProposed(result));
            }
            Command::ConfirmSend => {
                let result = match self.lc.as_mut() {
                    Some(lc) => lc
                        .send_stored_proposal(true)
                        .await
                        .map(|txids| txids.iter().map(|id| id.to_string()).collect())
                        .map_err(|e| e.to_string()),
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendDone(result));
                self.refresh().await;
            }
            Command::CancelSend => {
                if let Some(lc) = self.lc.as_mut() {
                    lc.wallet.write().await.clear_proposal();
                    let _ignore = lc.resume_sync();
                }
            }
            Command::Faucet { address } => {
                let Some(lc) = self.lc.as_ref() else {
                    return false;
                };
                let uri = lc.indexer_uri();
                let actions = self.actions.clone();
                tokio::spawn(async move {
                    let _ = actions.send(Action::FaucetDone(request_faucet(uri, address).await));
                });
            }
            Command::ProposeSend { to, amount, memo } => {
                let result = match self.lc.as_mut() {
                    Some(lc) => propose_send(lc, &to, amount, memo.as_deref()).await,
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendProposed(result));
            }
            Command::ConfirmSend => {
                let result = match self.lc.as_mut() {
                    Some(lc) => lc
                        .send_stored_proposal(true)
                        .await
                        .map(|txids| txids.iter().map(|id| id.to_string()).collect())
                        .map_err(|e| e.to_string()),
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendDone(result));
                self.refresh().await;
            }
            Command::CancelSend => {
                if let Some(lc) = self.lc.as_mut() {
                    lc.wallet.write().await.clear_proposal();
                    let _ignore = lc.resume_sync();
                }
            }
            Command::Faucet { address } => {
                let Some(lc) = self.lc.as_ref() else {
                    return false;
                };
                let uri = lc.indexer_uri();
                let actions = self.actions.clone();
                tokio::spawn(async move {
                    let _ = actions.send(Action::FaucetDone(request_faucet(uri, address).await));
                });
            }
            Command::ProposeSend { to, amount, memo } => {
                let result = match self.lc.as_mut() {
                    Some(lc) => propose_send(lc, &to, amount, memo.as_deref()).await,
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendProposed(result));
            }
            Command::ConfirmSend => {
                let result = match self.lc.as_mut() {
                    Some(lc) => lc
                        .send_stored_proposal(true)
                        .await
                        .map(|txids| txids.iter().map(|id| id.to_string()).collect())
                        .map_err(|e| e.to_string()),
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendDone(result));
                self.refresh().await;
            }
            Command::CancelSend => {
                if let Some(lc) = self.lc.as_mut() {
                    lc.wallet.write().await.clear_proposal();
                    let _ignore = lc.resume_sync();
                }
            }
            Command::Faucet { address } => {
                let Some(lc) = self.lc.as_ref() else {
                    return false;
                };
                let uri = lc.indexer_uri();
                let actions = self.actions.clone();
                tokio::spawn(async move {
                    let _ = actions.send(Action::FaucetDone(request_faucet(uri, address).await));
                });
            }
            Command::ProposeSend { to, amount, memo } => {
                let result = match self.lc.as_mut() {
                    Some(lc) => propose_send(lc, &to, amount, memo.as_deref()).await,
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendProposed(result));
            }
            Command::ConfirmSend => {
                let result = match self.lc.as_mut() {
                    Some(lc) => lc
                        .send_stored_proposal(true)
                        .await
                        .map(|txids| txids.iter().map(|id| id.to_string()).collect())
                        .map_err(|e| e.to_string()),
                    None => Err("no wallet is open".into()),
                };
                self.send(Action::SendDone(result));
                self.refresh().await;
            }
            Command::CancelSend => {
                if let Some(lc) = self.lc.as_mut() {
                    lc.wallet.write().await.clear_proposal();
                    let _ignore = lc.resume_sync();
                }
            }
            Command::Copy(_) | Command::SaveTheme(_) | Command::ForceQuit => {}
        }
        false
    }

    async fn open(&mut self, spec: OpenSpec) -> Result<(LightClient, WalletInfo), String> {
        let (lc, network, name) = match spec {
            OpenSpec::Read {
                network,
                name,
                verified,
            } => {
                let network = if verified {
                    network
                } else {
                    self.verify_chain(network, &name).await?
                };
                let server = self.config.server_uri(network, &name, None)?;
                let config = self.config.client_config(network, &name, server);
                let lc = LightClient::create_from_wallet_path(config)
                    .map_err(|e| format!("cannot open wallet: {e}"))?;
                (lc, network, name)
            }
            OpenSpec::Create {
                network,
                name,
                material,
                birthday,
                server,
            } => {
                let path = self.config.wallet_path(network, &name);
                if path.exists() {
                    return Err(format!(
                        "a wallet file already exists at {}",
                        path.display()
                    ));
                }
                let server_uri = self.config.server_uri(network, &name, server.as_deref())?;
                // the same rule the create screen applies, for the command line's sake
                let birthday = match fetch_tip(network, server_uri.clone()).await {
                    Ok(tip) => {
                        let settled = tip.settle_birthday(birthday);
                        if settled != birthday {
                            self.send(Action::Notice(format!(
                                "{} keeps no Ironwood tree state · the wallet scans from block {settled}",
                                server_uri
                            )));
                        }
                        settled
                    }
                    Err(e) => return Err(e),
                };
                let config = self.config.client_config(network, &name, server_uri);
                if let Some(server) = &server
                    && let Err(e) = self.config.save_server(network, &name, server)
                {
                    tracing::warn!("cannot save the server beside the wallet: {e}");
                }
                let base = match material {
                    KeyMaterial::Ufvk(ufvk) => WalletBase::Ufvk(ufvk),
                    KeyMaterial::Seed(phrase) => WalletBase::Mnemonic {
                        mnemonic: bip0039::Mnemonic::from_phrase(&phrase)
                            .map_err(|e| format!("not a valid seed phrase: {e}"))?,
                        no_of_accounts: NonZeroU32::new(1).expect("non-zero"),
                        seed_derivation: self.config.seed_derivation,
                    },
                };
                let wallet = LightWallet::new(
                    config.chain,
                    base,
                    BlockHeight::from_u32(birthday),
                    config.wallet_settings.clone(),
                )
                .map_err(|e| format!("cannot create wallet: {e}"))?;
                let lc = LightClient::create_from_wallet(wallet, config, false)
                    .map_err(|e| format!("cannot create wallet: {e}"))?;
                (lc, network, name)
            }
        };
        let (has_orchard, kind, birthday, known) = {
            let wallet = lc.wallet.read().await;
            let Some(keys) = wallet.unified_key_store.get(&zip32::AccountId::ZERO) else {
                return Err("the wallet file has no keys".into());
            };
            let kind = if keys.is_spending_key() {
                WalletKind::Seed
            } else {
                WalletKind::Watch
            };
            (
                keys.default_receivers().is_some_and(|r| r.orchard),
                kind,
                u32::from(wallet.birthday),
                wallet
                    .wallet_transactions
                    .keys()
                    .map(ToString::to_string)
                    .collect::<HashSet<_>>(),
            )
        };
        self.network = network;
        self.wallet_name = name.clone();
        self.known_txids = known;
        self.last = SyncSnapshot::default();
        // the block spacing, for turning the calendar's block counts into time
        let spacing_secs = match fetch_tip(network, lc.indexer_uri()).await {
            Ok(tip) => tip.spacing_secs,
            Err(_) => network.block_spacing_secs(),
        };
        let info = WalletInfo {
            network,
            wallet_name: name,
            wallet_path: lc.config.get_wallet_pathbuf().display().to_string(),
            birthday,
            server: lc.indexer_uri().to_string(),
            has_orchard,
            kind,
            spacing_secs,
        };
        if let Err(e) = self.config.save_meta(&info) {
            tracing::warn!("cannot write the wallet's meta file: {e}");
        }
        Ok((lc, info))
    }

    /// Which crosslink chain an unverified wallet file is on, from its newest synced block.
    /// A file with no blocks yet keeps `fallback`.
    async fn verify_chain(&self, fallback: Network, name: &str) -> Result<Network, String> {
        let path = self.config.wallet_path(fallback, name);
        let newest = tokio::task::spawn_blocking(move || -> Result<_, String> {
            let file = std::fs::File::open(&path)
                .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
            let wallet = LightWallet::read(std::io::BufReader::new(file), chain_type(fallback))
                .map_err(|e| format!("cannot read wallet: {e}"))?;
            Ok(wallet
                .wallet_blocks
                .iter()
                .next_back()
                .map(|(height, block)| (u32::from(*height), block.block_hash().0)))
        })
        .await
        .map_err(|e| format!("wallet read task failed: {e}"))??;
        match newest {
            None => Ok(fallback),
            Some(newest) => chain_of_blocks(&self.config, name, newest).await,
        }
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
                tracing::info!("sync session finished: {result:?}");
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
                total_blocks: now.total_blocks,
                scanned_blocks: now.scanned_blocks,
                total_outputs: now.total_outputs,
                scanned_outputs: now.scanned_outputs,
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
        let progress = |s: &SyncSnapshot| {
            (
                s.total_blocks,
                s.scanned_blocks,
                s.total_outputs,
                s.scanned_outputs,
            )
        };
        if now.started && progress(&now) != progress(&self.last) {
            self.sync_note(SyncNote::Progress {
                total_blocks: now.total_blocks,
                scanned_blocks: now.scanned_blocks,
                total_outputs: now.total_outputs,
                scanned_outputs: now.scanned_outputs,
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
        match load(lc, &self.last_bonds, &self.last_roster).await {
            Ok(data) => {
                self.last_bonds = data.bonds.clone();
                self.last_roster = data.roster.clone();
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
        let scanned_outputs = u64::from(status.total_sapling_outputs_scanned)
            + u64::from(status.total_orchard_outputs_scanned)
            + u64::from(status.total_ironwood_outputs_scanned);
        let scanned_blocks = u64::from(status.total_blocks_scanned);
        // the span from the birthday to the tip is the measure of progress; the engine
        // reports no percentage here, so the heights themselves are the total
        let total_blocks = match (birthday, tip) {
            (Some(b), Some(t)) if t >= b => u64::from(t - b) + 1,
            _ => 0,
        };
        out.started = u32::from(status.sync_start_height) != 0;
        out.ranges = status.scan_ranges.iter().map(chain_range).collect();
        out.total_blocks = total_blocks.max(scanned_blocks);
        out.scanned_blocks = scanned_blocks;
        out.total_outputs = 0;
        out.scanned_outputs = scanned_outputs;
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

/// Asks the faucet to pay `address`. The answer is what it sent, in zatoshis.
async fn request_faucet(uri: http::Uri, address: String) -> Result<u64, String> {
    use zcash_client_backend::proto::service::FaucetRequest;

    let mut client = zingo_netutils::get_client(uri)
        .await
        .map_err(|e| format!("cannot reach the server: {e:?}"))?;
    let answer = client
        .request_faucet_donation(FaucetRequest { address })
        .await
        .map_err(|e| e.message().to_string())?;
    Ok(answer.into_inner().amount)
}

/// Works out a send of `amount` zatoshis to `to` and stores it in the wallet for
/// [`LightClient::send_stored_proposal`]. Sync pauses while a proposal is held. The answer is
/// the fee and the total that leaves the wallet.
async fn propose_send(
    lc: &mut LightClient,
    to: &str,
    amount: u64,
    memo: Option<&str>,
) -> Result<(u64, u64), String> {
    use zcash_protocol::memo::Memo;
    use zingolib::data::receivers::{Receiver, transaction_request_from_receivers};
    use zingolib::utils::conversion::address_from_str;

    let recipient_address =
        address_from_str(to).map_err(|_| "that is not a zcash address".to_string())?;
    let amount_zats = Zatoshis::from_u64(amount).map_err(|_| "the amount is out of range")?;
    let memo = match memo {
        Some(text) => Some(
            Memo::from_str(text)
                .map_err(|e| format!("the memo cannot be used: {e}"))?
                .encode(),
        ),
        None => None,
    };
    let request = transaction_request_from_receivers(vec![Receiver {
        recipient_address,
        amount: amount_zats,
        memo,
    }])
    .map_err(|e| e.to_string())?;
    let proposal = lc
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .map_err(|e| e.to_string())?;
    let fee: u64 = proposal
        .steps()
        .iter()
        .map(|step| u64::from(step.balance().fee_required()))
        .sum();
    Ok((fee, amount.saturating_add(fee)))
}

/// The wallet's delegation bonds: every bond it created and has not withdrawn, with the
/// server's word on where each stands. The wallet lock is released before the server is
/// asked. When the server cannot be reached a bond keeps what `last` said about it, or its
/// creation terms if it is new.
async fn bonds(lc: &LightClient, last: &[BondRow]) -> (Vec<BondRow>, HashSet<[u8; 32]>) {
    use zcash_client_backend::proto::service::BondInfoRequest;
    use zcash_primitives::bft::PubKeyID;
    use zcash_primitives::transaction::{StakingAction, StakingActionKind};

    let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    // the finalizer as its address, `zfinv1…`; a reward conversion carries the bare key only
    let finalizer_of = |action: &StakingAction| {
        action
            .target_finalizer_address()
            .map(|address| address.encode())
            .unwrap_or_else(|| PubKeyID(action.target_finalizer_pk()).to_string())
    };
    /// A bond the wallet minted and has not withdrawn: its key, its finalizer as shown, the
    /// finalizer's public key, the amount it was created with, the height of its last action.
    type Known = ([u8; 32], String, [u8; 32], u64, u32);
    // in creation order
    let known: Vec<Known> = {
        let wallet = lc.wallet.read().await;
        let mut actions: Vec<_> = wallet
            .wallet_transactions
            .values()
            .filter_map(|tx| {
                tx.staking_data()
                    .map(|action| (u32::from(tx.status().get_height()), action))
            })
            .collect();
        actions.sort_by_key(|(height, _)| *height);
        let withdrawn: HashSet<[u8; 32]> = actions
            .iter()
            .filter(|(_, a)| a.kind() == StakingActionKind::WithdrawDelegationBond)
            .map(|(_, a)| a.bond_key())
            .collect();
        let mut known: Vec<Known> = Vec::new();
        for (height, action) in &actions {
            let key = action.bond_key();
            if withdrawn.contains(&key) {
                continue;
            }
            // `bond_terms` is `Some` only for the kinds that mint a bond
            if let Some((_, amount, _)) = action.bond_terms() {
                known.push((
                    key,
                    finalizer_of(action),
                    action.target_finalizer_pk(),
                    amount,
                    *height,
                ));
            } else if let Some(bond) = known.iter_mut().find(|(k, ..)| *k == key) {
                if action.kind() == StakingActionKind::RetargetDelegationBond {
                    bond.1 = finalizer_of(action);
                    bond.2 = action.target_finalizer_pk();
                }
                bond.4 = *height;
            }
        }
        known
    };
    if known.is_empty() {
        return (Vec::new(), HashSet::new());
    }
    let mut client = match zingo_netutils::get_client(lc.indexer_uri()).await {
        Ok(client) => Some(client),
        Err(e) => {
            tracing::warn!("cannot ask the server about bonds: {e:?}");
            None
        }
    };
    let mut rows = Vec::new();
    let mut finalizers = HashSet::new();
    for (key, finalizer, finalizer_key, created_with, height) in known {
        let key_hex = hex(key);
        let remembered = last.iter().find(|b| b.key == key_hex);
        let info = match client.as_mut() {
            Some(client) => client
                .get_bond_info(BondInfoRequest {
                    bond_key: key.to_vec(),
                })
                .await
                .map(|info| info.into_inner())
                .map_err(|e| tracing::warn!("cannot read a bond: {e}"))
                .ok(),
            None => None,
        };
        let row = match info {
            // 2 means withdrawn on chain, whatever the wallet has seen so far
            Some(info) if info.status == 2 => continue,
            Some(info) => BondRow {
                key: key_hex,
                finalizer: finalizer.clone(),
                amount: info.amount,
                status: if info.status == 1 {
                    BondStatus::Unbonding
                } else {
                    BondStatus::Bonded
                },
                since: info.last_action_height.max(height),
            },
            None => match remembered {
                Some(bond) => bond.clone(),
                None => BondRow {
                    key: key_hex,
                    finalizer: finalizer.clone(),
                    amount: created_with,
                    status: BondStatus::Bonded,
                    since: height,
                },
            },
        };
        finalizers.insert(finalizer_key);
        rows.push(row);
    }
    (rows, finalizers)
}

/// The chain's finalizer roster as the node reports it, largest stake first, each marked when
/// one of this wallet's bonds points at it. `last` stands in when the node is silent.
async fn roster(lc: &LightClient, ours: &HashSet<[u8; 32]>, last: &[RosterRow]) -> Vec<RosterRow> {
    use zcash_primitives::bft::PubKeyID;

    let members = match lc.get_roster_info().await {
        Ok(roster) => roster.members,
        Err(e) => {
            tracing::warn!("cannot read the roster: {e}");
            return last.to_vec();
        }
    };
    let mut rows: Vec<RosterRow> = members
        .into_iter()
        .map(|m| RosterRow {
            finalizer: m
                .finalizer_address
                .map(|a| a.encode())
                .unwrap_or_else(|| PubKeyID(m.pub_key).to_string()),
            stake: m.voting_power,
            ours: ours.contains(&m.pub_key),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.stake
            .cmp(&a.stake)
            .then_with(|| a.finalizer.cmp(&b.finalizer))
    });
    rows
}

/// Reads everything the screens show. `last_bonds` and `last_roster` stand in if the server is
/// silent.
async fn load(
    lc: &LightClient,
    last_bonds: &[BondRow],
    last_roster: &[RosterRow],
) -> Result<WalletData, String> {
    let account = zip32::AccountId::ZERO;
    let balance = {
        let wallet = lc.wallet.read().await;
        Balance {
            ironwood: pool_balance::<IronwoodNote>(&wallet, account),
            orchard: pool_balance::<OrchardNote>(&wallet, account),
            sapling: pool_balance::<SaplingNote>(&wallet, account),
            transparent: pool_balance::<TransparentCoin>(&wallet, account),
            staked: 0,
            unbonding: 0,
        }
    };
    let (bonds, ours) = bonds(lc, last_bonds).await;
    let roster = roster(lc, &ours, last_roster).await;
    let sum = |status: BondStatus| -> u64 {
        bonds
            .iter()
            .filter(|b| b.status == status)
            .map(|b| b.amount)
            .fold(0, u64::saturating_add)
    };
    let balance = Balance {
        staked: sum(BondStatus::Bonded),
        unbonding: sum(BondStatus::Unbonding),
        ..balance
    };

    let summaries = lc
        .transaction_summaries(false)
        .await
        .map_err(|e| e.to_string())?;
    let transfers = lc.wallet_events(false).await.map_err(|e| e.to_string())?;
    let mut by_txid: std::collections::HashMap<String, Vec<WalletEventRow>> =
        std::collections::HashMap::new();
    for transfer in transfers.iter() {
        by_txid
            .entry(transfer.txid.to_string())
            .or_default()
            .push(wallet_event_row(transfer));
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
        bonds,
        roster,
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
    use pepper_sync::wallet::{IronwoodNote, OrchardNote, SaplingNote};

    let wallet = lc.wallet.read().await;
    let mut index = OutputIndex::default();

    let pools = [
        (Pool::Ironwood, wallet.note_summaries::<IronwoodNote>(true)),
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
    transfers: &mut std::collections::HashMap<String, Vec<WalletEventRow>>,
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
        (Pool::Ironwood, &s.ironwood_notes),
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
        (Pool::Ironwood, &s.outgoing_ironwood_notes),
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
        wallet_events: transfers.remove(&txid).unwrap_or_default(),
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

fn wallet_event_row(transfer: &WalletEvent) -> WalletEventRow {
    let kind = match transfer.kind {
        WalletEventKind::Received => "received",
        WalletEventKind::Sent(SentWalletEvent::Send) => "sent",
        WalletEventKind::Sent(SentWalletEvent::SendToSelf(to_self)) => match to_self {
            SelfSendWalletEvent::Basic => "sent to self",
            SelfSendWalletEvent::Shield => "shield",
            SelfSendWalletEvent::MemoToSelf => "memo to self",
            SelfSendWalletEvent::Refund => "refund",
            SelfSendWalletEvent::Stake => "bond created",
            SelfSendWalletEvent::BeginUnbond => "unbond begun",
            SelfSendWalletEvent::WithdrawBond => "bond withdrawn",
            SelfSendWalletEvent::RetargetDelegationBond => "bond retargeted",
        },
    };
    WalletEventRow {
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
        let uri = parse_server(&Network::Stable.default_server()).unwrap();
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
            data_dir: Some(dir.path().to_path_buf()),
            server: None,
            seed_derivation: SeedDerivation::Zip32Standard,
        };
        let uri = config.server_uri(Network::Stable, "probe", None).unwrap();
        let mut client = zingo_netutils::get_client(uri.clone())
            .await
            .expect("connect");
        let tip = client
            .get_lightd_info(Empty {})
            .await
            .unwrap()
            .into_inner()
            .block_height;
        let zc = config.client_config(Network::Stable, "probe", uri);
        let mut lc = LightClient::new(
            zc,
            BlockHeight::from_u32(tip as u32),
            true,
            zingolib::wallet::SeedDerivation::Zip32Standard,
        )
        .unwrap();
        match lc.sync_and_await().await {
            Ok(r) => eprintln!("SYNC OK: {r:?}"),
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
            data_dir: Some(dir.path().to_path_buf()),
            server: std::env::var("DELEGATOR_PROBE_SERVER").ok(),
            seed_derivation: SeedDerivation::Zip32Standard,
        };
        let uri = config.server_uri(Network::Stable, "probe", None).unwrap();
        let zc = config.client_config(Network::Stable, "probe", uri);
        let mut lc = LightClient::create_from_wallet_path(zc).unwrap();
        eprintln!("birthday {}", lc.wallet.read().await.birthday);
        let result = tokio::time::timeout(Duration::from_secs(120), lc.sync_and_await()).await;
        match result {
            Ok(Ok(r)) => eprintln!("SYNC OK: {r:?}"),
            Ok(Err(e)) => eprintln!("SYNC ERR: {e}\nDEBUG: {e:?}"),
            Err(_) => eprintln!("SYNC STILL RUNNING after 120s"),
        }
    }

    /// Opens a copy of the wallet named by `DELEGATOR_PROBE_WALLET` against the stable node,
    /// prints what it holds, asks the faucet to pay its address when `DELEGATOR_PROBE_FAUCET`
    /// is set, and watches the mempool and the tip for a minute. Network, ignored.
    #[tokio::test]
    #[ignore = "needs the network and a wallet file"]
    async fn probe_faucet() {
        use zcash_client_backend::proto::service::{
            BlockId, ChainSpec, Empty, GetMempoolTxRequest,
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let source = std::env::var("DELEGATOR_PROBE_WALLET").expect("DELEGATOR_PROBE_WALLET");
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(&source, dir.path().join("probe.dat")).unwrap();
        let config = BackendConfig {
            data_dir: Some(dir.path().to_path_buf()),
            server: std::env::var("DELEGATOR_PROBE_SERVER").ok(),
            seed_derivation: SeedDerivation::Zip32Standard,
        };
        let uri = config.server_uri(Network::Stable, "probe", None).unwrap();
        eprintln!("server {uri}");
        let mut client = zingo_netutils::get_client(uri.clone())
            .await
            .expect("connect");
        let info = client.get_lightd_info(Empty {}).await.unwrap().into_inner();
        eprintln!(
            "node: chain {} height {} consensus {} version {}",
            info.chain_name, info.block_height, info.consensus_branch_id, info.version
        );
        // the mempool as the node streams it: how many transactions, and what each carries
        async fn mempool(uri: http::Uri) -> usize {
            let Ok(mut client) = zingo_netutils::get_client(uri).await else {
                eprintln!("  mempool: cannot connect");
                return 0;
            };
            let mut n = 0usize;
            match client
                .get_mempool_tx(GetMempoolTxRequest {
                    exclude_txid_suffixes: Vec::new(),
                    pool_types: Vec::new(),
                })
                .await
            {
                Ok(stream) => {
                    let mut stream = stream.into_inner();
                    loop {
                        match stream.message().await {
                            Ok(Some(tx)) => {
                                n += 1;
                                eprintln!(
                                    "  mempool tx {} actions {} ironwood {}",
                                    hex_of(&tx.txid),
                                    tx.actions.len(),
                                    tx.ironwood_actions.len()
                                );
                            }
                            Ok(None) => break,
                            Err(e) => {
                                eprintln!("  mempool stream error: {e}");
                                break;
                            }
                        }
                    }
                }
                Err(e) => eprintln!("  mempool request failed: {e}"),
            }
            n
        }
        eprintln!("mempool: {} transactions", mempool(uri.clone()).await);
        let tip = client
            .get_block(BlockId {
                height: info.block_height,
                hash: Vec::new(),
            })
            .await
            .unwrap()
            .into_inner();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        eprintln!(
            "tip block {} time {} ({} s ago), {} txs",
            tip.height,
            tip.time,
            now.saturating_sub(u64::from(tip.time)),
            tip.vtx.len()
        );
        for host in Network::Stable.servers() {
            let Ok(other) = parse_server(&host) else {
                continue;
            };
            match zingo_netutils::get_client(other).await {
                Ok(mut c) => match c.get_lightd_info(Empty {}).await {
                    Ok(i) => eprintln!("{host}: height {}", i.into_inner().block_height),
                    Err(e) => eprintln!("{host}: {}", e.message()),
                },
                Err(e) => eprintln!("{host}: cannot connect: {e:?}"),
            }
        }
        let tip = client
            .get_block(BlockId {
                height: info.block_height,
                hash: Vec::new(),
            })
            .await
            .unwrap()
            .into_inner();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        eprintln!(
            "tip block {} time {} ({} s ago), {} txs",
            tip.height,
            tip.time,
            now.saturating_sub(u64::from(tip.time)),
            tip.vtx.len()
        );
        for host in Network::Stable.servers() {
            let Ok(other) = parse_server(&host) else {
                continue;
            };
            match zingo_netutils::get_client(other).await {
                Ok(mut c) => match c.get_lightd_info(Empty {}).await {
                    Ok(i) => eprintln!("{host}: height {}", i.into_inner().block_height),
                    Err(e) => eprintln!("{host}: {}", e.message()),
                },
                Err(e) => eprintln!("{host}: cannot connect: {e:?}"),
            }
        }

        let zc = config.client_config(Network::Stable, "probe", uri.clone());
        let mut lc = LightClient::create_from_wallet_path(zc).unwrap();
        let (address, synced) = {
            let wallet = lc.wallet.read().await;
            eprintln!("wallet birthday {}", wallet.birthday);
            eprintln!(
                "synced to {:?}, tip known {:?}",
                wallet.sync_state.wallet_birthday(),
                wallet.sync_state.last_known_chain_height()
            );
            let mut address = None;
            for (id, ua) in wallet.unified_addresses() {
                let enc = ua.encode(&wallet.network);
                eprintln!(
                    "address #{} orchard={} sapling={} transparent={} {}",
                    id.address_index,
                    ua.has_orchard(),
                    ua.has_sapling(),
                    ua.has_transparent(),
                    enc
                );
                if ua.has_orchard() {
                    address = Some(enc);
                }
            }
            eprintln!("transactions known: {}", wallet.wallet_transactions.len());
            (address, wallet.wallet_blocks.len())
        };
        eprintln!("wallet blocks held: {synced}");
        let data = load(&lc, &[], &[]).await.unwrap();
        eprintln!(
            "balance: ironwood {} orchard {} sapling {} transparent {} (pending {})",
            data.balance.ironwood.total(),
            data.balance.orchard.total(),
            data.balance.sapling.total(),
            data.balance.transparent.total(),
            data.balance.pending()
        );
        for tx in &data.txs {
            eprintln!(
                "  tx {} {:?} {} {:?}",
                tx.txid, tx.kind, tx.value, tx.status
            );
        }

        if std::env::var("DELEGATOR_PROBE_FAUCET").is_ok() {
            let address = address.expect("an orchard address");
            eprintln!("asking the faucet for {address}");
            match request_faucet(uri.clone(), address).await {
                Ok(amount) => eprintln!("faucet answered: {amount} zats"),
                Err(e) => eprintln!("faucet error: {e}"),
            }
            let start = client
                .get_latest_block(ChainSpec {})
                .await
                .unwrap()
                .into_inner()
                .height;
            for i in 0..12 {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let tip = client
                    .get_latest_block(ChainSpec {})
                    .await
                    .unwrap()
                    .into_inner()
                    .height;
                eprintln!("t+{}s tip {tip}", (i + 1) * 5);
                let n = mempool(uri.clone()).await;
                eprintln!("  mempool: {n}");
                for h in (start + 1)..=tip {
                    if let Ok(block) = client
                        .get_block(BlockId {
                            height: h,
                            hash: Vec::new(),
                        })
                        .await
                    {
                        let block = block.into_inner();
                        eprintln!(
                            "  block {h}: {} txs, ironwood actions {}",
                            block.vtx.len(),
                            block
                                .vtx
                                .iter()
                                .map(|t| t.ironwood_actions.len())
                                .sum::<usize>()
                        );
                    }
                }
            }
            match tokio::time::timeout(Duration::from_secs(90), lc.sync_and_await()).await {
                Ok(Ok(r)) => eprintln!("SYNC OK: {r:?}"),
                Ok(Err(e)) => eprintln!("SYNC ERR: {e}"),
                Err(_) => eprintln!("SYNC STILL RUNNING"),
            }
            let data = load(&lc, &[], &[]).await.unwrap();
            eprintln!(
                "after: ironwood {} orchard {} pending {} txs {}",
                data.balance.ironwood.total(),
                data.balance.orchard.total(),
                data.balance.pending(),
                data.txs.len()
            );
        }
    }

    /// Asks every known crosslink node for its tip and how old that block is. Network, ignored.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn probe_every_node() {
        use zcash_client_backend::proto::service::{BlockId, Empty};
        let _ = rustls::crypto::ring::default_provider().install_default();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut hosts: Vec<(String, String)> = Vec::new();
        for network in Network::ALL {
            for host in network.servers() {
                hosts.push((network.name().to_string(), host));
            }
        }
        for (network, indexer) in IndexerList::builtin().iter() {
            let uri = indexer.uri.to_string().trim_end_matches('/').to_string();
            if !hosts.iter().any(|(_, h)| *h == uri) {
                hosts.push((format!("builtin {network}"), uri));
            }
        }
        for (label, host) in hosts {
            let Ok(uri) = parse_server(&host) else {
                continue;
            };
            let mut client =
                match tokio::time::timeout(Duration::from_secs(8), zingo_netutils::get_client(uri))
                    .await
                {
                    Ok(Ok(c)) => c,
                    Ok(Err(e)) => {
                        eprintln!("{label:<22} {host:<34} cannot connect: {e:?}");
                        continue;
                    }
                    Err(_) => {
                        eprintln!("{label:<22} {host:<34} connect timed out");
                        continue;
                    }
                };
            let info = match tokio::time::timeout(
                Duration::from_secs(8),
                client.get_lightd_info(Empty {}),
            )
            .await
            {
                Ok(Ok(i)) => i.into_inner(),
                Ok(Err(e)) => {
                    eprintln!("{label:<22} {host:<34} info failed: {}", e.message());
                    continue;
                }
                Err(_) => {
                    eprintln!("{label:<22} {host:<34} info timed out");
                    continue;
                }
            };
            let age = match client
                .get_block(BlockId {
                    height: info.block_height,
                    hash: Vec::new(),
                })
                .await
            {
                Ok(b) => format!(
                    "{} s ago",
                    now.saturating_sub(u64::from(b.into_inner().time))
                ),
                Err(e) => format!("block failed: {}", e.message()),
            };
            eprintln!(
                "{label:<22} {host:<34} chain {:<8} height {:<8} tip {age}  branch {}",
                info.chain_name, info.block_height, info.consensus_branch_id
            );
        }
    }

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn the_chosen_server_is_remembered_for_later_launches() {
        let dir = tempfile::tempdir().unwrap();
        let config = BackendConfig {
            data_dir: Some(dir.path().to_path_buf()),
            server: None,
            seed_derivation: SeedDerivation::Zip32Standard,
        };
        let builtin = format!("{}/", Network::Stable.default_server());
        assert_eq!(
            config
                .server_uri(Network::Stable, "w", None)
                .unwrap()
                .to_string(),
            builtin
        );
        config
            .save_server(Network::Stable, "w", "my.node:1234")
            .unwrap();
        assert_eq!(
            config
                .server_uri(Network::Stable, "w", None)
                .unwrap()
                .to_string(),
            "http://my.node:1234/"
        );
        // another wallet in the same directory has its own
        assert_eq!(
            config
                .server_uri(Network::Stable, "other", None)
                .unwrap()
                .to_string(),
            builtin
        );
        // the nightly chain has its own first indexer
        assert_ne!(
            Network::Nightly.default_server(),
            Network::Stable.default_server()
        );
        // a flag still wins
        let flagged = BackendConfig {
            server: Some("https://other:443".into()),
            ..config
        };
        assert_eq!(
            flagged
                .server_uri(Network::Stable, "w", None)
                .unwrap()
                .to_string(),
            "https://other:443/"
        );
    }

    /// A real wallet file for `chain`, written the way zingolib writes it.
    fn wallet_file(dir: &Path, name: &str, chain: ChainType) {
        let mut wallet = LightWallet::new(
            chain,
            WalletBase::FreshEntropy {
                no_of_accounts: NonZeroU32::new(1).unwrap(),
                seed_derivation: SeedDerivation::Zip32Standard,
            },
            BlockHeight::from_u32(500_000),
            wallet_settings(),
        )
        .unwrap();
        let mut bytes = Vec::new();
        wallet.write(&mut bytes, &chain).unwrap();
        std::fs::write(dir.join(super::wallet_file(name)), bytes).unwrap();
    }

    #[test]
    fn the_list_keeps_crosslink_wallets_and_hides_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let config = BackendConfig {
            data_dir: Some(dir.path().to_path_buf()),
            server: None,
            seed_derivation: SeedDerivation::Zip32Standard,
        };
        assert!(config.list_wallets().is_empty());
        let testnet = chain_type(Network::Stable);
        wallet_file(dir.path(), "main", testnet);
        wallet_file(dir.path(), "old", testnet);
        wallet_file(dir.path(), "by-server", testnet);
        wallet_file(dir.path(), "public", testnet);
        wallet_file(dir.path(), "zcash", ChainType::Mainnet);
        wallet_file(dir.path(), "local", chain_from_str("regtest").unwrap());
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("junk.dat"), b"not a wallet").unwrap();
        std::fs::write(
            dir.path().join("main.meta"),
            "network = nightly\nkind = seed\nbirthday = 118000\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("by-server.server"),
            format!("{}\n", Network::Nightly.default_server()),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("public.server"),
            "https://testnet.zec.rocks:443\n",
        )
        .unwrap();
        let list = config.list_wallets();
        let names: Vec<&str> = list.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names.len(),
            3,
            "mainnet, regtest, the public testnet and junk stay out: {names:?}"
        );
        let main = list.iter().find(|w| w.name == "main").unwrap();
        assert_eq!(main.network, Network::Nightly);
        assert_eq!(main.kind, WalletKind::Seed);
        assert_eq!(main.birthday, Some(118_000));
        assert_eq!(main.evidence, Evidence::Meta);
        assert!(main.age_secs.is_some());
        let by_server = list.iter().find(|w| w.name == "by-server").unwrap();
        assert_eq!(by_server.network, Network::Nightly);
        assert_eq!(by_server.evidence, Evidence::Server);
        let old = list.iter().find(|w| w.name == "old").unwrap();
        assert_eq!(old.kind, WalletKind::Unknown, "no meta file yet");
        assert_eq!(old.evidence, Evidence::Unverified);
        assert_eq!(
            old.network,
            Network::Stable,
            "until the blocks say otherwise"
        );
        // the meta file is what a saved WalletInfo writes
        config
            .save_meta(&WalletInfo {
                network: Network::Stable,
                wallet_name: "old".into(),
                wallet_path: String::new(),
                birthday: 2_604_500,
                server: String::new(),
                has_orchard: true,
                spacing_secs: 15,
                kind: WalletKind::Watch,
            })
            .unwrap();
        let old = config
            .list_wallets()
            .into_iter()
            .find(|w| w.name == "old")
            .unwrap();
        assert_eq!(
            (old.kind, old.birthday, old.evidence),
            (WalletKind::Watch, Some(2_604_500), Evidence::Meta)
        );
    }

    #[test]
    fn the_file_header_names_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        wallet_file(dir.path(), "t", chain_type(Network::Stable));
        wallet_file(dir.path(), "m", ChainType::Mainnet);
        assert_eq!(
            file_chain(&dir.path().join("t.dat")).unwrap().as_deref(),
            Some("test")
        );
        assert_eq!(
            file_chain(&dir.path().join("m.dat")).unwrap().as_deref(),
            Some("main")
        );
        std::fs::write(dir.path().join("short.dat"), b"abc").unwrap();
        assert!(file_chain(&dir.path().join("short.dat")).is_err());
    }
}
