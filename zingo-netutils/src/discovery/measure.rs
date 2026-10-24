//! Active probing and ranking of indexer candidates.
//!
//! [`measure_all`] fires one gRPC probe per candidate under a concurrency
//! limit and per-probe timeout, collecting a [`Sample`] for each. [`rank`]
//! turns those samples into a best-first list of [`ScoredIndexer`]s, dropping
//! indexers that demonstrably sit on a different chain than the wallet.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Semaphore, mpsc};
use zcash_client_backend::proto::service::{BlockId, ChainSpec, Empty};

use super::{ChainFingerprint, ScoredIndexer, Source};

/// One snapshot of how far a measurement batch has gotten, sent on a
/// progress channel after each probe starts and each one finishes. A consumer
/// keeps the batch's `total` and renders "probing {finished}/{total}".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeProgress {
    /// Probes that have begun their actual work (a permit was acquired).
    pub started: usize,
    /// Probes that returned a sample, success or failure.
    pub finished: usize,
    /// How many candidates this batch measures.
    pub total: usize,
}

/// Where a batch reports its progress. Each held copy sends one
/// [`ProbeProgress`] snapshot per probe start and finish; keep the receiving
/// half of the `mpsc::unbounded_channel`.
pub type ProgressSender = mpsc::UnboundedSender<ProbeProgress>;

/// One probe result: which indexer was asked, how long it took, and what it
/// answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// The candidate URI as it was probed.
    pub uri: String,
    /// How this candidate was discovered.
    pub source: Source,
    /// Wall-clock cost of the whole probe, timeouts included, so ranking can
    /// prefer responsive nodes.
    pub latency: Duration,
    /// The facts the node reported, or why the probe failed.
    pub answer: Result<SampleFacts, String>,
}

/// What a single node told us. Every fact is optional because gRPC answers
/// can be empty, partial, or fail partway through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SampleFacts {
    /// Reported network name, with empty strings treated as absent.
    pub chain_name: Option<String>,
    /// Reported consensus branch ID, with empty strings treated as absent.
    pub consensus_branch: Option<String>,
    /// Best-known tip height, preferring `GetLatestBlock` over the info
    /// banner's cached height.
    pub tip_height: Option<u64>,
    /// Seconds since the tip block's timestamp, when that block could be
    /// read.
    pub tip_age_secs: Option<u64>,
    /// Whether the block at the wallet's own height matches the wallet's
    /// hash. `None` when the wallet supplied no fingerprint or the block
    /// could not be read, so only a definite `Some(false)` damns a node.
    pub fingerprint_match: Option<bool>,
}

/// Probe every candidate, at most `concurrency` at a time and `timeout` per
/// probe.
///
/// Returns one [`Sample`] per candidate that ran to completion, in candidate
/// order; a probe task that panics is logged and skipped rather than failing
/// the batch.
///
/// Without a total budget (see [`measure_all_with_budget`]) the batch waits on
/// every probe's own timeout, so a wide candidate list with unreachable hosts
/// takes the whole `timeout` per wave of `concurrency`.
pub async fn measure_all(
    candidates: Vec<(String, Source)>,
    wallet_chain: Option<ChainFingerprint>,
    timeout: Duration,
    concurrency: usize,
) -> Vec<Sample> {
    measure_all_with(
        candidates,
        wallet_chain,
        timeout,
        concurrency,
        None,
        None,
    )
    .await
}

/// [`measure_all`] under a whole-batch wall-clock budget. `Some(ZERO)` counts
/// as no budget.
///
/// When the budget runs out, the in-flight probes are abandoned and the
/// samples that finished come back: the unreachable hosts cost nothing but
/// their rank omission, and the answers already in hand still rank.
pub async fn measure_all_with_budget(
    candidates: Vec<(String, Source)>,
    wallet_chain: Option<ChainFingerprint>,
    timeout: Duration,
    concurrency: usize,
    total_budget: Option<Duration>,
) -> Vec<Sample> {
    measure_all_with(candidates, wallet_chain, timeout, concurrency, total_budget, None).await
}

