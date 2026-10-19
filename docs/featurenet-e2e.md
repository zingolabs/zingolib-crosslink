# Featurenet end-to-end run

Running log of `zingo-cli` against the live Crosslink featurenet, the fixes it needed, and what the
node does not provide. Companion to [crosslink-featurenet.md](crosslink-featurenet.md).

Setup used throughout:

```text
zingo-cli --waitsync --chain testnet --server http://45.63.75.9:18234 --data-dir <dir> <command>
```

All three public peers (`45.63.75.9`, `70.34.220.94`, `64.177.44.176`) expose the node's embedded
lightwalletd gRPC server on port 18234 (`GetLightdInfo`: vendor `Crosslink zebrad`, version 6.2.3).

## Node facts established before the first sync

Checked with `grpcurl` against the pinned proto files (`zcash_client_backend/proto` at `0735e958`).

| Query | Result |
|---|---|
| `GetLatestBlock` | height 62149, unchanged since 2026-10-18 22:18 UTC (no block mined for 22 h; the recent block interval was hours) |
| `GetLightdInfo.consensusBranchId` | `37a5165b` (NU6.3) at height 62149, although the pinned config activates `FeaturenetUpgrade1` (`46545531`) at 55296; the handler computes the id from the tip, so the public nodes run a build whose schedule differs from the pin |
| `GetBlockRange(1..62149)` | 62149 blocks, 5880 transactions with `ironwoodActions`, zero Orchard actions, zero Sapling outputs, no `chainMetadata` on any block |
| `GetTreeState(62149)` | `saplingTree` and `orchardTree` are `000000` (empty trees), `ironwoodTree` absent (empty string) |
| `GetSubtreeRoots(shieldedProtocol = 2)` | `InvalidArgument: unknown shielded protocol` |
| `GetRoster` | returns roster bytes |

The last three are node gaps, present at the pin and still at `crosslink_monolith` `dev` head
(`24df728d`, 2026-10-19): `lightwalletd.rs` sets `ironwood_tree: String::new()` with the comment
"Crosslink does not build Ironwood bundles yet", serves subtree roots only for protocols 0 and 1, and
builds compact blocks with `chain_metadata: None`. The state itself has the Ironwood tree
(`ReadRequest::IronwoodSubtrees`, `add_ironwood_tree.rs`); only the gRPC layer omits it.

Consequence for a light wallet: it cannot obtain the Ironwood frontier for any height, so it cannot
witness Ironwood notes unless it scans every Ironwood action from the pool's activation (height 1).

## Flow 1: fresh wallet, sync, balance, addresses

### Attempt 1 (unmodified branch)

```text
error: Sync error. server error. server request failed. code: 'Client specified an invalid argument', message: "unknown shielded protocol"
```

`update_subtree_roots` requests Ironwood subtree roots (protocol 2) and the node rejects them.
The balance output also had no Ironwood lines.

### Fix (pepper-sync, zingolib, zingo-cli)

- `client::get_subtree_roots` tolerates `InvalidArgument`/`Unimplemented` for the Ironwood protocol
  only, logs a warning and returns no roots.
- `client::get_tree_sizes` replaces the three places that read tree sizes from the server frontier.
  When the tree state carries an empty `ironwood_tree` at a height where NU6.3 is active, it counts
  the Ironwood actions in `GetBlockRangeNullifiers(activation..=height)` instead. A served empty
  tree is `000000`, never an empty string, so the detection is unambiguous.
- `add_initial_frontier` fails with `ServerError::IronwoodTreeStateUnavailable` instead of inserting
  an empty Ironwood frontier for a birthday above activation, which would silently mis-position
  every note.
- `LightClient::fresh_wallet_birthday` probes the server; without Ironwood tree state the fresh
  wallet's birthday is the Ironwood activation height. `zingo-cli` prints when this happens. A
  restore from seed must pass `--birthday 1` on this network.
- `AccountBalance` gains confirmed/unconfirmed/total Ironwood balances.

### Attempt 2

Fresh wallet, birthday 1, full scan of 62149 blocks: 3 min 4 s wall clock, dominated by the
block stream. `balance` prints all four pools at 0, `addresses` lists the account 0 unified address
(Orchard receiver only), `height` matches the tip.

## Flow 2: faucet

`request_faucet` returned `{"request": "50000000"}`. The node put a 2-action Ironwood transaction in
its mempool (`GetMempoolTx` shows it). The wallet picked it up from the mempool stream:
`unconfirmed_ironwood_balance: 50_000_000`.

`transactions` showed the transaction with `value: 0` and no note list: the summaries, the
transaction-kind classifier, `total_value_received`/`total_value_sent` and the output-query sums
enumerated Sapling and Orchard only. Fixed in pepper-sync (`WalletTransaction` totals) and zingolib
(`TransactionSummary` gains `ironwood_notes`/`outgoing_ironwood_notes`, value transfers report
`pool_received: ironwood`, `transaction_kind` counts Ironwood spends and outgoing notes).

Confirmation is blocked on block production: no block since 2026-10-18 22:18 UTC. A local `zebrad`
built from the pinned commit (copied out of the cargo git checkout, run with a config kept under
`/tmp`, internal miner on) was started to mine; see below.
