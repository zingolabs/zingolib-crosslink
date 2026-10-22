//! Minimal zebra P2P wire-protocol client for indexer discovery: handshake,
//! `getaddr`/`addr` harvest, and framing under the network magic. Wire facts
//! and limits cite `p2p_spec.md`, next to this file.

use std::collections::{BTreeSet, HashSet};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, lookup_host};
use tokio::task::JoinSet;

/// Protocol version advertised in `version`: the current zebra version on the
/// ClT1 crosslink net, the safest choice against the node's minimum.
pub const PROTOCOL_VERSION: u32 = 170_165;

/// Our `version` user agent, in zebra's `/name:version/` shape.
pub const USER_AGENT: &str = "/zingo-discovery:0.1.0/";

/// The only defined services bit: a full node that can serve block data.
/// Syncing zebra nodes refuse peers that do not claim it.
pub const NODE_NETWORK: u64 = 1;

/// Wire header length: magic, command slot, body length, checksum.
pub const HEADER_LEN: usize = 24;

/// Largest body accepted, checked before any allocation.
pub const MAX_BODY_LEN: usize = 4 * 1024 * 1024;

/// Most `addr` entries accepted in one message, zebra's own limit.
pub const MAX_ADDRS: usize = 1000;

const MAX_BOOTSTRAPS: usize = 8;

const MAX_CONCURRENT_SESSIONS: usize = 2;

const DNS_TIMEOUT: Duration = Duration::from_secs(5);

const MAX_EXCHANGE_WINDOW: Duration = Duration::from_secs(10);

const IDLE_WINDOW: Duration = Duration::from_secs(3);

/// A peer address as it travels in `version`: services, IP, port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerAddr {
    pub services: u64,
    pub ip: IpAddr,
    pub port: u16,
}

/// The `version` payload, fields in wire order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionMessage {
    pub version: u32,
    pub services: u64,
    pub timestamp: i64,
    pub addr_recv: PeerAddr,
    pub addr_from: PeerAddr,
    pub nonce: u64,
    pub user_agent: String,
    pub start_height: u32,
    pub relay: bool,
}

/// One `addr` book entry: last-seen time, services, address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddrEntry {
    pub time: u32,
    pub services: u64,
    pub ip: IpAddr,
    pub port: u16,
}

/// A decoded frame: the command slot and the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub command: [u8; 12],
    pub body: Vec<u8>,
}

impl Message {
    /// Whether the command is `name`, compared as the full padded slot.
    pub fn is(&self, name: &str) -> bool {
        command_bytes(name).is_some_and(|bytes| bytes == self.command)
    }
}

fn command_bytes(name: &str) -> Option<[u8; 12]> {
    let bytes = name.as_bytes();
    if bytes.len() > 12 {
        return None;
    }
    let mut slot = [0u8; 12];
    slot[..bytes.len()].copy_from_slice(bytes);
    Some(slot)
}

/// First four bytes of SHA-256d over the body, as the header carries it.
pub fn checksum(body: &[u8]) -> [u8; 4] {
    let outer = Sha256::digest(Sha256::digest(body));
    [outer[0], outer[1], outer[2], outer[3]]
}

/// Frame a body under `magic`; errors when the command name cannot fit the
/// 12-byte slot.
pub fn encode_message(
    magic: [u8; 4],
    command: &str,
    body: &[u8],
) -> Result<Vec<u8>, super::DiscoveryError> {
    let slot = command_bytes(command).ok_or_else(|| {
        super::DiscoveryError::Protocol(format!("command name {command:?} does not fit the wire"))
    })?;
    let mut frame = Vec::with_capacity(HEADER_LEN + body.len());
    frame.extend_from_slice(&magic);
    frame.extend_from_slice(&slot);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&checksum(body));
    frame.extend_from_slice(body);
    Ok(frame)
}