/// [`measure_all`] with every knob: a whole-batch [`Duration`] ceiling and an
/// optional progress channel. `Some(ZERO)` counts as no budget.
pub async fn measure_all_with(
    candidates: Vec<(String, Source)>,
    wallet_chain: Option<ChainFingerprint>,
    timeout: Duration,
    concurrency: usize,
    total_budget: Option<Duration>,
    progress: Option<ProgressSender>,
) -> Vec<Sample> {
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let total = candidates.len();
    let mut tasks: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = Sample> + Send>>> =
        Vec::with_capacity(candidates.len());
    for (uri, source) in candidates {
        let semaphore = Arc::clone(&semaphore);
        let started = Arc::clone(&started);
        let progress = progress.clone();
        tasks.push(Box::pin(async move {
            match semaphore.acquire_owned().await {
                Ok(_permit) => {
                    let count = started.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if let Some(channel) = &progress {
                        let _ = channel.send(ProbeProgress {
                            started: count,
                            finished: 0,
                            total,
                        });
                    }
                    probe_one(uri, source, wallet_chain, timeout).await
                }
                Err(e) => {
                    warn(&uri, &format!("probe limiter closed: {e}"));
                    Sample {
                        uri,
                        source,
                        latency: Duration::ZERO,
                        answer: Err("probe limiter closed".to_string()),
                    }
                }
            }
        }));
    }

    let mut probes = Vec::with_capacity(total);
    let mut batch = tokio::task::JoinSet::new();
    for task in tasks {
        batch.spawn(task);
    }
    let deadline = total_budget
        .filter(|budget| !budget.is_zero())
        .map(|budget| tokio::time::Instant::now() + budget);
    let channel = progress;
    loop {
        let next = match deadline {
            None => batch.join_next().await,
            Some(deadline) => match tokio::time::timeout_at(deadline, batch.join_next()).await {
                Ok(next) => next,
                Err(_) => {
                    let remaining = batch.len();
                    warn(
                        "(batch)",
                        &format!(
                            "total budget elapsed: abandoning {remaining} probes, keeping {}",
                            probes.len()
                        ),
                    );
                    break;
                }
            },
        };
        let Some(joined) = next else {
            break;
        };
        match joined {
            Ok(probe) => probes.push(probe),
            Err(e) => warn("(join)", &format!("probe task failed: {e}")),
        }
        if let Some(channel) = &channel {
            let _ = channel.send(ProbeProgress {
                started: started.load(std::sync::atomic::Ordering::Relaxed),
                finished: probes.len(),
                total,
            });
        }
    }
    probes
}

/// Order probed indexers best-first and drop provably wrong-chain ones.
///
/// A candidate whose block hash disagrees with the wallet's fingerprint at
/// the wallet's height never ranks. The rest sort by fingerprint match, then
/// whether they answered at all, then freshest tip, highest height, lowest
/// latency.
pub fn rank(raw: Vec<Sample>, _wallet_chain: Option<ChainFingerprint>) -> Vec<ScoredIndexer> {
    let mut ranked: Vec<Entry> = raw
        .into_iter()
        .filter(
            |probe| !matches!(&probe.answer, Ok(answer) if answer.fingerprint_match == Some(false)),
        )
        .map(|probe| {
            let fingerprint = match &probe.answer {
                Ok(answer) => answer.fingerprint_match,
                Err(_) => None,
            };
            let (answered, chain_name, consensus_branch, tip_height, tip_age_secs) =
                match probe.answer {
                    Ok(answer) => (
                        true,
                        answer.chain_name,
                        answer.consensus_branch,
                        answer.tip_height,
                        answer.tip_age_secs,
                    ),
                    Err(_) => (false, None, None, None, None),
                };
            Entry {
                scored: ScoredIndexer {
                    uri: probe.uri,
                    chain_name,
                    consensus_branch,
                    tip_height,
                    tip_age_secs,
                    latency: probe.latency,
                    source: probe.source,
                },
                fingerprint,
                answered,
            }
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.fingerprint
            .cmp(&a.fingerprint)
            .then_with(|| b.answered.cmp(&a.answered))
            .then_with(|| cmp_none_last(a.scored.tip_age_secs, b.scored.tip_age_secs))
            .then_with(|| b.scored.tip_height.cmp(&a.scored.tip_height))
            .then_with(|| a.scored.latency.cmp(&b.scored.latency))
    });

    ranked.into_iter().map(|entry| entry.scored).collect()
}

