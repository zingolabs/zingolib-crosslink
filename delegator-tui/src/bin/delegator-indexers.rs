//! Lists a crosslink chain's indexers, best first: the built-in seeds and every peer the
//! node network reports, measured and cross-checked, then ranked by freshness and speed.
//! The same discovery pass the wallet runs in the background, on its own.
//!
//! `delegator-indexers [--network nightly]`

use std::process::ExitCode;

use clap::Parser;
use zingo_netutils::discovery;

#[derive(Parser, Debug)]
#[command(name = "delegator-indexers", version, about)]
struct Args {
    /// Crosslink chain: stable (the featurenet) or nightly. Defaults to stable.
    #[arg(long, short = 'n', value_parser = ["stable", "nightly"])]
    network: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("error: cannot set up TLS: {e:?}");
        return ExitCode::FAILURE;
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(args))
}

async fn run(args: Args) -> ExitCode {
    let network = match args.network.as_deref() {
        Some("nightly") => false,
        Some("stable") | None => true,
        Some(other) => {
            eprintln!("error: unknown network {other}");
            return ExitCode::FAILURE;
        }
    };
    let config = delegator_tui::backend::discovery_config(if network {
        delegator_tui::app::Network::Stable
    } else {
        delegator_tui::app::Network::Nightly
    });
    let report = discovery::discover(&config, None).await;
    for error in &report.errors {
        eprintln!("note: {error}");
    }
    if report.ranking.is_empty() {
        eprintln!("no indexer answered");
        return ExitCode::FAILURE;
    }
    println!(
        "{} indexers (from {} peers), best first:",
        report.ranking.len(),
        report.peers_seen
    );
    for indexer in &report.ranking {
        println!(
            "{:>5} ms  height {:<9} chain {:<8} {:<10} {}",
            indexer.latency.as_millis(),
            indexer
                .tip_height
                .map(|h| h.to_string())
                .unwrap_or_else(|| "?".into()),
            indexer.chain_name.as_deref().unwrap_or("?"),
            format!("{:?}", indexer.source).to_lowercase(),
            indexer.uri,
        );
    }
    ExitCode::SUCCESS
}