/// Validate and split one frame: magic, size cap, declared-vs-actual length,
/// checksum.
pub fn decode_message(magic: [u8; 4], frame: &[u8]) -> Result<Message, super::DiscoveryError> {
    if frame.len() < HEADER_LEN {
        return Err(super::DiscoveryError::Protocol(format!(
            "frame is {} bytes, shorter than the {}-byte header",
            frame.len(),
            HEADER_LEN
        )));
    }
    if frame[..4] != magic {
        return Err(super::DiscoveryError::Protocol(format!(
            "magic mismatch: got {:?}, expected {:?}",
            &frame[..4],
            magic
        )));
    }
    let mut command = [0u8; 12];
    command.copy_from_slice(&frame[4..16]);
    let body_len = u32::from_le_bytes(frame[16..20].try_into().expect("4 bytes")) as usize;
    if body_len > MAX_BODY_LEN {
        return Err(super::DiscoveryError::Protocol(format!(
            "body length {body_len} exceeds the {MAX_BODY_LEN}-byte limit"
        )));
    }
    if frame.len() != HEADER_LEN + body_len {
        return Err(super::DiscoveryError::Protocol(format!(
            "declared body length {body_len} does not match frame size {}",
            frame.len() - HEADER_LEN
        )));
    }
    let body = &frame[HEADER_LEN..];
    let expected = checksum(body);
    if frame[20..24] != expected {
        return Err(super::DiscoveryError::Protocol(format!(
            "checksum mismatch: expected {:?}, got {:?}",
            expected,
            &frame[20..24]
        )));
    }
    Ok(Message {
        command,
        body: body.to_vec(),
    })
}

/// Read one frame off the stream; the declared body length is capped before
/// anything is allocated.
pub async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut R,
    magic: [u8; 4],
) -> Result<Message, super::DiscoveryError> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header).await?;
    let frame_len = {
        let body_len = u32::from_le_bytes(header[16..20].try_into().expect("4 bytes")) as usize;
        if body_len > MAX_BODY_LEN {
            return Err(super::DiscoveryError::Protocol(format!(
                "body length {body_len} exceeds the {MAX_BODY_LEN}-byte limit"
            )));
        }
        HEADER_LEN + body_len
    };
    let mut frame = Vec::with_capacity(frame_len);
    frame.extend_from_slice(&header);
    frame.resize(frame_len, 0);
    reader.read_exact(&mut frame[HEADER_LEN..]).await?;
    decode_message(magic, &frame)
}

fn encode_compact_size(out: &mut Vec<u8>, n: u64) {
    match n {
        0..=252 => out.push(n as u8),
        253..=65_535 => {
            out.push(0xfd);
            out.extend_from_slice(&(n as u16).to_le_bytes());
        }
        65_536..=4_294_967_295 => {
            out.push(0xfe);
            out.extend_from_slice(&(n as u32).to_le_bytes());
        }
        _ => {
            out.push(0xff);
            out.extend_from_slice(&n.to_le_bytes());
        }
    }
}

fn decode_compact_size(body: &[u8], offset: &mut usize) -> Result<u64, super::DiscoveryError> {
    let prefix = *body
        .get(*offset)
        .ok_or_else(|| super::DiscoveryError::Protocol("compact size past end of body".into()))?;
    *offset += 1;
    let width = match prefix {
        0..=252 => return Ok(prefix as u64),
        0xfd => 2usize,
        0xfe => 4,
        _ => 8,
    };
    let end = *offset + width;
    let bytes = body.get(*offset..end).ok_or_else(|| {
        super::DiscoveryError::Protocol("compact size truncated at end of body".into())
    })?;
    *offset = end;
    let mut buf = [0u8; 8];
    buf[..width].copy_from_slice(bytes);
    Ok(u64::from_le_bytes(buf))
}

fn encode_ip(ip: IpAddr) -> [u8; 16] {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        IpAddr::V6(v6) => v6.octets(),
    }
}

fn decode_ip(bytes: &[u8; 16]) -> IpAddr {
    let v6 = Ipv6Addr::from(*bytes);
    match v6.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(v6),
    }
}

/// The `version` we introduce ourselves with. `services` claims
/// [`NODE_NETWORK`] because a zero value gets us dropped by syncing zebra
/// nodes, and `addr_from` stays unspecified because we never listen.
pub fn our_version(peer: (IpAddr, u16), nonce: u64) -> VersionMessage {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0);
    VersionMessage {
        version: PROTOCOL_VERSION,
        services: NODE_NETWORK,
        timestamp,
        addr_recv: PeerAddr {
            services: 0,
            ip: peer.0,
            port: peer.1,
        },
        addr_from: PeerAddr {
            services: 0,
            ip: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            port: 0,
        },
        nonce,
        user_agent: USER_AGENT.to_string(),
        start_height: 0,
        relay: false,
    }
}

