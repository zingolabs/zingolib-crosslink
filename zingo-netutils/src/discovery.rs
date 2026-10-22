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

pub use measure::indexer_key;

/// Where a candidate indexer was learned from, in order of trust: the first is trusted most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

/// The consensus branch a node must report to serve the caller's chain, as a function of the
/// node's tip height. A node that missed a network upgrade keeps its old branch ID past the
/// upgrade height and builds a chain of its own: its blocks answer, its tip even moves, but
/// the caller's transactions fail there and its own history disagrees with it. Callers build
/// the rule from their own consensus parameters, so "the right branch" means the one their
/// transactions are built for.
#[derive(Clone)]
pub struct BranchRule(std::sync::Arc<dyn Fn(u64) -> u32 + Send + Sync>);

impl BranchRule {
    /// A rule from a function of the height to the branch ID valid there.
    pub fn new(rule: impl Fn(u64) -> u32 + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(rule))
    }

    /// The branch ID valid at `height`.
    pub fn at(&self, height: u64) -> u32 {
        (self.0)(height)
    }

    /// Why `indexer` is on another chain, or `None` when it is not known to be. A node reports
    /// the branch of the block it would build next, so either its tip's branch or the next
    /// block's passes; a node that reports no branch or no tip is not condemned.
    pub fn mismatch(&self, indexer: &ScoredIndexer) -> Option<String> {
        let reported = indexer.consensus_branch.as_deref()?;
        let tip = indexer.tip_height?;
        let branch = u32::from_str_radix(reported.trim_start_matches("0x"), 16).ok()?;
        let expected = self.at(tip.saturating_add(1));
        (branch != expected && branch != self.at(tip)).then(|| {
            format!(
                "{}: consensus branch {branch:08x} at height {tip}, this chain's is {expected:08x}",
                indexer.uri
            )
        })
    }
}

impl std::fmt::Debug for BranchRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BranchRule")
    }
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
    /// The branch each node must report, when the caller knows its chain's upgrades. Nodes on
    /// another branch never rank, and each one is named in the report's errors.
    pub expected_branch: Option<BranchRule>,
}

impl DiscoveryConfig {
    /// This configuration, keeping only nodes on the branch `rule` gives their height.
    pub fn with_branch_rule(self, rule: BranchRule) -> Self {
        Self {
            expected_branch: Some(rule),
            ..self
        }
    }
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
            expected_branch: None,
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
        candidates.push((format!("http://{host}:{grpc_port}"), Source::PeerExchange));
    }
    let candidates = distinct(config, candidates);

    let mut ranking = measure::rank(
        measure::measure_all(candidates, wallet_chain, config.timeout, config.concurrency).await,
        wallet_chain,
    );
    drop_other_branches(config, &mut ranking, &mut errors);

    IndexerReport {
        ranking,
        peers_seen,
        errors,
    }
}

/// Each indexer once, in its [`indexer_key`] form, in first-seen order. A node listed twice keeps
/// its most trusted source, and one of `config`'s seeds is always [`Source::Builtin`], whatever
/// the caller learned it as. Candidates that are not addresses are dropped.
fn distinct(config: &DiscoveryConfig, candidates: Vec<(String, Source)>) -> Vec<(String, Source)> {
    let seeds: Vec<String> = config
        .seed_uris
        .iter()
        .filter_map(|uri| indexer_key(uri))
        .collect();
    let mut out: Vec<(String, Source)> = Vec::with_capacity(candidates.len());
    for (uri, source) in candidates {
        let Some(key) = indexer_key(&uri) else {
            continue;
        };
        let source = if seeds.contains(&key) {
            Source::Builtin
        } else {
            source
        };
        match out.iter_mut().find(|(known, _)| *known == key) {
            Some((_, known)) => *known = (*known).min(source),
            None => out.push((key, source)),
        }
    }
    out
}

/// Removes the nodes `config`'s branch rule says sit on another chain, naming each in `errors`.
fn drop_other_branches(
    config: &DiscoveryConfig,
    ranking: &mut Vec<ScoredIndexer>,
    errors: &mut Vec<String>,
) {
    let Some(rule) = &config.expected_branch else {
        return;
    };
    ranking.retain(|indexer| match rule.mismatch(indexer) {
        Some(why) => {
            errors.push(why);
            false
        }
        None => true,
    });
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
    let uris = distinct(config, uris);
    let mut ranking = measure::rank(
        measure::measure_all(uris, wallet_chain, config.timeout, config.concurrency).await,
        wallet_chain,
    );
    let mut errors = Vec::new();
    drop_other_branches(config, &mut ranking, &mut errors);
    IndexerReport {
        ranking,
        peers_seen: 0,
        errors,
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

    fn indexer(uri: &str, branch: Option<&str>, tip: Option<u64>) -> ScoredIndexer {
        ScoredIndexer {
            uri: uri.into(),
            chain_name: Some("test".into()),
            consensus_branch: branch.map(str::to_string),
            tip_height: tip,
            tip_age_secs: Some(5),
            latency: std::time::Duration::from_millis(20),
            source: Source::Builtin,
        }
    }

    #[test]
    fn a_node_that_missed_an_upgrade_is_named_and_dropped() {
        // an upgrade at 55296: the old branch below it, the new one from it
        let rule = BranchRule::new(|height| {
            if height < 55_296 {
                0x37a5_165b
            } else {
                0x4654_5531
            }
        });
        let config = DiscoveryConfig::crosslink_stable().with_branch_rule(rule.clone());
        let mut ranking = vec![
            indexer("http://upgraded", Some("46545531"), Some(78_916)),
            indexer("http://stale", Some("37a5165b"), Some(62_152)),
            indexer("http://silent", None, None),
            indexer("http://before", Some("37a5165b"), Some(50_000)),
            // a node at the last block before the upgrade already builds the next one
            indexer("http://edge", Some("46545531"), Some(55_295)),
        ];
        let mut errors = Vec::new();
        drop_other_branches(&config, &mut ranking, &mut errors);
        let kept: Vec<&str> = ranking.iter().map(|i| i.uri.as_str()).collect();
        assert_eq!(
            kept,
            [
                "http://upgraded",
                "http://silent",
                "http://before",
                "http://edge"
            ]
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("http://stale"), "{}", errors[0]);
        assert!(errors[0].contains("37a5165b") && errors[0].contains("46545531"));
        // without a rule nothing is dropped
        let mut all = vec![indexer("http://stale", Some("37a5165b"), Some(62_152))];
        drop_other_branches(&DiscoveryConfig::crosslink_stable(), &mut all, &mut errors);
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn one_node_spelled_two_ways_is_one_candidate_and_a_seed_stays_builtin() {
        let config = DiscoveryConfig {
            seed_uris: vec!["http://88.198.67.189:18234".into()],
            ..DiscoveryConfig::crosslink_stable()
        };
        let candidates = distinct(
            &config,
            vec![
                // the light client's own spelling, with a trailing slash, learned as a peer
                ("http://88.198.67.189:18234/".into(), Source::PeerExchange),
                ("http://88.198.67.189:18234".into(), Source::PeerExchange),
                ("95.111.225.120".into(), Source::PeerExchange),
                ("http://95.111.225.120:18234".into(), Source::DnsSeed),
                ("not a uri".into(), Source::PeerExchange),
            ],
        );
        assert_eq!(
            candidates,
            vec![
                ("http://88.198.67.189:18234".to_string(), Source::Builtin),
                ("http://95.111.225.120:18234".to_string(), Source::DnsSeed),
            ]
        );
        assert_eq!(
            indexer_key("HTTPS://Node.Example"),
            Some("https://node.example:443".into())
        );
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
