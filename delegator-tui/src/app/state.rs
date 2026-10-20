//! Application state. Plain data only, so every screen can be tested without a terminal.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a footer notice stays visible.
pub const NOTICE_TTL: Duration = Duration::from_secs(4);
/// Sync events kept for the Sync screen log.
pub const EVENT_LOG_CAP: usize = 100;
/// Progress samples kept for the ETA rate window.
const PROGRESS_WINDOW: usize = 12;

/// Zcash network the wallet watches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    Mainnet,
    Testnet,
    Regtest,
}

impl Network {
    pub const ALL: [Network; 3] = [Network::Mainnet, Network::Testnet, Network::Regtest];

    pub fn name(self) -> &'static str {
        match self {
            Network::Mainnet => "mainnet",
            Network::Testnet => "testnet",
            Network::Regtest => "regtest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mainnet" => Some(Network::Mainnet),
            "testnet" => Some(Network::Testnet),
            "regtest" => Some(Network::Regtest),
            _ => None,
        }
    }

    /// Default indexer for the network.
    pub fn default_server(self) -> &'static str {
        match self {
            Network::Mainnet => zingolib::config::DEFAULT_INDEXER_URI,
            Network::Testnet => zingolib::config::DEFAULT_TESTNET_INDEXER_URI,
            Network::Regtest => "http://127.0.0.1:9067",
        }
    }

    /// The block Sapling activated at, the lowest birthday a wallet can have. The testnet here
    /// is the crosslink testnet, live from block 1.
    pub fn sapling_activation(self) -> u32 {
        match self {
            Network::Mainnet => 419_200,
            Network::Testnet | Network::Regtest => 1,
        }
    }

    /// Bech32m human-readable prefix of a unified full viewing key on this network.
    pub fn ufvk_prefix(self) -> &'static str {
        match self {
            Network::Mainnet => "uview1",
            Network::Testnet => "uviewtest1",
            Network::Regtest => "uviewregtest1",
        }
    }

    /// The network a unified full viewing key is encoded for, read from its prefix.
    pub fn from_ufvk(ufvk: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|n| ufvk.starts_with(n.ufvk_prefix()))
    }
}

/// Top-level screens, each reachable by a single key from anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Transactions,
    Receive,
    Addresses,
    Sync,
}

impl Screen {
    /// Tab order. Each screen's key is the first letter of its title.
    pub const ALL: [Screen; 5] = [
        Screen::Home,
        Screen::Transactions,
        Screen::Receive,
        Screen::Addresses,
        Screen::Sync,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Home => "Home",
            Screen::Transactions => "Transactions",
            Screen::Receive => "Receive",
            Screen::Addresses => "Addresses",
            Screen::Sync => "Sync",
        }
    }
}

/// What the transaction detail view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailMode {
    InputsOutputs,
    ValueTransfers,
}

/// A view layered over the current screen. Esc removes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    TxDetail {
        mode: DetailMode,
    },
    RescanConfirm,
    NewUnifiedAddress {
        orchard_only: bool,
    },
    AddressQr,
    /// The theme list. The highlighted theme is live in `State::theme`; `original` is what Esc
    /// restores.
    ThemePicker {
        original: usize,
    },
}

/// Which address the Receive screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveKind {
    Unified,
    Transparent,
}

/// Seconds between blocks, for turning a date into a height. Mainnet is fixed by consensus;
/// the crosslink featurenets run faster, so their figure is an estimate.
impl Network {
    pub fn block_spacing_secs(self) -> u64 {
        match self {
            Network::Mainnet => 75,
            Network::Testnet | Network::Regtest => 15,
        }
    }
}

/// What kind of key material a wallet file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalletKind {
    /// A seed phrase: the wallet can spend and stake.
    Seed,
    /// A viewing key only.
    Watch,
    /// Not recorded yet; the file was made by an older version and has not been opened since.
    Unknown,
}

impl WalletKind {
    pub fn name(self) -> &'static str {
        match self {
            WalletKind::Seed => "seed",
            WalletKind::Watch => "watch",
            WalletKind::Unknown => "?",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "seed" => Some(WalletKind::Seed),
            "watch" => Some(WalletKind::Watch),
            _ => None,
        }
    }
}

