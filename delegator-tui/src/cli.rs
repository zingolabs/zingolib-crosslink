//! Command line arguments.

use std::path::PathBuf;

use clap::Parser;

/// Terminal wallet for Zcash crosslink delegators.
///
/// Starts on the list of crosslink wallets in the data directory, from which a wallet is opened
/// or a new one is created, restored from a seed phrase, or set up watch-only from a viewing
/// key. Pass --wallet to open one straight away, or --ufvk and --birthday to create a
/// watch-only wallet without the screens. --fake runs on a made-up wallet, for a look around.
#[derive(Parser, Debug, Clone)]
#[command(name = "delegator-tui", version, about)]
pub struct Cli {
    /// Crosslink chain: stable (the featurenet) or nightly. Picks the indexers, and the chain
    /// a new wallet is for. Defaults to stable.
    #[arg(long, short = 'n', value_parser = ["stable", "nightly"])]
    pub network: Option<String>,

    /// Indexer URL. Defaults to the first built-in crosslink node for --network.
    #[arg(long)]
    pub server: Option<String>,

    /// Directory that holds the wallet files. Defaults to the platform Zcash testnet data
    /// directory, which both crosslink chains share.
    #[arg(long, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// Open this wallet instead of showing the list. The file is NAME.dat in the data
    /// directory. With --ufvk, the name the new wallet gets.
    #[arg(long, short = 'w', value_name = "NAME")]
    pub wallet: Option<String>,

    /// Create a watch-only wallet from this unified full viewing key. Fails if the wallet file
    /// exists.
    #[arg(long, value_name = "UFVK", requires = "birthday")]
    pub ufvk: Option<String>,

    /// Block height the viewing key was created at. Required with --ufvk.
    #[arg(long, value_name = "HEIGHT")]
    pub birthday: Option<u32>,

    /// How spending keys are derived from a seed phrase: zip32, the standard, or crosslink, the
    /// truncated form the zebra-crosslink node uses.
    #[arg(long, default_value = "zip32", value_parser = ["zip32", "crosslink"])]
    pub seed_derivation: String,

    /// Run on a made-up wallet with made-up transactions, addresses and sync progress, and no
    /// network. Every screen can be seen without a real wallet. Nothing is written to disk.
    #[arg(long)]
    pub fake: bool,

    /// Draw QR codes with '#' characters instead of Unicode blocks.
    #[arg(long)]
    pub ascii_qr: bool,

    /// Log file. Defaults to delegator-tui.log in the data directory. Level comes from RUST_LOG.
    #[arg(long, value_name = "FILE")]
    pub log_file: Option<PathBuf>,

    /// Colour theme for this session, overriding the saved one. Press c inside to pick and save.
    #[arg(
        long,
        value_name = "THEME",
        value_parser = clap::builder::PossibleValuesParser::new(crate::theme::ids())
    )]
    pub theme: Option<String>,

    /// Colour depth. auto uses 24-bit colour when COLORTERM advertises it and the 256-colour
    /// palette otherwise.
    #[arg(long, default_value = "auto", value_parser = ["auto", "24bit", "256"])]
    pub color_depth: String,

    /// The drifting dither field in the empty part of the screen: blocks (shade characters),
    /// braille (finer dots, where the terminal font has them), or off. Needs 24-bit colour and
    /// a palette theme; a flat screen otherwise.
    #[arg(long, default_value = "braille", value_parser = ["blocks", "braille", "off"])]
    pub dither: String,
}