struct Entry {
    scored: ScoredIndexer,
    fingerprint: Option<bool>,
    answered: bool,
}

fn cmp_none_last(a: Option<u64>, b: Option<u64>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => a.cmp(&b),
    }
}

async fn probe_one(
    uri: String,
    source: Source,
    wallet_chain: Option<ChainFingerprint>,
    timeout: Duration,
) -> Sample {
    let started = std::time::Instant::now();
    let answer = match tokio::time::timeout(timeout, run_probe(&uri, wallet_chain)).await {
        Ok(answer) => answer,
        Err(_) => {
            warn(&uri, "timed out");
            Err("timed out".to_string())
        }
    };
    Sample {
        uri,
        source,
        latency: started.elapsed(),
        answer,
    }
}

async fn run_probe(
    raw: &str,
    wallet_chain: Option<ChainFingerprint>,
) -> Result<SampleFacts, String> {
    let uri = normalize_uri(raw)
        .ok_or_else(|| format!("invalid uri: {raw}"))?
        .parse::<http::Uri>()
        .map_err(|e| format!("invalid uri {raw}: {e}"))?;

    let mut client = crate::get_client(uri)
        .await
        .map_err(|e| format!("connect: {e}"))?;

    let info = client
        .get_lightd_info(tonic::Request::new(Empty {}))
        .await
        .map_err(|e| format!("GetLightdInfo: {e}"))?
        .into_inner();
    let chain_name = non_empty(info.chain_name);
    let consensus_branch = non_empty(info.consensus_branch_id);
    let reported_height = (info.block_height != 0).then_some(info.block_height);

    let latest = client
        .get_latest_block(tonic::Request::new(ChainSpec {}))
        .await
        .map_err(|e| format!("GetLatestBlock: {e}"))?
        .into_inner();
    let tip_height = (latest.height != 0)
        .then_some(latest.height)
        .or(reported_height);

    let tip_age_secs = if latest.height != 0 {
        match read_block(&mut client, latest.height).await {
            Ok(block) => Some(elapsed_secs(block.time)),
            Err(e) => {
                warn(raw, &e);
                None
            }
        }
    } else {
        None
    };

    let fingerprint_match = match wallet_chain {
        Some(fingerprint) => match read_block(&mut client, u64::from(fingerprint.height)).await {
            Ok(block) => Some(block.hash.as_slice() == fingerprint.hash.as_slice()),
            Err(e) => {
                warn(raw, &e);
                None
            }
        },
        None => None,
    };

    Ok(SampleFacts {
        chain_name,
        consensus_branch,
        tip_height,
        tip_age_secs,
        fingerprint_match,
    })
}

async fn read_block(
    client: &mut zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient<tonic::transport::Channel>,
    height: u64,
) -> Result<zcash_client_backend::proto::compact_formats::CompactBlock, String> {
    let block = client
        .get_block(tonic::Request::new(BlockId {
            height,
            hash: vec![],
        }))
        .await
        .map_err(|e| format!("GetBlock({height}): {e}"))?
        .into_inner();
    Ok(block)
}

fn elapsed_secs(unix_time_secs: u32) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.saturating_sub(Duration::from_secs(u64::from(unix_time_secs)))
        .as_secs()
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn warn(uri: &str, what: &str) {
    tracing::warn!("discovery probe {uri}: {what}");
}