/// One wallet file in the data directory, as the opening list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletEntry {
    /// The file name without its `.dat` suffix.
    pub name: String,
    pub network: Network,
    pub kind: WalletKind,
    pub birthday: Option<u32>,
    /// Seconds since the file was last written, when the file system says.
    pub age_secs: Option<u64>,
}

/// The choices on the setup menu, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Create,
    Restore,
    Watch,
    Quit,
}

impl MenuItem {
    pub const ALL: [MenuItem; 4] = [
        MenuItem::Create,
        MenuItem::Restore,
        MenuItem::Watch,
        MenuItem::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MenuItem::Create => "create a new wallet",
            MenuItem::Restore => "restore from a seed phrase",
            MenuItem::Watch => "watch with a viewing key",
            MenuItem::Quit => "quit",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            MenuItem::Create => "24-word seed · spend and stake",
            MenuItem::Restore => "spend and stake",
            MenuItem::Watch => "balances and bonds · cannot spend",
            MenuItem::Quit => "",
        }
    }
}

/// How the user answers the birthday question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BirthdayMode {
    /// A number pasted from the app, which shows it under the viewing key.
    FromApp,
    Height,
    Date,
    /// Scan from Sapling activation.
    Unknown,
}

impl BirthdayMode {
    pub const ALL: [BirthdayMode; 4] = [
        BirthdayMode::FromApp,
        BirthdayMode::Height,
        BirthdayMode::Date,
        BirthdayMode::Unknown,
    ];
}

/// The screens of the first-run flow, in the order they are reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The wallets already in the data directory.
    Wallets,
    /// Create, restore, or watch.
    Menu,
    Create,
    Restore,
    Watch,
    Birthday,
    Name,
}

/// What a new wallet is built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyMaterial {
    /// A BIP-39 phrase.
    Seed(String),
    /// A unified full viewing key.
    Ufvk(String),
}

/// The chain tip as the server last reported it, with the wall-clock time of the report so a
/// date can be turned into a height without asking the clock again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tip {
    pub network: Network,
    pub height: u32,
    pub at_unix: u64,
}

/// The first-run flow: list, menu, one branch, birthday, name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Onboarding {
    pub step: Step,
    pub wallets: Vec<WalletEntry>,
    /// The list has been read at least once. Until then the list screen says so.
    pub listed: bool,
    pub wallet_sel: usize,
    pub menu_sel: usize,
    /// Network for a seed wallet. A viewing key names its own.
    pub network: Network,
    pub tip: Option<Tip>,
    pub tip_error: Option<String>,
    /// A tip request is out for this network.
    pub tip_pending: Option<Network>,
    /// The phrase generated for a new wallet.
    pub seed: Option<String>,
    /// The seed shown on one line instead of the numbered grid.
    pub seed_line: bool,
    /// Words of the phrase being restored, and the one being typed.
    pub words: Vec<String>,
    pub draft: String,
    pub key: String,
    pub birthday_mode: Option<BirthdayMode>,
    pub birthday_sel: usize,
    pub birthday_input: String,
    /// A number that came with the paste, read as the birthday.
    pub birthday_hint: Option<u32>,
    pub birthday: Option<u32>,
    /// The accepted key material and the network it is for.
    pub material: Option<(Network, KeyMaterial)>,
    pub name: String,
    pub error: Option<String>,
    /// The server came from the command line.
    pub server: Option<String>,
}

impl Onboarding {
    pub fn new(network: Network, server: Option<String>) -> Self {
        Self {
            step: Step::Wallets,
            wallets: Vec::new(),
            listed: false,
            wallet_sel: 0,
            menu_sel: 0,
            network,
            tip: None,
            tip_error: None,
            tip_pending: None,
            seed: None,
            seed_line: false,
            words: Vec::new(),
            draft: String::new(),
            key: String::new(),
            birthday_mode: None,
            birthday_sel: 1,
            birthday_input: String::new(),
            birthday_hint: None,
            birthday: None,
            material: None,
            name: String::new(),
            error: None,
            server,
        }
    }

    pub fn menu_item(&self) -> MenuItem {
        MenuItem::ALL[self.menu_sel.min(MenuItem::ALL.len() - 1)]
    }

