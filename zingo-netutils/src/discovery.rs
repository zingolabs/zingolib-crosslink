//! Indexer discovery for the crosslink chains.
//!
//! A crosslink node embeds its lightwalletd-compatible gRPC service, so every peer on the
//! node's own P2P network is an indexer candidate. Discovery asks a small bootstrap set for
//! more peers, measures every candidate once, cross-checks the answers against the wallet's
//! own synced block, and ranks the survivors: correct chain, freshest tip, highest height,
//! lowest latency. Callers prepend the ranking to their indexer list, so the light client
//! starts on the best answer and fails over down the list.
//!
//! Discovery never hard-fails: if the P2P exchange finds nothing, the built-in seed endpoints
//! are measured on their own, so a ranking always holds at least today's known indexers.

pub mod measure;
pub mod p2p;
pub mod seeds;

/// Where a candidate indexer was learned from, in order of trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A built-in seed endpoint, shipped with the library.
    Builtin,
    /// A host resolved from a DNS seed.
    DnsSeed,
    /// An address learned from another node's address gossip.
    PeerExchange,
}

/// A gRPC endpoint candidate, with everything the measurement learned about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoredIndexer {
    /// The gRPC URI, ready for a light client config.
    pub uri: String,
    /// The `chain_name` the node reports, when it answered.
    pub chain_name: Option<String>,
    /// The `consensus_branch_id` the node reports, when it answered.
    pub consensus_branch: Option<String>,
    /// The tip height the node reports, when it answered.
    pub tip_height: Option<u64>,
    /// Seconds since the tip block, when the node served one.
    pub tip_age_secs: Option<u64>,
    /// The connect-plus-`GetLightdInfo` round trip.
    pub latency: std::time::Duration,
    pub source: Source,
}

/// The wallet's newest synced block, used to tell the crosslink chains apart and to catch a
/// node serving the wrong chain. Stable and nightly run identical software, so only the block
/// hash at a height the wallet has seen settles which chain a node serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainFingerprint {
    pub height: u32,
    pub hash: [u8; 32],
}

/// Knobs for one discovery run. Defaults come from [`seeds`].
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    /// gRPC seed endpoints to measure directly, `host:port` or full URIs.
    pub seed_uris: Vec<String>,
    /// DNS seed hostnames; each address of each is a P2P bootstrap candidate.
    pub dns_seeds: Vec<String>,
    /// P2P listener ports to try on seed hosts that have no resolved DNS seed.
    pub p2p_ports: Vec<u16>,
    /// The embedded gRPC port on discovered peers: their P2P port plus this offset.
    pub grpc_port_offset: u16,
    /// The P2P network magic, when discovery may crawl this chain's P2P network. `None`
    /// disables the peer exchange (the seeds are still probed).
    pub magic: Option<[u8; 4]>,
    /// Per-peer and per-probe timeout.
    pub timeout: std::time::Duration,
    /// Probe concurrency bound.
    pub concurrency: usize,
    /// Cap on peers crawled from the P2P network, so one loud node cannot flood the list.
    pub max_peers: usize,
    /// Whether to attempt a P2P address exchange at all. `false` measures the seeds only.
    pub crawl_peers: bool,
}

impl DiscoveryConfig {
    /// The featurenet ("stable") configuration from [`seeds`].
    pub fn crosslink_stable() -> Self {
        Self {
            seed_uris: seeds::CROSSLINK_STABLE_SEED_URIS
                .iter()
                .map(|uri| uri.to_string())
                .collect(),
            dns_seeds: seeds::CROSSLINK_DNS_SEEDS
                .iter()
                .map(|seed| seed.to_string())
                .collect(),
            p2p_ports: seeds::P2P_PORTS.to_vec(),
            grpc_port_offset: seeds::GRPC_PORT_OFFSET,
            magic: Some(seeds::CROSSLINK_STABLE_MAGIC),
            timeout: seeds::PROBE_TIMEOUT,
            concurrency: seeds::PROBE_CONCURRENCY,
            max_peers: seeds::MAX_PEERS,
            crawl_peers: true,
        }
    }

    /// The nightly configuration from [`seeds`]. Its P2P magic is not published yet, so the
    /// peer exchange stays off until it is; the seeds are measured on their own.
    pub fn crosslink_nightly() -> Self {
        Self {
            seed_uris: seeds::CROSSLINK_NIGHTLY_SEED_URIS
                .iter()
                .map(|uri| uri.to_string())
                .collect(),
            crawl_peers: false,
            magic: None,
            ..Self::crosslink_stable()
        }
    }
}

/// What one discovery or health check run learned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexerReport {
    /// Every candidate that answered, best first.
    pub ranking: Vec<ScoredIndexer>,
    /// How many peers the P2P exchange surfaced, before measuring.
    pub peers_seen: usize,
    /// What went wrong along the way, for the log and the UI.
    pub errors: Vec<String>,
}

impl IndexerReport {
    /// The gRPC URIs in rank order, ready to prepend to an indexer list.
    pub fn uris(&self) -> Vec<String> {
        self.ranking.iter().map(|s| s.uri.clone()).collect()
    }
}

/// The error type for the P2P exchange.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("no bootstrap peer answered the handshake")]
    NoBootstrap,
    #[error("the address exchange returned nothing")]
    NoPeers,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("p2p protocol: {0}")]
    Protocol(String),
}

