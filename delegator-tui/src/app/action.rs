//! Inputs to the reducer and the effects it requests.

use std::time::Instant;

use super::state::{ChainRange, Network, SyncMode, WalletData, WalletInfo};

/// A normalised key press. The terminal layer maps raw events onto this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
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
        total_outputs: u64,
        scanned_outputs: u64,
        scanned_blocks: u64,
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
    /// The wallet's own count of what has been scanned so far. `total_outputs` is an estimate
    /// from the percentage the sync engine reports, and zero until that percentage exists.
    Progress {
        total_outputs: u64,
        scanned_outputs: u64,
        scanned_blocks: u64,
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
    WalletOpened(WalletInfo),
    OpenFailed(String),
    DataLoaded(WalletData),
    Sync(SyncNote, Instant),
    Notice(String),
    Copied(bool),
    /// The backend finished shutting down.
    Shutdown,
}

/// How the backend should open or create the wallet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenSpec {
    Read,
    Create {
        network: Network,
        server: String,
        ufvk: String,
        birthday: u32,
    },
}

/// Side effects requested by the reducer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
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
    /// Persist the theme with this id as the user's preference.
    SaveTheme(&'static str),
    Quit,
    ForceQuit,
}