    /// The network the wallet being set up is for: the key's, once there is one.
    pub fn target_network(&self) -> Network {
        self.material.as_ref().map_or(self.network, |(n, _)| *n)
    }

    pub fn tip_for(&self, network: Network) -> Option<Tip> {
        self.tip.filter(|t| t.network == network)
    }

    /// Whether the material is a viewing key.
    pub fn is_watch(&self) -> bool {
        matches!(self.material, Some((_, KeyMaterial::Ufvk(_))))
    }
}

/// Lifecycle phase of the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Onboarding(Box<Onboarding>),
    Loading(String),
    Fatal(String),
    Main,
    ShuttingDown,
}

/// Facts about the opened wallet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletInfo {
    pub network: Network,
    pub wallet_name: String,
    pub wallet_path: String,
    pub birthday: u32,
    pub server: String,
    /// Whether the viewing key has an orchard component.
    pub has_orchard: bool,
    pub kind: WalletKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolBalance {
    pub confirmed: u64,
    pub pending: u64,
}

impl PoolBalance {
    pub fn total(self) -> u64 {
        self.confirmed + self.pending
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balance {
    pub orchard: PoolBalance,
    pub sapling: PoolBalance,
    pub transparent: PoolBalance,
    /// Value locked in delegation bonds. Not spendable, so it is not part of any pool.
    pub staked: u64,
}

impl Balance {
    /// Spendable value across the pools. Bonded funds are not included.
    pub fn total(&self) -> u64 {
        self.pools().iter().map(|(_, b)| b.total()).sum()
    }

    pub fn pending(&self) -> u64 {
        self.pools().iter().map(|(_, b)| b.pending).sum()
    }

    /// Every pool with its balance, in display order.
    pub fn pools(&self) -> [(Pool, PoolBalance); 3] {
        [
            (Pool::Orchard, self.orchard),
            (Pool::Sapling, self.sapling),
            (Pool::Transparent, self.transparent),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxStatus {
    Confirmed(u32),
    Pending,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxKind {
    Received,
    Sent,
    /// Sent to this wallet within the pools it spent from.
    SentToSelf,
    Shield,
    /// Funds locked into a delegation bond.
    CreateBond,
    /// Unbonding started; the funds come back once the unbonding period ends.
    BeginUnbond,
    /// Unbonded funds withdrawn back into the wallet.
    WithdrawBond,
    /// A bond pointed at a different finalizer.
    RetargetBond,
}

impl TxKind {
    pub fn long(self) -> &'static str {
        match self {
            TxKind::Received => "received",
            TxKind::Sent => "sent",
            TxKind::SentToSelf => "sent to self",
            TxKind::Shield => "shielded",
            TxKind::CreateBond => "bond created",
            TxKind::BeginUnbond => "unbond begun",
            TxKind::WithdrawBond => "bond withdrawn",
            TxKind::RetargetBond => "bond retargeted",
        }
    }

    /// The sign shown before the amount: money in, money out, or a move within the wallet.
    pub fn sign(self) -> &'static str {
        match self {
            TxKind::Received => "+",
            TxKind::Sent => "-",
            TxKind::SentToSelf
            | TxKind::Shield
            | TxKind::CreateBond
            | TxKind::BeginUnbond
            | TxKind::WithdrawBond
            | TxKind::RetargetBond => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pool {
    Orchard,
    Sapling,
    Transparent,
}

impl Pool {
    pub fn name(self) -> &'static str {
        match self {
            Pool::Orchard => "orchard",
            Pool::Sapling => "sapling",
            Pool::Transparent => "transparent",
        }
    }
}

/// What an output did in its transaction, from this wallet's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputRole {
    /// Arrived from someone else.
    Received,
    /// Paid to someone else.
    Sent,
    /// Sent to this wallet on purpose: to one of its addresses, or into a pool the transaction
    /// moved value to.
    ToSelf,
    /// The remainder, returned to this wallet.
    Change,
}

/// Whether a wallet output has been spent, and by which transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpendState {
    Unspent,
    /// Consumed by a confirmed transaction.
    Spent(String),
    /// A spend exists but is not yet confirmed.
    PendingSpent(String),
}

impl SpendState {
    pub fn describe(&self) -> String {
        match self {
            SpendState::Unspent => "unspent".into(),
            SpendState::Spent(txid) => {
                format!("spent by {}", crate::format::truncate_middle(txid, 18))
            }
            SpendState::PendingSpent(txid) => {
                format!(
                    "spend pending in {}",
                    crate::format::truncate_middle(txid, 18)
                )
            }
        }
    }
}

/// One output of a transaction as the wallet sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRow {
    pub pool: Pool,
    pub role: OutputRole,
    pub value: u64,
    /// Position of the output within the transaction.
    pub index: u32,
    pub address: Option<String>,
    pub memo: Option<String>,
    /// Meaningful for received outputs only.
    pub spend: SpendState,
    /// External for a receiving output, internal for change.
    pub scope: Option<String>,
}

/// One wallet output consumed by a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputRow {
    pub pool: Pool,
    pub value: u64,
    /// The transaction that created the output being spent.
    pub source_txid: String,
    /// Position of the output within its own transaction.
    pub output_index: u32,
    pub memo: Option<String>,
    /// The spend has not been confirmed yet.
    pub pending: bool,
}

/// One wallet event associated with a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueTransferRow {
    pub kind: String,
    pub value: u64,
    pub recipient: Option<String>,
    pub pool_received: Option<String>,
    pub memos: Vec<String>,
}

/// One row of the transaction list, with its detail data attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxRow {
    pub txid: String,
    pub datetime: u32,
    pub height: u32,
    pub status: TxStatus,
    pub kind: TxKind,
    pub value: u64,
    pub fee: Option<u64>,
    /// Wallet outputs this transaction consumed.
    pub inputs: Vec<InputRow>,
    pub outputs: Vec<OutputRow>,
    pub value_transfers: Vec<ValueTransferRow>,
}

