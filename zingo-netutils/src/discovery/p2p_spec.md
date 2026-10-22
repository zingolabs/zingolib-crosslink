# Zcash P2P wire protocol as implemented in ShieldedLabs/crosslink_monolith (`main`)

Source of truth: the zebra fork under `zebra-crosslink/` in
<https://github.com/ShieldedLabs/crosslink_monolith> (branch `main`), files
retrieved 2026-10-21. Every fact cites the file (and const/fn) in the monolith.
All facts below are from the monolith unless explicitly labeled
"upstream zebra, may differ".

## 1. Network magic bytes

Magic lives in `zebra_chain::parameters`, not in `zebra-network`:

| Network | Magic (wire order) | Hex | Source |
|---|---|---|---|
| Mainnet | `24 e9 27 64` | 0x24e92764 | `zebra-crosslink/zebra-chain/src/parameters/constants.rs`, `magics::MAINNET` |
| Default public Testnet | `fa 1a f9 bf` | 0xfa1af9bf | same file, `magics::TESTNET`; also `parameters/network/testnet.rs` `ParametersBuilder::default()` (`network_magic: magics::TESTNET`) |
| Regtest | `aa e8 3f 5f` | 0xaae83f5f | same file, `magics::REGTEST` |
| **Crosslink testnet (zebrad's default network on this monolith)** | `43 6c 54 31` ("ClT1") | 0x436c5431 | `zebra-crosslink/zebrad/src/config.rs`, `ZebradConfig::crosslink_default()` (`Magic([b'C', b'l', b'T', b'1'])`) |

- There is **no hardcoded per-crosslink-network magic table**. Crosslink
  networks (featurenet/nightly) are "configured testnets": magic comes from the
  config file field `network.network.network_magic: [u8; 4]`
  (`zebra-crosslink/zebra-network/src/config.rs`, `DTestnetParameters`).
  `ParametersBuilder::with_network_magic` rejects only `magics::MAINNET` and
  `magics::REGTEST` (`parameters/network/testnet.rs`).
- **Crosslink does NOT reuse the zebra testnet magic**: `ClT1` (43 6c 54 31)
  ≠ `fa 1a f9 bf`. With no config file, zebrad substitutes the
  `crosslink_default()` network (magic ClT1) whenever the raw key
  `network.network` is absent or `"Testnet"` (`zebrad/src/config.rs`,
  `ZebradConfig::load`). Cache dir suffix confirms the deployment name:
  `crosslink_featurenet_20260929_staking3d` (`zebrad/src/config.rs`, `load`).
- A genesis PoW file named `ClT0-genesis.pow` exists at
  `zebra-crosslink/ClT0-genesis.pow` (binary; loaded as the genesis block in
  `zebrad/src/commands/start.rs`, `new_network_task_handle`). Its hash must
  equal the configured genesis `05a60a92d99d85997cce3b87616c089f6124d7342af37106edc76126334a2c38`
  (default Testnet genesis, `parameters/network/testnet.rs`,
  `TESTNET_GENESIS_HASH`; also set explicitly in `crosslink_default()`).
  The file name suggests an earlier `ClT0` magic; no `ClT0` magic constant is
  used in code — NOT FOUND in monolith beyond the file name.
- Wire order: magic bytes are written as stored, `dst.write_all(&network.magic().0[..])`
  (`zebra-crosslink/zebra-network/src/protocol/external/codec.rs`, `encode`).

## 2. Default ports

| What | Port | Source |
|---|---|---|
| P2P default port, Mainnet | 8233 | `zebra-crosslink/zebra-chain/src/parameters/network.rs`, `Network::default_port()` |
| P2P default port, **every** Testnet-kind network (default Testnet, configured Testnet incl. Crosslink, Regtest) | 18233 | same fn: `Network::Testnet(_params) => 18233` |
| P2P default listener when no config | `[::]:8233` (fixed, regardless of network) | `zebra-crosslink/zebra-network/src/config.rs`, `impl Default for Config` (`listen_addr: "[::]:8233"`). A portless `listen_addr` gets `network.default_port()` appended in `Deserialize for Config`. |
| Crosslink initial peers (what deployed nodes actually listen on) | `45.63.75.9:8233`, `70.34.220.94:8233`, `64.177.44.176:8233` | `zebrad/src/config.rs`, `crosslink_default().network.initial_testnet_peers` |
| zebrad JSON-RPC (crosslink default) | `127.0.0.1:8232` | `zebrad/src/config.rs`, `crosslink_default().rpc.listen_addr` |
| **Embedded lightwalletd / CompactTxStreamer gRPC** (served by zebrad itself, `zebra-crosslink/zebrad/src/lightwalletd.rs`, `lightwalletd_spawn(ctx, port, ready_port)`) | **p2p listen port + 10001** → 19234 with the default 8233 listener; readiness/legacy JSON-RPC probe on listen port + 10000 (18233) | `zebra-crosslink/zebrad/src/commands/start.rs`, `lightwalletd_thread_handle` block: `let lwd_port = zebra_port_base + 10001;` and `lightwalletd_spawn(lwd_ctx, lwd_port, zebra_port_base + 10000)`; the in-process wallet reads the port from `wallet::wallet_main_lightwalletd_port` (`wallet/src/lib.rs`) |
| gRPC plaintext | h2c on `[::]:port` and `0.0.0.0:port`; PATH prefix `/cash.z.wallet.sdk.rpc.CompactTxStreamer/` | `zebrad/src/lightwalletd.rs`, `lightwalletd_spawn`, `PATH_PREFIX` |
| A literal default `18234` | **NOT FOUND in monolith** | 18234 = listen 8233 + 10001 only if base were 8232... the derived default with `[::]:8233` is **19234**. If a deployment shows 18234, its p2p listen port is 8232. The wallet's legacy readiness probe posts to `http://localhost:18232` (`wallet/src/lib.rs`, `wait_for_lightwalletd`), which equals `8232 + 10000`. |
| Standalone lightwalletd container (docker) | gRPC 9067, HTTP 9068 | `zebra-crosslink/docker/docker-compose.lwd.yml` |

## 3. DNS seeds

Defaults in `zebra-crosslink/zebra-network/src/config.rs`, `impl Default for Config`:

| Network | Seeds (exactly as written) |
|---|---|
| Mainnet | `dnsseed.str4d.xyz:8233`, `dnsseed.z.cash:8233`, `mainnet.seeder.shieldedinfra.net:8233`, `mainnet.seeder.zfnd.org:8233`, `seeder.zec.rocks:8233` |
| Testnet (default public) | `dnsseed.testnet.z.cash:18233`, `seeder.testnet.zec.rocks:18233`, `testnet.seeder.zfnd.org:18233` |
| Crosslink | **NO DNS seeds.** Fixed IP peers instead (see §2); set via `crosslink_default()` and overridable via `initial_testnet_peers`. `initial_peer_hostnames()` maps any `Network::Testnet(..)` to `initial_testnet_peers` (`config.rs`). Configs with non-default testnet params that still name the default seeds are rejected (`build_configured_testnet`, "cannot use default initials peers with incompatible testnet"). |

Resolution: DNS with 5s timeout, retried until at least one peer resolves
(`config.rs`, `resolve_peers`/`resolve_host`, `constants::DNS_LOOKUP_TIMEOUT`
in `zebra-crosslink/zebra-network/src/constants.rs`).

## 4. Handshake

Implemented in `zebra-crosslink/zebra-network/src/peer/handshake.rs`
(`negotiate_version`) + `protocol/external/codec.rs`. Zebra **sends its
`version` first on both inbound and outbound** connections, then waits for the
peer's `version`, then sends `verack` and waits for `verack`. Non-`version`
messages arriving during the version wait are skipped; same for `verack`.

Sequence a dialing client sees: `version(zebra)` → (client sends `version`)
→ `verack(zebra)` → (client sends `verack`). Sending the client `version`
immediately on connect also works; zebra just reads it from the stream.

`version` payload, exact field order/encoding (`codec.rs`, `write_body` /
`read_version`; struct `VersionMessage` in `protocol/external/message.rs`):

| # | Field | Encoding |
|---|---|---|
| 1 | version | u32 LE (`Version(pub u32)`, `types.rs`) |
| 2 | services | u64 LE; decoded with `PeerServices::from_bits_truncate` (unknown bits dropped). Only `NODE_NETWORK = 1` defined (`types.rs`, `bitflags PeerServices`) |
| 3 | timestamp | i64 LE (unix seconds; must be a valid DateTime, `read_version`) |
| 4 | addr_recv | services u64 LE + 16-byte IPv6 (IPv4 as ::ffff:mapped) + port u16 **BE** (`protocol/external/addr/in_version.rs`, `AddrInVersion`) |
| 5 | addr_from | same as addr_recv |
| 6 | nonce | u64 LE (random per message/connection, `types.rs` `Nonce::default`) |
| 7 | user_agent | CompactSize byte-count + bytes; ≤ 256 (`message.rs` `MAX_USER_AGENT_LENGTH`) |
| 8 | start_height | u32 LE |
| 9 | relay | u8, 0/1; **optional**: missing (EOF) ⇒ true (`codec.rs` `read_version`) |

What zebra puts in its own version (handshake.rs): `version` =
`CURRENT_NETWORK_PROTOCOL_VERSION` = **170_165** ("FeaturenetUpgrade1",
`constants.rs`); `address_recv.services` hard-coded `NODE_NETWORK`;
`address_from` = `external_addr` or `listen_addr`; timestamp truncated to
5-minute grid; user agent from zebrad = `format!("/Zebra:{release_version}/")`
(`zebrad/src/application.rs`, `user_agent`).

Zebra's checks on OUR version (handshake.rs):
- `remote.version >= min_version` where min =
  `Version::min_remote_for_height` (`types.rs`). On the ClT1 crosslink net
  (custom testnet, upgrades < FeaturenetUpgrade1): **170_160** minimum
  (floor 170_150 from `INITIAL_MIN_NETWORK_PROTOCOL_VERSION` Testnet/Nu6_2,
  `constants.rs`). Advertise 170_160..=170_165; 170_165 safest.
- nonce must not collide with zebra's own nonce set (self-connection ⇒ reject).
- Outbound-to-zebra peers while zebra is syncing must advertise
  `NODE_NETWORK` in `services` (handshake.rs `MissingRequiredServices`).
- Version user_agent ≤ 256 enforced at codec level.
- Whole handshake must finish within `HANDSHAKE_TIMEOUT` = 3 s
  (`constants.rs`); per-message body limit pre-handshake is 1024 bytes
  (`codec.rs`, `MAX_HANDSHAKE_BODY_LEN`).

After the handshake zebra sends, unprompted (all in `handshake.rs`/
`peer/`/`peer_set/candidate_set.rs`):
- `ping` every 59 s (`constants.rs` `HEARTBEAT_INTERVAL`, first one 59 s after
  handshake; connection killed if no pong within another 59 s).
  **Minimal client: reply `pong` with the same u64 nonce** (`codec.rs`).
- `getaddr` when its crawler runs (every 61 s default, min 31 s between
  crawls, fanout 1, 8 s response timeout — `constants.rs`
  `DEFAULT_CRAWL_NEW_PEER_INTERVAL`, `MIN_PEER_GET_ADDR_INTERVAL`,
  `GET_ADDR_FANOUT`, `PEER_GET_ADDR_TIMEOUT`). **Minimal client: answer with
  an `addr` message; an empty list (`0x00`) is valid.**
- Unsolicited `inv` (new block/tx gossip), `getheaders`/`getdata` from syncing
  peers, `reject` never sent. No sentry or separate "localaddr" messages
  exist — our listener address is only carried in `version.addr_from` /
  `external_addr`. **No other unprompted message types found.**
- Unknown commands WE send are decoded, logged, and ignored without
  disconnect (`codec.rs`, `_ =>` arm: "unknown message command from peer").

## 5. `getaddr` / `addr` exchange

- Message names on the wire: `getaddr` (empty payload), `addr`.
  Commands as bytes: `b"getaddr\0\0\0\0\0"`, `b"addr\0\0\0\0\0\0\0\0"`
  (`codec.rs`, `encode`/`decode` match arms).
- `addr` entry layout (`protocol/external/addr/v1.rs`, `AddrV1::zcash_serialize`):
  `time` u32 LE (unix seconds, DateTime32) + `services` u64 LE + 16-byte
  IPv6 (IPv4 ⇒ IPv4-mapped) + `port` u16 **BE** = 30 bytes/entry. Prefixed by
  CompactSize entry count.
- `addrv2` (ZIP-155): zebra **parses** `addrv2` on receive
  (`codec.rs` `read_addrv2`, command `b"addrv2\0\0\0\0\0\0"`) but **never
  sends it** ("Zebra always sends `addr` messages", `codec.rs` `Addr` arm).
  Entry: time u32 LE, services CompactSize, network id u8 (1=IPv4, 2=IPv6,
  4=TorV3; unknown ids' entries dropped), addr len CompactSize + bytes, port
  u16 BE (`addr/v2.rs` semantics per `tests/vectors.rs`; some address types
  silently filtered, possibly leaving an empty list — not an error).
- Size limits: **max 1000 entries** per `addr`/`addrv2`
  (`constants.rs` `MAX_ADDRS_IN_MESSAGE = 1000`; enforced in `read_addr`/
  `read_addrv2` and `TrustedPreallocate` in `addr/v1.rs`).
- Zebra's answers to our `getaddr`: a random sample of its address book of
  size ≈ `book.len() / ADDR_RESPONSE_LIMIT_DENOMINATOR` (temporarily 2,
  `constants.rs`), capped at `PEER_ADDR_RESPONSE_LIMIT` = 37
  (`constants.rs`). Timestamps in received addrs are skew-corrected or the
  whole batch is dropped if any is in the future beyond correction
  (`candidate_set.rs`, `limit_last_seen_times`).
- Timing rules: zebra **requires the version/verack handshake to complete
  first** — pre-handshake messages are consumed by the handshake state
  machine (only version/verack accepted, others skipped then the handshake
  waits forever / times out). After handshake, zebra answers `getaddr` from
  any connected peer, including short-lived inbound ones; there is no
  minimum connection age. Zebra itself only *asks* `getaddr` of peers in its
  peer set on the crawler schedule above.

## 6. Framing

(`codec.rs`, `encode`/`decode`, `HEADER_LEN = 24`)

```
offset  size  field
0       4     magic (network bytes, as stored)
4       12    command: ASCII, zero-padded to 12 (e.g. "version\0\0\0\0\0")
16      4     body length, u32 LE
20      4     checksum: first 4 bytes of SHA256(SHA256(body)), u32 LE slot
24      ..    body
```

- Checksum computed over the body only (`sha256d::Checksum::from`,
  `codec.rs`); compared on receive ("supplied message checksum does not
  match"). Magic mismatch ⇒ parse error (disconnect).
- Min body size 0 (verack/getaddr/mempool/filterclear). Max body:
  **1024 bytes before the handshake completes**
  (`MAX_HANDSHAKE_BODY_LEN`), raised to
  `zebra_chain::serialization::MAX_PROTOCOL_MESSAGE_LEN` after it
  (`reconfigure_full_body_len`) — a fork-specific change; upstream zebra
  applies the 2 MiB limit from the start (upstream zebra, may differ).
- Command byte order: literal command bytes first, then NUL padding. Decoding
  matches the exact 12 bytes (`b"verack\0\0\0\0\0\0"` etc.); trailing garbage
  ⇒ unknown command ⇒ message skipped, connection kept.
- `version` encoding: only the canonical field order above is implemented.
  Tolerances: unknown service bits masked; trailing bytes after any message
  are allowed and logged ("extra data after decoding message"); `relay` may
  be omitted. A separate "obsolete version encoding" (as in zcashd history)
  is NOT FOUND in monolith — zebra here requires every field through
  `start_height` and only `relay` is optional (`codec.rs` `read_version`
  doc comment: "zcashd only requires fields up to `address_recv`…").

## 7. Golden test vectors (payload bodies, from `zebra-crosslink/zebra-network/src/protocol/external/tests/vectors.rs` + `zebra-crosslink/zebra-test/src/network_addr.rs`)

These are body bytes zebra decodes successfully via `Codec::read_addr` /
`read_addrv2` (no 24-byte header). To test a full decoder, frame them with
any magic + computed checksum, or feed the body directly.

**`addr` v1, 2 entries** (`ADDR_V1_IP_VECTORS[0]`):
```
02
61bc6649 0000000000000000 00000000000000000000000000000001 0000
79627683 0100000000000000 00000000000000000000000000000001 00f1
```
hex string:
`0261bc6649000000000000000000000000000000000000000100007962768301000000000000000000000000000000000000000100f1`
Expected: `[::1]:0` services=0 seen 2009-01-09T02:54:25Z; `[::1]:241`
services=NODE_NETWORK seen 2039-11-22T11:22:33Z.

**`addr` v1, IPv4-mapped** (`ADDR_V1_IP_VECTORS[3]`):
```
01
61bc6649 0100000000000000 00000000000000000000ffff 7f000001 0000
```
hex: `0161bc66490100000000000000000000000000ffff7f0000010000`
Expected: 127.0.0.1:0, NODE_NETWORK.

**`addrv2`, 3 entries (3rd is TorV3 ⇒ dropped, result 2 addrs)**
(`ADDR_V2_IP_VECTORS[0]`):
```
03
61bc6649 00 02 10 00000000000000000000000000000001 0000
79627683 01 02 10 00000000000000000000000000000001 00f1
79627683 01 04 20 53cd5648488c4707914182655b7664034e09e66f7e8cbf1084e654eb56c5bd88 235a
```
**`addr` empty list** (`ADDR_V1_EMPTY_VECTORS[0]`): `00`
**`ping`/`pong` body**: u64 LE nonce only; **`verack` body**: empty
(`codec.rs` `read_ping`/`read_pong`/`read_verack`).
A full framed zebra-encoded message (header+body hex) is NOT FOUND in
monolith test vectors; vectors.rs only exercises bodies.

## 8. Quick client recipe (crosslink ClT1 net)

1. TCP dial one of `45.63.75.9:8233`, `70.34.220.94:8233`,
   `64.177.44.176:8233` (or any discovered peer; port from its `addr` entry).
2. Complete handshake within 3 s: send `version` (version=170_165,
   services=1, real timestamp, addr_recv = peer, addr_from = your addr or
   `[::]:0`-style unspecified, random nonce, short UA, height 0, relay 0),
   read zebra's `version`, read `verack`, send `verack`.
3. Reply `pong(nonce)` to every `ping`; answer every `getaddr` with `addr`
   (`00` if empty); ignore `inv`/others (they are decoded and skipped by
   zebra if you send nothing).
4. After ~1 RTT of handshake completion, send `getaddr`, collect `addr`
   (≤ 37 entries), validate entries (30-byte v1 layout), disconnect.
   Keep total lifetime short; zebra's request timeout is 20 s
   (`constants.rs` `REQUEST_TIMEOUT`).

Nothing found in the monolith makes a minimal discovery client infeasible.
