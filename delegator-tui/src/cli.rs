//! Command line arguments.

use std::path::PathBuf;

use clap::Parser;

/// Terminal wallet for Zcash crosslink delegators.
///
/// Starts on the list of wallets in the data directory, from which a wallet is opened or a new
/// one is created, restored from a seed phrase, or set up watch-only from a viewing key. Pass
/// --wallet to open one straight away, or --ufvk and --birthday to create a watch-only wallet
/// without the screens.
#[derive(Parser, Debug, Clone)]
#[command(name = "delegator-tui", version, about)]
pub struct Cli {
    /// Network for a wallet made from a seed phrase, and for --wallet: mainnet, testnet, or
    /// regtest. A viewing key names its own network. Defaults to mainnet.
    #[arg(long, short = 'c', value_parser = ["mainnet", "testnet", "regtest"])]
    pub chain: Option<String>,

    /// Indexer URL. Defaults to zec.rocks for mainnet and testnet, 127.0.0.1:9067 for regtest.
    #[arg(long)]
    pub server: Option<String>,

    /// Directory that holds the wallet files. Defaults to the platform Zcash data directory,
    /// with a subdirectory per network.
    #[arg(long, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// Open this wallet instead of showing the list. The file is NAME.dat in the data
    /// directory of --chain. With --ufvk, the name the new wallet gets.
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

    /// Paint an animated dither gradient (shade blocks) in the empty part of the screen. Needs
    /// a 24-bit colour terminal and a palette theme; a flat screen otherwise.
    #[arg(long)]
    pub experimental_dither: bool,

    /// The same gradient in braille dots: finer, but the glyphs depend on the terminal font.
    /// Takes precedence over --experimental-dither.
    #[arg(long)]
    pub experimental_dither_braille: bool,
}