/// Serialize a `version` payload in zebra's canonical field order.
pub fn encode_version_body(v: &VersionMessage) -> Vec<u8> {
    let mut out = Vec::with_capacity(88 + v.user_agent.len());
    out.extend_from_slice(&v.version.to_le_bytes());
    out.extend_from_slice(&v.services.to_le_bytes());
    out.extend_from_slice(&v.timestamp.to_le_bytes());
    encode_peer_addr(&mut out, &v.addr_recv);
    encode_peer_addr(&mut out, &v.addr_from);
    out.extend_from_slice(&v.nonce.to_le_bytes());
    encode_compact_size(&mut out, v.user_agent.len() as u64);
    out.extend_from_slice(v.user_agent.as_bytes());
    out.extend_from_slice(&v.start_height.to_le_bytes());
    out.push(u8::from(v.relay));
    out
}

fn encode_peer_addr(out: &mut Vec<u8>, addr: &PeerAddr) {
    out.extend_from_slice(&addr.services.to_le_bytes());
    out.extend_from_slice(&encode_ip(addr.ip));
    out.extend_from_slice(&addr.port.to_be_bytes());
}

fn take_bytes<'a>(
    body: &'a [u8],
    offset: &mut usize,
    n: usize,
) -> Result<&'a [u8], super::DiscoveryError> {
    let end = *offset + n;
    let slice = body
        .get(*offset..end)
        .ok_or_else(|| super::DiscoveryError::Protocol("body truncated".into()))?;
    *offset = end;
    Ok(slice)
}

/// Parse a `version` payload. The trailing `relay` byte is the one optional
/// field: its absence reads as `true`, as in zebra.
pub fn decode_version_body(body: &[u8]) -> Result<VersionMessage, super::DiscoveryError> {
    const PEER_ADDR_LEN: usize = 8 + 16 + 2;
    let mut off = 0usize;
    let version = u32::from_le_bytes(take_bytes(body, &mut off, 4)?.try_into().expect("4 bytes"));
    let services = u64::from_le_bytes(take_bytes(body, &mut off, 8)?.try_into().expect("8 bytes"));
    let timestamp = i64::from_le_bytes(take_bytes(body, &mut off, 8)?.try_into().expect("8 bytes"));
    let addr_recv = decode_peer_addr(take_bytes(body, &mut off, PEER_ADDR_LEN)?)?;
    let addr_from = decode_peer_addr(take_bytes(body, &mut off, PEER_ADDR_LEN)?)?;
    let nonce = u64::from_le_bytes(take_bytes(body, &mut off, 8)?.try_into().expect("8 bytes"));
    let ua_len = decode_compact_size(body, &mut off)?;
    if ua_len > 256 {
        return Err(super::DiscoveryError::Protocol(
            "version body: user agent longer than zebra's 256-byte limit".into(),
        ));
    }
    let user_agent =
        String::from_utf8_lossy(take_bytes(body, &mut off, ua_len as usize)?).into_owned();
    let start_height =
        u32::from_le_bytes(take_bytes(body, &mut off, 4)?.try_into().expect("4 bytes"));
    let relay = match body.get(off) {
        Some(&raw) => raw != 0,
        None => true,
    };
    Ok(VersionMessage {
        version,
        services,
        timestamp,
        addr_recv,
        addr_from,
        nonce,
        user_agent,
        start_height,
        relay,
    })
}

fn decode_peer_addr(bytes: &[u8]) -> Result<PeerAddr, super::DiscoveryError> {
    let ip: [u8; 16] = bytes[8..24].try_into().expect("16 bytes");
    Ok(PeerAddr {
        services: u64::from_le_bytes(bytes[..8].try_into().expect("8 bytes")),
        ip: decode_ip(&ip),
        port: u16::from_be_bytes(bytes[24..26].try_into().expect("2 bytes")),
    })
}

/// Serialize an `addr` payload: compact-size count, then 30-byte entries.
pub fn encode_addr_body(entries: &[AddrEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + entries.len() * 30);
    encode_compact_size(&mut out, entries.len() as u64);
    for entry in entries {
        out.extend_from_slice(&entry.time.to_le_bytes());
        out.extend_from_slice(&entry.services.to_le_bytes());
        out.extend_from_slice(&encode_ip(entry.ip));
        out.extend_from_slice(&entry.port.to_be_bytes());
    }
    out
}