/// The one form of an indexer's address, `scheme://host:port`, so that two spellings of the same
/// node, `http://h:18234` and `http://h:18234/` say, are one candidate. A bare host gets
/// `http` and the crosslink gRPC port. `None` when `raw` is not an address.
pub fn indexer_key(raw: &str) -> Option<String> {
    let uri: http::Uri = normalize_uri(&raw.to_ascii_lowercase())?.parse().ok()?;
    let scheme = uri.scheme_str().unwrap_or("http");
    let port = uri
        .port_u16()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    Some(format!("{scheme}://{}:{port}", uri.host()?))
}

fn normalize_uri(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let with_scheme = if raw.starts_with("http") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let uri: http::Uri = with_scheme.parse().ok()?;
    if uri.port().is_none() && uri.scheme_str() == Some("http") {
        format!("{with_scheme}:{}", super::seeds::GRPC_PORT)
            .parse()
            .ok()
    } else {
        Some(uri.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{mock_node, now_secs, spawn_mock};
    use super::*;

    fn answer() -> SampleFacts {
        SampleFacts::default()
    }

    fn probe(uri: &str, answer: Result<SampleFacts, String>, latency_ms: u64) -> Sample {
        Sample {
            uri: uri.to_string(),
            source: Source::Builtin,
            latency: Duration::from_millis(latency_ms),
            answer,
        }
    }

    #[test]
    fn wrong_chain_is_dropped() {
        let mut good = answer();
        good.fingerprint_match = Some(true);
        let mut bad = answer();
        bad.fingerprint_match = Some(false);
        let ranked = rank(
            vec![probe("bad", Ok(bad), 1), probe("good", Ok(good), 1)],
            None,
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].uri, "good");
    }

    #[test]
    fn matching_fingerprint_ranks_above_no_fingerprint() {
        let mut matching = answer();
        matching.fingerprint_match = Some(true);
        matching.tip_height = Some(10);
        let ranked = rank(
            vec![
                probe("no-fingerprint", Ok(answer()), 1),
                probe("matching", Ok(matching), 1),
            ],
            Some(ChainFingerprint {
                height: 1,
                hash: [7; 32],
            }),
        );
        assert_eq!(ranked[0].uri, "matching");
    }

    #[test]
    fn fresher_tip_wins() {
        let mut stale = answer();
        stale.tip_age_secs = Some(60);
        let mut fresh = answer();
        fresh.tip_age_secs = Some(5);
        let ranked = rank(
            vec![probe("stale", Ok(stale), 1), probe("fresh", Ok(fresh), 1)],
            None,
        );
        assert_eq!(ranked[0].uri, "fresh");
    }

    #[test]
    fn higher_height_breaks_ties() {
        let mut lower = answer();
        lower.tip_age_secs = Some(5);
        lower.tip_height = Some(100);
        let mut higher = answer();
        higher.tip_age_secs = Some(5);
        higher.tip_height = Some(200);
        let ranked = rank(
            vec![probe("lower", Ok(lower), 1), probe("higher", Ok(higher), 1)],
            None,
        );
        assert_eq!(ranked[0].uri, "higher");
    }

    #[test]
    fn lower_latency_breaks_ties() {
        let ranked = rank(
            vec![
                probe("slow", Ok(answer()), 100),
                probe("fast", Ok(answer()), 20),
            ],
            None,
        );
        assert_eq!(ranked[0].uri, "fast");
    }

    #[test]
    fn answered_ranks_above_failed() {
        let mut answered = answer();
        answered.tip_height = Some(10);
        let ranked = rank(
            vec![
                probe("failed", Err("timed out".to_string()), 1),
                probe("answered", Ok(answered), 1),
            ],
            None,
        );
        assert_eq!(ranked[0].uri, "answered");
        assert_eq!(ranked[1].uri, "failed");
    }

    #[test]
    fn bare_host_gets_scheme_and_default_port() {
        assert_eq!(
            normalize_uri("example.com").as_deref(),
            Some("http://example.com:18234")
        );
        assert_eq!(
            normalize_uri("example.com:9944").as_deref(),
            Some("http://example.com:9944/")
        );
        assert_eq!(
            normalize_uri("https://example.com").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(normalize_uri(""), None);
        assert_eq!(normalize_uri("http://[bad"), None);
    }

    #[tokio::test]
    async fn measure_all_reads_canned_answers_from_mock_node() {
        let addr = spawn_mock(mock_node(now_secs())).await;

        let candidates = vec![(format!("{}:{}", addr.ip(), addr.port()), Source::Builtin)];
        let wallet_chain = Some(ChainFingerprint {
            height: 900,
            hash: [7u8; 32],
        });

        let probes = measure_all(candidates, wallet_chain, Duration::from_secs(5), 4).await;

        assert_eq!(probes.len(), 1);
        let answer = probes[0]
            .answer
            .as_ref()
            .expect("probe against mock node answered");
        assert_eq!(answer.chain_name.as_deref(), Some("test"));
        assert_eq!(answer.consensus_branch.as_deref(), Some("abc123"));
        assert_eq!(answer.tip_height, Some(1000));
        assert_eq!(answer.fingerprint_match, Some(true));
        assert!(
            answer.tip_age_secs.unwrap() < 60,
            "tip should read as fresh"
        );
    }

    #[tokio::test]
    async fn wrong_chain_answer_is_reported_then_dropped_by_rank() {
        let addr = spawn_mock(mock_node(now_secs())).await;

        let wallet_chain = Some(ChainFingerprint {
            height: 900,
            hash: [1u8; 32],
        });
        let probes = measure_all(
            vec![(format!("http://{addr}"), Source::PeerExchange)],
            wallet_chain,
            Duration::from_secs(5),
            4,
        )
        .await;

        let answer = probes[0].answer.as_ref().expect("answered");
        assert_eq!(answer.fingerprint_match, Some(false));
        assert!(
            rank(probes, wallet_chain).is_empty(),
            "wrong chain must not rank"
        );
    }

    #[tokio::test]
    async fn unreachable_node_answers_err_without_hanging() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local_addr");
        drop(listener);

        let probes = measure_all(
            vec![(format!("http://{addr}"), Source::Builtin)],
            None,
            Duration::from_millis(500),
            1,
        )
        .await;

        assert_eq!(probes.len(), 1);
        let error = probes[0].answer.as_ref().expect_err("must fail fast");
        assert!(!error.is_empty());
    }

    /// A listener that accepts connections and never answers: the probe's own
    /// timeout is the only thing that would eventually free it.
    async fn spawn_silent_server() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket); // sockets kept open, never answered
            }
        });
        addr
    }

    fn seconds_elapsed(started: std::time::Instant) -> u64 {
        started.elapsed().as_secs()
    }

    #[tokio::test]
    async fn the_total_budget_ends_the_batch_and_keeps_the_fast_answers() {
        let slow = spawn_silent_server().await;
        let fast = spawn_mock(mock_node(now_secs())).await;

        let started = std::time::Instant::now();
        let probes = measure_all_with_budget(
            vec![
                (format!("http://{slow}"), Source::Builtin),
                (format!("http://{fast}"), Source::Builtin),
            ],
            None,
            Duration::from_secs(30),
            4,
            Some(Duration::from_secs(1)),
        )
        .await;

        let elapsed_secs = seconds_elapsed(started);
        let elapsed_ms = started.elapsed().as_millis();
        assert!(
            elapsed_ms < 5_000,
            "the batch must end near the {elapsed_secs}s budget, took {elapsed_ms}ms"
        );

        let fast_uri = format!("http://{fast}");
        let answer = probes
            .iter()
            .find(|probe| probe.uri == fast_uri)
            .expect("the fast probe survived the budget")
            .answer
            .as_ref()
            .expect("the fast probe answered");
        assert_eq!(answer.chain_name.as_deref(), Some("test"));
        let slow_uri = format!("http://{slow}");
        assert!(
            probes.iter().all(|probe| probe.uri != slow_uri),
            "the probe past the budget never ranked"
        );
    }

    #[tokio::test]
    async fn a_zero_budget_counts_as_no_budget() {
        let probes = measure_all_with_budget(
            vec![("http://does-not-resolve.invalid".to_string(), Source::Builtin)],
            None,
            Duration::from_millis(300),
            1,
            Some(Duration::ZERO),
        )
        .await;
        assert!(!probes.is_empty(), "the per-probe timeout still applied");
    }

    #[tokio::test]
    async fn progress_events_show_started_and_finished_counts() {
        let slow = spawn_silent_server().await;
        let fast = spawn_mock(mock_node(now_secs())).await;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

        measure_all_with(
            vec![
                (format!("http://{slow}"), Source::Builtin),
                (format!("http://{fast}"), Source::Builtin),
            ],
            None,
            Duration::from_secs(30),
            4,
            Some(Duration::from_secs(1)),
            Some(sender),
        )
        .await;

        let events: Vec<ProbeProgress> =
            std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        assert!(
            events.iter().any(|event| event.started == 2 && event.total == 2),
            "both probes must report as started: {events:?}"
        );
        let first = events.first().expect("progress events were sent");
        assert_eq!((first.started, first.finished), (1, 0), "{first:?}");
        assert!(
            events
                .iter()
                .any(|event| event.finished == 1 && event.total == 2),
            "the finished count reaches 1 of 2: {events:?}"
        );
    }
}

