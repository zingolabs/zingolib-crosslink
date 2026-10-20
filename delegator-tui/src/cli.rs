//! Command line arguments.

use std::path::PathBuf;

use clap::Parser;

/// Watch-only terminal wallet for Zcash crosslink delegators.
///
/// Opens the wallet file if one exists, otherwise walks through creating one from a unified
/// full viewing key. Pass --ufvk and --birthday to skip the first-run screen.
#[derive(Parser, Debug, Clone)]
#[command(name = "delegator-tui", version, about)]
pub struct Cli {
    /// Network of the wallet to open: mainnet, testnet, or regtest. Defaults to mainnet. When
    /// creating a wallet the key's prefix decides, and this only has to agree with it.
    #[arg(long, short = 'c', value_parser = ["mainnet", "testnet", "regtest"])]
    pub chain: Option<String>,

    /// Lightwalletd URL. Defaults to zec.rocks for mainnet and testnet, 127.0.0.1:9067 for
    /// regtest.
    #[arg(long)]
    pub server: Option<String>,

    /// Directory that holds the wallet file. Defaults to the platform Zcash data directory.
    #[arg(long, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// Wallet file name inside the data directory.
    #[arg(long, default_value = "delegator-tui.dat", value_name = "NAME")]
    pub wallet_name: String,

    /// Create the wallet from this unified full viewing key. Fails if a wallet file exists.
    #[arg(long, value_name = "UFVK", requires = "birthday")]
    pub ufvk: Option<String>,

    /// Block height the viewing key was created at. Required with --ufvk.
    #[arg(long, value_name = "HEIGHT")]
    pub birthday: Option<u32>,

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
}