/// Parse an `addr` payload, rejecting counts over [`MAX_ADDRS`].
pub fn decode_addr_body(body: &[u8]) -> Result<Vec<AddrEntry>, super::DiscoveryError> {
    let mut off = 0usize;
    let count = decode_compact_size(body, &mut off)?;
    if count as usize > MAX_ADDRS {
        return Err(super::DiscoveryError::Protocol(format!(
            "addr message declares {count} entries, over the limit of {MAX_ADDRS}"
        )));
    }
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let entry_bytes = body
            .get(off..off + 30)
            .ok_or_else(|| super::DiscoveryError::Protocol("addr entry truncated".into()))?;
        off += 30;
        entries.push(AddrEntry {
            time: u32::from_le_bytes(entry_bytes[..4].try_into().expect("4 bytes")),
            services: u64::from_le_bytes(entry_bytes[4..12].try_into().expect("8 bytes")),
            ip: decode_ip(entry_bytes[12..28].try_into().expect("16 bytes")),
            port: u16::from_be_bytes(entry_bytes[28..30].try_into().expect("2 bytes")),
        });
    }
    Ok(entries)
}

/// A `ping` or `pong` body: the nonce, little-endian.
pub fn ping_body(nonce: u64) -> Vec<u8> {
    nonce.to_le_bytes().to_vec()
}

/// Whether the address could belong to a reachable peer: filters loopback,
/// private, link-local, multicast, documentation, benchmarking, and
/// carrier-NAT ranges, plus Tor v2 (OnionCat) addresses. IPv4-mapped IPv6 is
/// judged as its IPv4 self.
pub fn is_routable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 100 && (o[1] & 0xc0) == 64))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_routable(IpAddr::V4(v4));
            }
            let o = v6.octets();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (o[0] == 0xfe && (o[1] & 0xc0) == 0x80)
                || (o[0] & 0xfe) == 0xfc
                || (o[..4] == [0x20, 0x01, 0x0d, 0xb8])
                || (o[..6] == [0xfd, 0x87, 0xd8, 0x7e, 0xeb, 0x43]))
        }
    }
}

fn uri_host(raw: &str) -> Option<String> {
    let rest = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let host = rest.split(['/', '?']).next()?;
    if let Some(close) = host.rfind(']') {
        return host
            .strip_prefix('[')
            .map(|inner| inner[..close - 1].to_string());
    }
    if host.matches(':').count() == 1
        && let Some((bare, port)) = host.split_once(':')
        && !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
    {
        return Some(bare.to_string());
    }
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn random_nonce() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos() as u64)
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    hasher.finish()
}

async fn bootstrap_candidates(config: &super::DiscoveryConfig) -> Vec<(IpAddr, u16)> {
    let mut hosts = Vec::new();
    for uri in config
        .seed_uris
        .iter()
        .chain(config.dns_seeds.iter())
        .filter_map(|raw| uri_host(raw))
    {
        if !hosts.contains(&uri) {
            hosts.push(uri);
        }
    }

    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for host in hosts {
        let addresses: Vec<IpAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => match tokio::time::timeout(DNS_TIMEOUT, lookup_host(format!("{host}:0")))
                .await
            {
                Ok(Ok(addrs)) => addrs.map(|sa| sa.ip()).collect(),
                Ok(Err(e)) => {
                    tracing::debug!("discovery: cannot resolve seed {host}: {e}");
                    continue;
                }
                Err(_) => {
                    tracing::debug!("discovery: seed {host} took over {DNS_TIMEOUT:?} to resolve");
                    continue;
                }
            },
        };
        for ip in addresses {
            let ip = match ip {
                IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
                v4 => v4,
            };
            for &port in &config.p2p_ports {
                if seen.insert((ip, port)) {
                    candidates.push((ip, port));
                }
            }
        }
    }
    candidates.truncate(MAX_BOOTSTRAPS);
    candidates
}

async fn send_message<S: AsyncWrite + Unpin>(
    writer: &mut S,
    magic: [u8; 4],
    command: &str,
    body: &[u8],
    timeout: Duration,
) -> Result<(), super::DiscoveryError> {
    let frame = encode_message(magic, command, body)?;
    let write = async {
        writer.write_all(&frame).await?;
        writer.flush().await
    };
    tokio::time::timeout(timeout, write).await.map_err(|_| {
        super::DiscoveryError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("writing {command} timed out"),
        ))
    })??;
    Ok(())
}