impl TxRow {
    pub fn first_memo(&self) -> Option<&str> {
        self.outputs
            .iter()
            .filter_map(|o| o.memo.as_deref())
            .find(|m| !m.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressKind {
    Unified {
        orchard: bool,
        sapling: bool,
        transparent: bool,
    },
    Transparent {
        scope: String,
    },
}

impl AddressKind {
    pub fn is_unified(&self) -> bool {
        matches!(self, AddressKind::Unified { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            AddressKind::Unified {
                orchard,
                sapling,
                transparent,
            } => {
                let mut parts = Vec::new();
                if *orchard {
                    parts.push("orchard");
                }
                if *sapling {
                    parts.push("sapling");
                }
                if *transparent {
                    parts.push("transparent");
                }
                parts.join("+")
            }
            AddressKind::Transparent { scope } => scope.to_lowercase(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressRow {
    pub kind: AddressKind,
    pub index: u32,
    pub address: String,
    pub received: u64,
}

/// Everything the backend loads from the wallet on a refresh.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalletData {
    pub balance: Balance,
    pub txs: Vec<TxRow>,
    pub addresses: Vec<AddressRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMode {
    NotRunning,
    Running,
    Paused,
    Stopping,
}

/// The scheduler's reason for scanning a chain range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangePriority {
    RefetchingNullifiers,
    Scanning,
    Scanned,
    ScannedWithoutMapping,
    Historic,
    OpenAdjacent,
    FoundNote,
    ChainTip,
    Verify,
}

/// One contiguous range on the scheduler's chain plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainRange {
    pub start: u32,
    pub end: u32,
    pub priority: RangePriority,
}

/// How far one stretch of the chain has got. The Sync screen draws only these three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Scanned,
    /// The scheduler is working on it now.
    Live,
    NotYet,
}

/// The newest transaction the sync found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundTx {
    pub txid: String,
    pub confirmed: bool,
    pub at: Instant,
}

/// Sync progress as the Sync screen shows it.
#[derive(Clone, Debug)]
pub struct SyncView {
    pub mode: SyncMode,
    /// Whether a new session launches automatically after the previous one finishes.
    pub auto: bool,
    pub birthday: Option<u32>,
    pub tip: Option<u32>,
    pub chain_ranges: Vec<ChainRange>,
    pub outputs_total: u64,
    pub outputs_scanned: u64,
    pub blocks_scanned: u64,
    pub txs_found: u64,
    pub sessions_done: u64,
    pub progress_log: VecDeque<(Instant, u64)>,
    pub events: VecDeque<String>,
    pub last_found: Option<FoundTx>,
    pub next_launch: Option<Instant>,
    pub error: Option<String>,
}

impl SyncView {
    pub fn new() -> Self {
        Self {
            mode: SyncMode::NotRunning,
            auto: true,
            birthday: None,
            tip: None,
            chain_ranges: Vec::new(),
            outputs_total: 0,
            outputs_scanned: 0,
            blocks_scanned: 0,
            txs_found: 0,
            sessions_done: 0,
            progress_log: VecDeque::with_capacity(PROGRESS_WINDOW + 1),
            events: VecDeque::with_capacity(EVENT_LOG_CAP + 1),
            last_found: None,
            next_launch: None,
            error: None,
        }
    }

    pub fn fraction(&self) -> f64 {
        if self.outputs_total == 0 {
            if self.sessions_done > 0 { 1.0 } else { 0.0 }
        } else {
            (self.outputs_scanned as f64 / self.outputs_total as f64).clamp(0.0, 1.0)
        }
    }

    pub fn set_scan_plan(&mut self, ranges: Vec<ChainRange>) {
        self.chain_ranges = ranges;
    }

    pub fn clear_scan_plan(&mut self) {
        self.chain_ranges.clear();
    }

    /// The stage of blocks `start..end`, or `None` when no plan covers them. Live work anywhere
    /// in the stretch wins, so a range narrower than one screen cell still shows. Otherwise the
    /// stretch is scanned only when all of it is.
    pub fn stage(&self, start: u32, end: u32) -> Option<Stage> {
        let overlaps = |a: u32, b: u32| a < end && start < b;
        let mut covered = false;
        let mut scanned = true;
        for range in self
            .chain_ranges
            .iter()
            .filter(|r| overlaps(r.start, r.end))
        {
            covered = true;
            match range.priority {
                RangePriority::Scanning | RangePriority::RefetchingNullifiers => {
                    return Some(Stage::Live);
                }
                RangePriority::Scanned | RangePriority::ScannedWithoutMapping => {}
                _ => scanned = false,
            }
        }
        covered.then_some(if scanned {
            Stage::Scanned
        } else {
            Stage::NotYet
        })
    }

    /// Whether `p` pauses rather than resumes, or `None` while the sync is stopping.
    pub fn p_pauses(&self) -> Option<bool> {
        match self.mode {
            SyncMode::Running => Some(true),
            SyncMode::Paused => Some(false),
            SyncMode::NotRunning => Some(self.auto),
            SyncMode::Stopping => None,
        }
    }

    pub fn begin_session(&mut self) {
        self.clear_scan_plan();
    }

    /// Outputs per second observed over the recent window.
    fn rate(&self) -> Option<f64> {
        let (first_at, first) = self.progress_log.front()?;
        let (last_at, last) = self.progress_log.back()?;
        let span = last_at.duration_since(*first_at).as_secs_f64();
        (span > 0.5 && last > first).then(|| (last - first) as f64 / span)
    }

    pub fn eta_secs(&self) -> Option<u64> {
        let rate = self.rate()?;
        (rate > 1.0 && self.outputs_total > self.outputs_scanned)
            .then(|| ((self.outputs_total - self.outputs_scanned) as f64 / rate) as u64)
    }

    pub fn record_progress(&mut self, at: Instant) {
        self.progress_log.push_back((at, self.outputs_scanned));
        if self.progress_log.len() > PROGRESS_WINDOW {
            self.progress_log.pop_front();
        }
    }

    pub fn reset_progress(&mut self) {
        self.outputs_total = 0;
        self.outputs_scanned = 0;
        self.blocks_scanned = 0;
        self.birthday = None;
        self.tip = None;
        self.clear_scan_plan();
        self.progress_log.clear();
    }

    pub fn log(&mut self, line: String) {
        self.events.push_back(line);
        if self.events.len() > EVENT_LOG_CAP {
            self.events.pop_front();
        }
    }

    /// Short state label for the header. Only what the user acts on: whether the wallet is
    /// current, and if not, why.
    pub fn label(&self) -> String {
        match self.mode {
            SyncMode::Running => format!("syncing {:.0}%", self.fraction() * 100.0),
            SyncMode::Paused => "sync paused".into(),
            SyncMode::Stopping => "sync stopping".into(),
            SyncMode::NotRunning => {
                if self.error.is_some() {
                    "sync failed".into()
                } else if !self.auto {
                    "sync paused".into()
                } else if self.sessions_done > 0 {
                    "up to date".into()
                } else {
                    "starting sync".into()
                }
            }
        }
    }
}

impl Default for SyncView {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete application state.
#[derive(Clone, Debug)]
pub struct State {
    pub phase: Phase,
    pub screen: Screen,
    pub overlay: Overlay,
    pub size: (u16, u16),
    pub ascii_qr: bool,
    pub wallet: Option<WalletInfo>,
    pub balance: Balance,
    pub txs: Vec<TxRow>,
    pub addresses: Vec<AddressRow>,
    pub tx_selected: usize,
    pub addr_selected: usize,
    pub receive_kind: ReceiveKind,
    pub detail_scroll: u16,
    pub sync: SyncView,
    /// The Sync screen shows the chain as a map instead of a single strip.
    pub sync_map: bool,
    pub notice: Option<(String, Instant)>,
    pub now: Instant,
    /// Ticks since start. Animations step on this, so they keep a steady pace no matter how
    /// often other actions redraw the screen.
    pub ticks: u64,
    pub exit: bool,
    /// Set after the first quit request. A second one forces an immediate exit.
    pub quit_requested: bool,
    /// The flow that produced the pending open request, restored if it fails.
    pub onboarding_backup: Option<Onboarding>,
    /// The dither field behind idle panes. Off unless asked for on the command line.
    pub dither: crate::theme::Dither,
    /// Index into [`crate::theme::THEMES`].
    pub theme: usize,
    pub color_depth: crate::theme::ColorDepth,
}

impl State {
    pub fn new(phase: Phase, now: Instant) -> Self {
        Self {
            phase,
            screen: Screen::Home,
            overlay: Overlay::None,
            size: (80, 24),
            ascii_qr: false,
            wallet: None,
            balance: Balance::default(),
            txs: Vec::new(),
            addresses: Vec::new(),
            tx_selected: 0,
            addr_selected: 0,
            receive_kind: ReceiveKind::Unified,
            detail_scroll: 0,
            sync: SyncView::new(),
            sync_map: false,
            notice: None,
            now,
            ticks: 0,
            exit: false,
            quit_requested: false,
            onboarding_backup: None,
            dither: crate::theme::Dither::Off,
            theme: crate::theme::choose(None, None, false),
            color_depth: crate::theme::ColorDepth::TrueColor,
        }
    }

    /// Seconds since start, on the tick clock. Animations run on this.
    pub fn elapsed_secs(&self) -> f32 {
        self.ticks as f32 * crate::TICK_SECS
    }

    pub fn has_orchard(&self) -> bool {
        self.wallet.as_ref().is_some_and(|w| w.has_orchard)
    }

    pub fn selected_tx(&self) -> Option<&TxRow> {
        self.txs.get(self.tx_selected)
    }

    pub fn selected_address(&self) -> Option<&AddressRow> {
        self.addresses.get(self.addr_selected)
    }

    /// The address the Receive screen shows: the newest of the chosen kind.
    pub fn receive_address(&self) -> Option<&AddressRow> {
        match self.receive_kind {
            ReceiveKind::Unified => self
                .addresses
                .iter()
                .filter(|a| a.kind.is_unified())
                .max_by_key(|a| a.index),
            ReceiveKind::Transparent => self
                .addresses
                .iter()
                .filter(|a| matches!(&a.kind, AddressKind::Transparent { scope } if scope.eq_ignore_ascii_case("external")))
                .max_by_key(|a| a.index),
        }
    }

    pub fn notice_text(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at)| self.now.duration_since(*at) < NOTICE_TTL)
            .map(|(m, _)| m.as_str())
    }

    pub fn set_notice(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), self.now));
    }
}
