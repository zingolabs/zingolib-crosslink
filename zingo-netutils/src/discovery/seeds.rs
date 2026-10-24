//! Built-in bootstrap constants for crosslink indexer discovery.
//!
//! These are the addresses discovery can trust even before the network has told it
//! anything: the Shielded Labs featurenet nodes, the ports their listeners use, and the
//! knobs that keep one discovery pass fast and polite.

/// gRPC seed endpoints of the stable featurenet, measured directly by every discovery run.
pub const CROSSLINK_STABLE_SEED_URIS: &[&str] = &[
    "http://88.198.67.189:18234",
    "http://45.63.75.9:18234",
    "http://70.34.220.94:18234",
    "http://64.177.44.176:18234",
];

/// gRPC seed endpoints of the nightly featurenet, measured directly by every discovery run.
pub const CROSSLINK_NIGHTLY_SEED_URIS: &[&str] =
    &["http://45.32.186.234:18234", "http://64.177.122.73:18234"];

/// DNS seeds are not published yet; Shielded Labs runs the featurenet nodes. When they
/// publish a seeder hostname, add it here and discovery picks it up.
pub const CROSSLINK_DNS_SEEDS: &[&str] = &[];

/// P2P listener ports tried on seed hosts for the peer exchange, in order. Crosslink nodes
/// keep zebra's fixed default listener on 8233; 18233 is the plain testnet default, tried
/// as a fallback.
pub const P2P_PORTS: &[u16] = &[8233, 18233];

/// The embedded gRPC service listens this far above a node's P2P listener: 8233 + 10001 =
/// 18234, the address every deployed crosslink indexer answers on.
/// The deployed crosslink gRPC port: a node's default listener 8233 plus the service
/// offset.
pub const GRPC_PORT_OFFSET: u16 = 10_001;

/// The stable featurenet's P2P network magic, the bytes of `ClT1`, from the monolith's
/// `zebrad` config. The nightly's magic is not published yet, so its peer exchange stays
/// off.
pub const CROSSLINK_STABLE_MAGIC: [u8; 4] = [0x43, 0x6c, 0x54, 0x31];

/// The deployed crosslink gRPC port: a node's default listener 8233 plus the service
/// offset.
pub const GRPC_PORT: u16 = 18_234;

/// How long one measurement may take, covering connect and every RPC on the way.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The whole measurement batch's wall-clock ceiling. Slow DNS or unreachable
/// hosts used to keep the batch waiting on every probe's own timeout in turn's
/// worth of joins; this ends the run and returns the samples that finished.
pub const TOTAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

/// How many measurements run at once.
pub const PROBE_CONCURRENCY: usize = 8;

/// Cap on peers crawled from the P2P network, so one loud node cannot flood the list.
pub const MAX_PEERS: usize = 64;