async fn run_session(
    peer: (IpAddr, u16),
    magic: [u8; 4],
    config: &super::DiscoveryConfig,
) -> Result<Vec<(IpAddr, u16)>, super::DiscoveryError> {
    let addr = SocketAddr::new(peer.0, peer.1);
    let connect = tokio::time::timeout(config.timeout, TcpStream::connect(addr)).await;
    let mut stream = match connect {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return Err(super::DiscoveryError::Io(e)),
        Err(_) => {
            return Err(super::DiscoveryError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("connecting to {addr} timed out"),
            )));
        }
    };
    let _ = stream.set_nodelay(true);

    let handshake_window = config.timeout.min(Duration::from_secs(5));
    let deadline = Instant::now() + handshake_window;
    send_message(
        &mut stream,
        magic,
        "version",
        &encode_version_body(&our_version(peer, random_nonce())),
        config.timeout,
    )
    .await?;

    let mut remaining = deadline.saturating_duration_since(Instant::now());
    loop {
        if remaining.is_zero() {
            return Err(super::DiscoveryError::Protocol(format!(
                "handshake with {addr}: timed out waiting for version"
            )));
        }
        let message = match tokio::time::timeout(remaining, read_message(&mut stream, magic)).await
        {
            Ok(Ok(message)) => message,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(super::DiscoveryError::Protocol(format!(
                    "handshake with {addr}: timed out waiting for version"
                )));
            }
        };
        if message.is("version") {
            let theirs = decode_version_body(&message.body)?;
            tracing::debug!(
                "discovery: {addr} is {} {} (height {})",
                theirs.user_agent,
                theirs.version,
                theirs.start_height
            );
            break;
        }
        tracing::debug!(
            "discovery: {addr} sent {} before its version; skipping",
            String::from_utf8_lossy(message.command.as_slice())
        );
        remaining = deadline.saturating_duration_since(Instant::now());
    }

    send_message(&mut stream, magic, "verack", &[], config.timeout).await?;
    loop {
        remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(super::DiscoveryError::Protocol(format!(
                "handshake with {addr}: timed out waiting for verack"
            )));
        }
        let message = match tokio::time::timeout(remaining, read_message(&mut stream, magic)).await
        {
            Ok(Ok(message)) => message,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(super::DiscoveryError::Protocol(format!(
                    "handshake with {addr}: timed out waiting for verack"
                )));
            }
        };
        if message.is("verack") {
            break;
        }
    }

    send_message(&mut stream, magic, "getaddr", &[], config.timeout).await?;
    let mut peers: BTreeSet<(IpAddr, u16)> = BTreeSet::new();
    let window = config.timeout.min(MAX_EXCHANGE_WINDOW);
    let deadline = Instant::now() + window;
    let mut last_new_peer = Instant::now();
    loop {
        if peers.len() >= config.max_peers {
            break;
        }
        if !peers.is_empty() && last_new_peer.elapsed() > IDLE_WINDOW {
            break;
        }
        let slice = deadline.saturating_duration_since(Instant::now());
        if slice.is_zero() {
            break;
        }
        let message = tokio::time::timeout(slice, read_message(&mut stream, magic)).await;
        let message = match message {
            Ok(Ok(message)) => message,
            Ok(Err(e @ super::DiscoveryError::Protocol(_))) => {
                tracing::debug!("discovery: {addr} sent a bad message, hanging up: {e}");
                break;
            }
            Ok(Err(e)) => {
                tracing::debug!("discovery: {addr} closed the connection: {e}");
                break;
            }
            Err(_) => break,
        };
        if message.is("ping") {
            if message.body.len() == 8 {
                let nonce = u64::from_le_bytes(message.body[..8].try_into().expect("8 bytes"));
                if let Err(e) = send_message(
                    &mut stream,
                    magic,
                    "pong",
                    &ping_body(nonce),
                    config.timeout,
                )
                .await
                {
                    tracing::debug!("discovery: cannot answer {addr}'s ping: {e}");
                    break;
                }
            }
        } else if message.is("getaddr") {
            send_message(
                &mut stream,
                magic,
                "addr",
                &encode_addr_body(&[]),
                config.timeout,
            )
            .await
            .map_err(|e| {
                tracing::debug!("discovery: cannot answer {addr}'s getaddr: {e}");
                e
            })?;
        } else if message.is("addr") {
            match decode_addr_body(&message.body) {
                Ok(entries) => {
                    for entry in entries {
                        if entry.port != 0
                            && is_routable(entry.ip)
                            && peers.insert((entry.ip, entry.port))
                        {
                            last_new_peer = Instant::now();
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("discovery: {addr} sent a bad addr message: {e}");
                }
            }
        }
    }
    Ok(peers.into_iter().collect())
}

/// Dial a few bootstrap peers, handshake with each, and harvest their address
/// books. Returns the deduplicated `(ip, port)` candidates and how many peers
/// were seen in total; the IP is formatted as a string for the caller, whose
/// gRPC port sits [`super::DiscoveryConfig::grpc_port_offset`] above the
/// listener. A no-op success when the magic or `crawl_peers` is unset;
/// stops after the first batch with a successful handshake, since one node's
/// answer already samples its whole address book.
pub async fn peer_exchange(
    config: &super::DiscoveryConfig,
) -> Result<(Vec<(String, u16)>, usize), super::DiscoveryError> {
    let Some(magic) = config.magic else {
        return Ok((Vec::new(), 0));
    };
    if !config.crawl_peers {
        return Ok((Vec::new(), 0));
    }

    let candidates = bootstrap_candidates(config).await;
    if candidates.is_empty() {
        return Err(super::DiscoveryError::NoBootstrap);
    }

    let mut handshakes = 0usize;
    let mut peers: BTreeSet<(IpAddr, u16)> = BTreeSet::new();
    for batch in candidates.chunks(MAX_CONCURRENT_SESSIONS) {
        let mut sessions = JoinSet::new();
        for peer in batch {
            let peer = *peer;
            let config = config.clone();
            sessions.spawn(async move { run_session(peer, magic, &config).await });
        }
        while let Some(joined) = sessions.join_next().await {
            match joined {
                Ok(Ok(found)) => {
                    handshakes += 1;
                    peers.extend(found);
                }
                Ok(Err(e)) => {
                    tracing::debug!("discovery: bootstrap session failed: {e}");
                }
                Err(e) => {
                    tracing::warn!("discovery: bootstrap session panicked: {e}");
                }
            }
        }
        if handshakes > 0 {
            break;
        }
    }

    if handshakes == 0 {
        return Err(super::DiscoveryError::NoBootstrap);
    }
    if peers.is_empty() {
        return Err(super::DiscoveryError::NoPeers);
    }

    let peers_seen = peers.len();
    let listed = peers
        .into_iter()
        .take(config.max_peers)
        .map(|(ip, port)| (ip.to_string(), port))
        .collect();
    Ok((listed, peers_seen))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const MAGIC: [u8; 4] = [0x43, 0x6c, 0x54, 0x31];

    fn unhex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex digit"))
            .collect()
    }

    fn sample_version() -> VersionMessage {
        VersionMessage {
            version: PROTOCOL_VERSION,
            services: NODE_NETWORK,
            timestamp: 1_760_000_000,
            addr_recv: PeerAddr {
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(45, 63, 75, 9)),
                port: 8233,
            },
            addr_from: PeerAddr {
                services: 0,
                ip: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                port: 0,
            },
            nonce: 0x0123_4567_89ab_cdef,
            user_agent: USER_AGENT.to_string(),
            start_height: 0,
            relay: false,
        }
    }

    #[test]
    fn version_round_trips() {
        let body = encode_version_body(&sample_version());
        let decoded = decode_version_body(&body).expect("decodes");
        assert_eq!(decoded, sample_version());
    }

    #[test]
    fn version_relay_may_be_omitted_reading_true() {
        let mut body = encode_version_body(&sample_version());
        body.pop();
        let decoded = decode_version_body(&body).expect("decodes");
        assert!(decoded.relay, "zebra reads a missing relay as true");
    }

    #[test]
    fn version_frame_round_trips() {
        let frame = encode_message(MAGIC, "version", &encode_version_body(&sample_version()))
            .expect("encodes");
        assert_eq!(&frame[..4], &MAGIC);
        assert_eq!(&frame[4..16], b"version\0\0\0\0\0");
        let message = decode_message(MAGIC, &frame).expect("decodes");
        assert!(message.is("version"));
        assert!(!message.is("verack"));
        assert_eq!(
            decode_version_body(&message.body).unwrap(),
            sample_version()
        );
    }

    #[test]
    fn verack_and_getaddr_have_empty_bodies() {
        for command in ["verack", "getaddr"] {
            let frame = encode_message(MAGIC, command, &[]).expect("encodes");
            assert_eq!(frame[4..16], command_bytes(command).unwrap());
            let message = decode_message(MAGIC, &frame).expect("decodes");
            assert!(message.is(command));
            assert!(message.body.is_empty());
        }
    }

    #[test]
    fn ping_pong_bodies_are_the_nonce() {
        for nonce in [0u64, 0xdead_beef_cafe_f00d] {
            let body = ping_body(nonce);
            assert_eq!(body.len(), 8);
            assert_eq!(u64::from_le_bytes(body[..8].try_into().unwrap()), nonce);
        }
    }

    #[test]
    fn compact_size_round_trips_minimally() {
        for (n, encoded_len) in [
            (0u64, 1),
            (252, 1),
            (253, 3),
            (65_535, 3),
            (65_536, 5),
            (u32::MAX as u64, 5),
            (u32::MAX as u64 + 1, 9),
        ] {
            let mut out = Vec::new();
            encode_compact_size(&mut out, n);
            assert_eq!(out.len(), encoded_len, "minimal encoding of {n}");
            let mut off = 0;
            assert_eq!(decode_compact_size(&out, &mut off).unwrap(), n);
            assert_eq!(off, encoded_len);
        }
    }

    #[test]
    fn decodes_the_golden_addr_v1_two_entry_vector() {
        let hex = "02\
                   61bc6649 0000000000000000 00000000000000000000000000000001 0000\
                   79627683 0100000000000000 00000000000000000000000000000001 00f1";
        let body = unhex(&hex.replace(' ', ""));
        let entries = decode_addr_body(&body).expect("decodes");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].time, 1_231_469_665);
        assert_eq!(entries[0].services, 0);
        assert_eq!(entries[0].ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(entries[0].port, 0);
        assert_eq!(entries[1].time, 2_205_573_753);
        assert_eq!(entries[1].services, NODE_NETWORK);
        assert_eq!(entries[1].ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(entries[1].port, 241);
    }

    #[test]
    fn decodes_the_golden_addr_v1_ipv4_mapped_vector() {
        let hex = "01\
                   61bc6649 0100000000000000 00000000000000000000ffff 7f000001 0000";
        let body = unhex(&hex.replace(' ', ""));
        let entries = decode_addr_body(&body).expect("decodes");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].time, 1_231_469_665);
        assert_eq!(entries[0].services, NODE_NETWORK);
        assert_eq!(entries[0].ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(entries[0].port, 0);
    }

    #[test]
    fn decodes_the_golden_empty_addr_vector() {
        assert!(decode_addr_body(&unhex("00")).expect("decodes").is_empty());
    }

    #[test]
    fn addr_round_trips_through_our_own_encoder() {
        let entries = vec![
            AddrEntry {
                time: 1,
                services: NODE_NETWORK,
                ip: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                port: 8233,
            },
            AddrEntry {
                time: u32::MAX,
                services: 0,
                ip: IpAddr::V6("2606:4700::1111".parse().unwrap()),
                port: 18_233,
            },
        ];
        assert_eq!(
            decode_addr_body(&encode_addr_body(&entries)).unwrap(),
            entries
        );
    }

    #[test]
    fn an_addr_count_over_the_limit_is_rejected() {
        let mut body = Vec::new();
        encode_compact_size(&mut body, (MAX_ADDRS + 1) as u64);
        let err = decode_addr_body(&body).unwrap_err();
        assert!(err.to_string().contains("over the limit"));
    }

    #[test]
    fn a_checksum_failure_is_rejected() {
        let mut frame = encode_message(MAGIC, "verack", &[]).expect("encodes");
        let last = frame.len() - 1;
        frame[last] ^= 0xff;
        let err = decode_message(MAGIC, &frame).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
    }

    #[test]
    fn a_magic_mismatch_is_rejected() {
        let frame = encode_message([1, 2, 3, 4], "verack", &[]).expect("encodes");
        let err = decode_message(MAGIC, &frame).unwrap_err();
        assert!(err.to_string().contains("magic mismatch"));
    }

    #[test]
    fn an_oversized_body_is_rejected_before_allocating() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&MAGIC);
        frame.extend_from_slice(b"version\0\0\0\0\0");
        frame.extend_from_slice(&(MAX_BODY_LEN as u32 + 1).to_le_bytes());
        frame.extend_from_slice(&[0; 4]);
        let err = decode_message(MAGIC, &frame).unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    #[test]
    fn declared_and_actual_body_lengths_must_agree() {
        let mut frame = encode_message(MAGIC, "verack", &[]).expect("encodes");
        frame.push(0);
        let err = decode_message(MAGIC, &frame).unwrap_err();
        assert!(err.to_string().contains("does not match frame size"));
    }

    #[test]
    fn the_addr_filter_keeps_only_probeable_peers() {
        let entries = vec![
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6("2606:4700::1111".parse().unwrap()),
                port: 18_233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(172, 16, 0, 9)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6("fd00::1".parse().unwrap()),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6("fe80::1".parse().unwrap()),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6("ff02::1".parse().unwrap()),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6("2001:db8::1".parse().unwrap()),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V6("fd87:d87e:eb43:1234::1".parse().unwrap()),
                port: 8233,
            },
            AddrEntry {
                time: 0,
                services: 0,
                ip: IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)),
                port: 0,
            },
        ];
        let kept: BTreeSet<(IpAddr, u16)> = entries
            .iter()
            .filter(|e| e.port != 0 && is_routable(e.ip))
            .map(|e| (e.ip, e.port))
            .collect();
        assert_eq!(
            kept,
            BTreeSet::from([
                (IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 8233),
                (IpAddr::V6("2606:4700::1111".parse().unwrap()), 18_233),
            ])
        );
    }

    #[test]
    fn seed_hosts_are_bare_hostnames() {
        assert_eq!(
            uri_host("http://45.63.75.9:18234").as_deref(),
            Some("45.63.75.9")
        );
        assert_eq!(
            uri_host("https://seed.example/path").as_deref(),
            Some("seed.example")
        );
        assert_eq!(
            uri_host("dnsseed.z.cash:8233").as_deref(),
            Some("dnsseed.z.cash")
        );
        assert_eq!(uri_host("10.0.0.1").as_deref(), Some("10.0.0.1"));
        assert_eq!(
            uri_host("[2001:db8::1]:8233").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(uri_host("2001:db8::1").as_deref(), Some("2001:db8::1"));
        assert_eq!(uri_host(""), None);
    }

    #[tokio::test]
    async fn crawls_a_local_zebra_lookalike() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let version = read_message(&mut sock, MAGIC).await.unwrap();
            assert!(version.is("version"));
            let theirs = decode_version_body(&version.body).unwrap();
            assert_eq!(theirs.version, PROTOCOL_VERSION);
            assert_eq!(theirs.user_agent, USER_AGENT);
            assert_eq!(theirs.start_height, 0);
            let zebra_version = VersionMessage {
                version: PROTOCOL_VERSION,
                services: NODE_NETWORK,
                timestamp: 1_760_000_000,
                addr_recv: PeerAddr {
                    services: 0,
                    ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    port: theirs.addr_recv.port,
                },
                addr_from: PeerAddr {
                    services: NODE_NETWORK,
                    ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    port,
                },
                nonce: 42,
                user_agent: "/Zebra:2.0.0/".into(),
                start_height: 1_000,
                relay: true,
            };
            sock.write_all(
                &encode_message(MAGIC, "version", &encode_version_body(&zebra_version)).unwrap(),
            )
            .await
            .unwrap();
            sock.write_all(&encode_message(MAGIC, "verack", &[]).unwrap())
                .await
                .unwrap();
            let verack = read_message(&mut sock, MAGIC).await.unwrap();
            assert!(verack.is("verack"));
            let getaddr = read_message(&mut sock, MAGIC).await.unwrap();
            assert!(getaddr.is("getaddr"));
            let canned = [
                AddrEntry {
                    time: 1,
                    services: NODE_NETWORK,
                    ip: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                    port: 8233,
                },
                AddrEntry {
                    time: 1,
                    services: NODE_NETWORK,
                    ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    port: 8233,
                },
            ];
            sock.write_all(&encode_message(MAGIC, "addr", &encode_addr_body(&canned)).unwrap())
                .await
                .unwrap();
            sock.write_all(&encode_message(MAGIC, "ping", &ping_body(0x1234_5678)).unwrap())
                .await
                .unwrap();
            let pong = tokio::time::timeout(Duration::from_secs(5), read_message(&mut sock, MAGIC))
                .await
                .unwrap()
                .unwrap();
            assert!(pong.is("pong"));
            assert_eq!(pong.body, ping_body(0x1234_5678));
        });

        let config = super::super::DiscoveryConfig {
            seed_uris: vec![format!("http://127.0.0.1:{port}")],
            dns_seeds: Vec::new(),
            p2p_ports: vec![port],
            grpc_port_offset: 10_001,
            magic: Some(MAGIC),
            timeout: Duration::from_secs(2),
            concurrency: 1,
            max_peers: 64,
            crawl_peers: true,
        };
        let (peers, peers_seen) = peer_exchange(&config)
            .await
            .expect("the peer_exchange succeeds");
        server.await.unwrap();
        assert_eq!(peers, vec![("8.8.8.8".to_string(), 8233)]);
        assert_eq!(peers_seen, 1);
    }

    #[tokio::test]
    async fn crawl_without_magic_reports_no_peers() {
        let config = super::super::DiscoveryConfig {
            magic: None,
            crawl_peers: false,
            ..super::super::DiscoveryConfig::crosslink_stable()
        };
        let (peers, peers_seen) = peer_exchange(&config).await.expect("a no-op, not an error");
        assert!(peers.is_empty());
        assert_eq!(peers_seen, 0);
    }
}