/// Runs one discovery pass: peer exchange, seeds, measurements, cross-check, rank.
///
/// With `wallet_chain`, nodes whose block at that height fails the hash check are dropped;
/// without one, everything that answers is ranked, and the caller must verify the chain
/// itself before trusting the first entry.
pub async fn discover(
    config: &DiscoveryConfig,
    wallet_chain: Option<ChainFingerprint>,
) -> IndexerReport {
    let mut errors = Vec::new();

    let (mut candidate_hosts, peers_seen) = if config.crawl_peers {
        match p2p::peer_exchange(config).await {
            Ok(peers) => peers,
            Err(e) => {
                errors.push(format!("peer exchange: {e}"));
                (Vec::new(), 0)
            }
        }
    } else {
        (Vec::new(), 0)
    };

    let mut candidates: Vec<(String, Source)> = config
        .seed_uris
        .iter()
        .map(|uri| (uri.clone(), Source::Builtin))
        .collect();
    for (host, p2p_port) in candidate_hosts.drain(..) {
        let grpc_port = p2p_port.saturating_add(config.grpc_port_offset);
        let uri = format!("http://{host}:{grpc_port}");
        if !candidates.iter().any(|(known, _)| *known == uri) {
            candidates.push((uri, Source::PeerExchange));
        }
    }

    let ranking = measure::rank(
        measure::measure_all(candidates, wallet_chain, config.timeout, config.concurrency).await,
        wallet_chain,
    );

    IndexerReport {
        ranking,
        peers_seen,
        errors,
    }
}

/// Runs a health check over a known list of indexers: the same measurements and ranking as
/// [`discover`], without the peer exchange, so a wallet can refresh its picture of the
/// indexer set every few minutes without asking the node network for addresses again.
/// `uris` carry their source so the ranking keeps telling shipped and peer-found nodes
/// apart.
pub async fn health_check(
    config: &DiscoveryConfig,
    uris: Vec<(String, Source)>,
    wallet_chain: Option<ChainFingerprint>,
) -> IndexerReport {
    let ranking = measure::rank(
        measure::measure_all(uris, wallet_chain, config.timeout, config.concurrency).await,
        wallet_chain,
    );
    IndexerReport {
        ranking,
        peers_seen: 0,
        errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(height: u32) -> ChainFingerprint {
        ChainFingerprint {
            height,
            hash: [7; 32],
        }
    }

    #[test]
    fn config_defaults_differ_only_in_their_seeds() {
        let stable = DiscoveryConfig::crosslink_stable();
        let nightly = DiscoveryConfig::crosslink_nightly();
        assert!(!stable.seed_uris.is_empty());
        assert!(!nightly.seed_uris.is_empty());
        assert_ne!(stable.seed_uris, nightly.seed_uris);
        assert_eq!(stable.timeout, nightly.timeout);
        assert_eq!(stable.grpc_port_offset, nightly.grpc_port_offset);
        assert_eq!(stable.magic, Some(seeds::CROSSLINK_STABLE_MAGIC));
        assert_eq!(nightly.magic, None, "the nightly's magic is not published");
        assert!(!nightly.crawl_peers, "no magic, no peer exchange");
        assert!(stable.crawl_peers);
    }

    #[test]
    fn uris_preserve_the_ranking() {
        let report = IndexerReport {
            ranking: vec![
                ScoredIndexer {
                    uri: "http://10.0.0.1:18234".into(),
                    chain_name: Some("test".into()),
                    consensus_branch: None,
                    tip_height: Some(100),
                    tip_age_secs: Some(5),
                    latency: std::time::Duration::from_millis(20),
                    source: Source::PeerExchange,
                },
                ScoredIndexer {
                    uri: "http://10.0.0.2:18234".into(),
                    chain_name: None,
                    consensus_branch: None,
                    tip_height: None,
                    tip_age_secs: None,
                    latency: std::time::Duration::from_millis(900),
                    source: Source::Builtin,
                },
            ],
            peers_seen: 12,
            errors: Vec::new(),
        };
        assert_eq!(
            report.uris(),
            vec![
                "http://10.0.0.1:18234".to_string(),
                "http://10.0.0.2:18234".to_string()
            ]
        );
        assert_eq!(report.peers_seen, 12);
        let _ = fingerprint(1);
    }

    #[test]
    fn crawled_peers_map_their_grpc_port_above_the_listener() {
        let p2p_port: u16 = 8233;
        assert_eq!(p2p_port.saturating_add(seeds::GRPC_PORT_OFFSET), 18_234);
    }

    #[tokio::test]
    async fn a_health_check_of_an_empty_list_answers_nothing() {
        let report =
            super::health_check(&DiscoveryConfig::crosslink_stable(), Vec::new(), None).await;
        assert!(report.ranking.is_empty());
        assert_eq!(report.peers_seen, 0, "a health_check never crawls");
    }

    #[tokio::test]
    #[ignore = "needs the network"]
    async fn crosslink_stable_discovery_live() {
        let report = super::discover(&DiscoveryConfig::crosslink_stable(), None).await;
        println!("peers seen: {}", report.peers_seen);
        for error in &report.errors {
            println!("error: {error}");
        }
        for indexer in &report.ranking {
            println!(
                "{:>3} ms  height {:?}  chain {:?}  {} ({:?})",
                indexer.latency.as_millis(),
                indexer.tip_height,
                indexer.chain_name,
                indexer.uri,
                indexer.source,
            );
        }
        assert!(
            !report.ranking.is_empty(),
            "the built-in seeds alone should answer"
        );
    }
}
