//! Inputs to the reducer and the effects it requests.

use std::time::Instant;

use super::state::{
    ChainRange, KeyMaterial, Network, SyncMode, Tip, WalletData, WalletEntry, WalletInfo,
};

/// A normalised key press. The terminal layer maps raw events onto this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    CtrlC,
    Other,
}

/// A sync engine observation, already stripped of library types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncNote {
    SessionStarted {
        tip: u32,
        birthday: u32,
        /// Blocks between the birthday and the tip: the measure of progress.
        total_blocks: u64,
        scanned_blocks: u64,
        total_outputs: u64,
        scanned_outputs: u64,
    },
    ScanPlanUpdated {
        ranges: Vec<ChainRange>,
    },
    TxDiscovered {
        txid: String,
        confirmed: bool,
    },
    TipMoved {
        to: u32,
    },
    /// The wallet's own count of what has been scanned so far. Progress is measured in blocks:
    /// on the crosslink chains most blocks carry no shielded output, so an output count stands
    /// still for most of a scan. The totals are estimates from the percentages the sync engine
    /// reports, and zero until those exist; a zero leaves the last known total in place.
    Progress {
        total_blocks: u64,
        scanned_blocks: u64,
        total_outputs: u64,
        scanned_outputs: u64,
    },
    Finished {
        blocks_scanned: u64,
    },
    Failed(String),
    Mode(SyncMode),
    Auto(bool),
    Scheduled {
        at: Instant,
    },
    /// A rescan started. Progress counters restart from zero.
    Reset,
}

/// Everything that can change the state.
#[derive(Clone, Debug)]
pub enum Action {
    Key(Key),
    Paste(String),
    Resize(u16, u16),
    Tick(Instant),
    /// The wallet files in the data directory.
    WalletsListed(Vec<WalletEntry>),
    /// The chain tip a server reported, or why it could not.
    TipFetched(Result<Tip, (Network, String)>),
    /// A fresh 24-word phrase for a new wallet.
    SeedGenerated(String),
    WalletOpened(WalletInfo),
    OpenFailed(String),
    DataLoaded(WalletData),
    Sync(SyncNote, Instant),
    Notice(String),
    Copied(bool),
    /// The faucet answered: the zatoshis it sent, or why it did not.
    FaucetDone(Result<u64, String>),
    /// The wallet worked out a send: `(fee, total)` in zatoshis, or why it cannot.
    SendProposed(Result<(u64, u64), String>),
    /// The send was transmitted: its txids, or why it failed.
    SendDone(Result<Vec<String>, String>),
    /// The backend finished shutting down.
    Shutdown,
}

/// How the backend should open or create the wallet. `name` is the file name without `.dat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenSpec {
    Read {
        network: Network,
        name: String,
        /// The chain is known from the meta or server file; otherwise the backend checks the
        /// wallet's blocks against the indexers first.
        verified: bool,
    },
    Create {
        network: Network,
        name: String,
        material: KeyMaterial,
        birthday: u32,
        server: Option<String>,
    },
}

/// Side effects requested by the reducer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Read the data directory and report its wallet files.
    ListWallets,
    /// Ask this network's server for its tip.
    FetchTip(Network),
    GenerateSeed,
    Open(OpenSpec),
    Refresh,
    NewUnifiedAddress {
        orchard: bool,
        sapling: bool,
    },
    NewTransparentAddress,
    PauseSync,
    ResumeSync,
    Rescan,
    Copy(String),
    /// Ask the chain's faucet to pay this address.
    Faucet {
        address: String,
    },
    /// Work out a send and hold it for confirmation. `amount` is in zatoshis.
    ProposeSend {
        to: String,
        amount: u64,
        memo: Option<String>,
    },
    /// Build and transmit the held send.
    ConfirmSend,
    /// Drop the held send.
    CancelSend,
    /// Persist the theme with this id as the user's preference.
    SaveTheme(&'static str),
    Quit,
    ForceQuit,
}