/// A canned lightwalletd gRPC node for this crate's test modules: it answers
/// `GetLightdInfo`, `GetLatestBlock`, and `GetBlock` from its block table.
#[cfg(test)]
pub(super) mod tests_support {
    use std::sync::Arc;

    pub(crate) struct MockNode {
        chain_name: String,
        consensus_branch_id: String,
        blocks: std::collections::HashMap<u64, (Vec<u8>, u32)>,
        tip_height: u64,
    }

    fn grpc_frame(message: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(5 + message.len());
        frame.push(0);
        frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
        frame.extend_from_slice(message);
        frame
    }

    const WIRE_VARINT: u32 = 0;

    fn varint_field(field: u32, value: u64, out: &mut Vec<u8>) {
        out.push((field << 3 | WIRE_VARINT) as u8);
        let mut value = value;
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn len_delimited_field(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
        out.push(((field << 3) | 2) as u8);
        out.push(bytes.len() as u8);
        out.extend_from_slice(bytes);
    }

    fn lightd_info(node: &MockNode) -> Vec<u8> {
        let mut message = Vec::new();
        len_delimited_field(4, node.chain_name.as_bytes(), &mut message);
        len_delimited_field(6, node.consensus_branch_id.as_bytes(), &mut message);
        varint_field(7, node.tip_height, &mut message);
        message
    }

    fn block_id(height: u64, hash: &[u8]) -> Vec<u8> {
        let mut message = Vec::new();
        varint_field(1, height, &mut message);
        len_delimited_field(2, hash, &mut message);
        message
    }

    fn compact_block(height: u64, hash: &[u8], time: u32) -> Vec<u8> {
        let mut message = Vec::new();
        varint_field(2, height, &mut message);
        len_delimited_field(3, hash, &mut message);
        varint_field(5, u64::from(time), &mut message);
        message
    }

    fn requested_height(body: &[u8]) -> u64 {
        let mut index = 5usize;
        let mut height = 0u64;
        while index < body.len() {
            let tag = body[index];
            index += 1;
            match tag & 0x07 {
                0 => {
                    let mut value = 0u64;
                    let mut shift = 0u32;
                    loop {
                        let byte = body.get(index).copied().unwrap_or(0);
                        index += 1;
                        value |= u64::from(byte & 0x7f) << shift;
                        shift += 7;
                        if byte & 0x80 == 0 {
                            break;
                        }
                    }
                    if tag >> 3 == 1 {
                        height = value;
                    }
                }
                2 => {
                    let mut len = 0usize;
                    let mut shift = 0u32;
                    loop {
                        let byte = body.get(index).copied().unwrap_or(0);
                        index += 1;
                        len |= usize::from(byte & 0x7f) << shift;
                        shift += 7;
                        if byte & 0x80 == 0 {
                            break;
                        }
                    }
                    index += len;
                }
                _ => break,
            }
        }
        height
    }

    struct GrpcBody {
        data: hyper::body::Bytes,
        grpc_status: &'static str,
        trailers_sent: bool,
    }

    impl hyper::body::Body for GrpcBody {
        type Data = hyper::body::Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
            if !self.data.is_empty() {
                let data = std::mem::take(&mut self.data);
                return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(data))));
            }
            if !self.trailers_sent {
                self.trailers_sent = true;
                let mut trailers = http::HeaderMap::new();
                trailers.insert(
                    "grpc-status",
                    http::HeaderValue::from_static(self.grpc_status),
                );
                return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::trailers(trailers))));
            }
            std::task::Poll::Ready(None)
        }
    }

    fn grpc_response(message: &[u8], grpc_status: &'static str) -> http::Response<GrpcBody> {
        http::Response::builder()
            .status(200)
            .header("content-type", "application/grpc")
            .body(GrpcBody {
                data: grpc_frame(message).into(),
                grpc_status,
                trailers_sent: false,
            })
            .expect("static response parts")
    }

    async fn handle(
        node: &MockNode,
        req: http::Request<hyper::body::Incoming>,
    ) -> http::Response<GrpcBody> {
        let path = req.uri().path().to_string();
        let body = {
            use http_body_util::BodyExt;
            req.into_body()
                .collect()
                .await
                .map(|collected| collected.to_bytes())
                .unwrap_or_default()
        };

        if path.ends_with("GetLightdInfo") {
            return grpc_response(&lightd_info(node), "0");
        }
        if path.ends_with("GetLatestBlock") {
            let (hash, _) = node.blocks[&node.tip_height].clone();
            return grpc_response(&block_id(node.tip_height, &hash), "0");
        }
        if path.ends_with("GetBlock") {
            let height = requested_height(&body);
            return match node.blocks.get(&height) {
                Some((hash, time)) => grpc_response(&compact_block(height, hash, *time), "0"),
                None => grpc_response(&[], "5"),
            };
        }
        grpc_response(&[], "12")
    }

    /// Spawn `node` on a loopback port and answer that port.
    pub(crate) async fn spawn_mock(node: MockNode) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local_addr");
        let node = Arc::new(node);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let node = Arc::clone(&node);
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |req| {
                        let node = Arc::clone(&node);
                        async move { Ok::<_, std::convert::Infallible>(handle(&node, req).await) }
                    });
                    let _ = hyper::server::conn::http2::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .serve_connection(hyper_util::rt::TokioIo::new(socket), service)
                    .await;
                });
            }
        });
        addr
    }

    /// A crosslink-looking node: chain `test`, branch `abc123`, tip 1000, and a
    /// block 900 whose hash the fingerprint checks hit.
    pub(crate) fn mock_node(tip_time: u32) -> MockNode {
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(1000, (vec![9u8; 32], tip_time));
        blocks.insert(900, (vec![7u8; 32], tip_time));
        MockNode {
            chain_name: "test".to_string(),
            consensus_branch_id: "abc123".to_string(),
            blocks,
            tip_height: 1000,
        }
    }

    pub(crate) fn now_secs() -> u32 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_secs() as u32
    }
}
