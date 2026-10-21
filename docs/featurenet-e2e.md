# Featurenet end-to-end run

Running log of `zingo-cli` against the live Crosslink featurenet, the fixes it needed, and what the
node does not provide. Companion to [crosslink-featurenet.md](crosslink-featurenet.md).

Setup used throughout:

```text
zingo-cli --waitsync --chain testnet --indexer http://45.63.75.9:18234 --data-dir <dir> <command>
```

All three public peers (`45.63.75.9`, `70.34.220.94`, `64.177.44.176`) expose the node's embedded
lightwalletd gRPC indexer on port 18234 (`GetLightdInfo`: vendor `Crosslink zebrad`, version 6.2.3).

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
- `client::get_tree_sizes` replaces the three places that read tree sizes from the indexer frontier.
  When the tree state carries an empty `ironwood_tree` at a height where NU6.3 is active, it counts
  the Ironwood actions in `GetBlockRangeNullifiers(activation..=height)` instead. A served empty
  tree is `000000`, never an empty string, so the detection is unambiguous.
- `add_initial_frontier` fails with `ServerError::IronwoodTreeStateUnavailable` instead of inserting
  an empty Ironwood frontier for a birthday above activation, which would silently mis-position
  every note.
- `LightClient::fresh_wallet_birthday` probes the indexer; without Ironwood tree state the fresh
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
(`TransactionSummary` gains `ironwood_notes`/`outgoing_ironwood_notes`, wallet events report
`pool_received: ironwood`, `transaction_kind` counts Ironwood spends and outgoing notes).

Confirmation never came: no block since 2026-10-18 22:18 UTC on that node.

## The configured public peers are on a stale fork

A local `zebrad` built from the pinned commit (copied out of the cargo git checkout, run with a
config kept under `/tmp`, data under `/tmp/claude/zebrad-data`) was started to mine. Its crosslink
peer list showed about twenty peers at height 68 980+ advancing roughly one block a minute, and only
the three configured hosts at 62 149. Probing the IPs the node discovered found three more nodes
with port 18234 open:

| Host | `blockHeight` | `consensusBranchId` |
|---|---|---|
| `45.63.75.9`, `70.34.220.94`, `64.177.44.176` (configured peers) | 62149 | `37a5165b` (NU6.3) |
| `95.111.225.120` | 39680 | `37a5165b` |
| `5.78.157.36` | 61109 | `46545531` (`FeaturenetUpgrade1`) |
| `88.198.67.189` | 68990 and rising | `46545531` |

So the hosts hard-coded in the node's `crosslink_default()` run a build without
`FeaturenetUpgrade1` and stopped at 62149 (shielded activity on their chain ends at height 59412),
while the live network has activated the upgrade at 55296 and sits near 69 000. This is a node and
deployment problem, not a wallet one. Everything below uses `http://88.198.67.189:18234`; the local
node, once synced, is an equivalent endpoint at `http://127.0.0.1:18234`.

The live node has the same lightwalletd gaps (no `ironwoodTree`, no Ironwood subtree roots, no
`chainMetadata`); 134 of the last 1989 blocks carry Ironwood transactions.

## Flow 5: roster and finalizer addresses (on the stale node, repeated later on the live one)

`roster_info` returned 13 members, each with a `zfinv1` address and a display-order `pubkey`.
`decode_finalizer_address` on the first address reports `valid: true`, the same display-order
`pubkey` and the raw key bytes.

## Flow 8 (restore part): restore from seed

`recovery_info` printed the 24 words and birthday 1. A new data directory restored with
`--seed ... --birthday 1` synced in 2 min 23 s and showed the same unconfirmed Ironwood balance.
Restoring with `--birthday 60000` on this indexer fails on purpose:

```text
error: Sync error. server error. server returned no Ironwood tree state at height 60000. the wallet birthday must be the Ironwood activation height on this server so the Ironwood tree can be rebuilt from scanned blocks.
```

## Flows 1 and 2 again, on the live node

Fresh wallet against `88.198.67.189`: birthday 1, 68 990 blocks scanned in 1 min 45 s.
`request_faucet` returned `{"request": "50000000"}`.
The faucet transaction confirmed at height 68996 (`confirmed_ironwood_balance: 50_000_000`).

`spendable_balance` still printed 0: `shielded_spendable_balance` and the CLI `notes` command
enumerated Sapling and Orchard only. Fixed; it now prints the Ironwood total.

## Flow 3: send to own address

Commands are piped into interactive mode (one command per process would lose the stored proposal):

```text
send utest1gf09...asea 10000000 "self send"
confirm
```

Proposal fee 10 000 zats, broadcast as `f8461b94...`, mined at 69007, summary kind `send-to-self`,
two unspent Ironwood notes (10 000 000 and the 39 990 000 change).

## Flow 4: pool transfers

```text
transfer_pool ironwood transparent 5000000   -> fee 15000, txid 5a500f30..., mined at 69020
shield                                      -> value_to_shield 4985000, fee 15000, txid 439ac230..., mined
```

`balance` afterwards: 49 960 000 Ironwood, 0 transparent. There are no Orchard notes on this
network (every shielded output is Ironwood), so `transfer_pool orchard ironwood` cannot be exercised
against the live chain; the proposal path is the same code as the Ironwood to transparent transfer.

## Flow 5 again, on the live node

`roster_info` lists 13 finalizers with `zfinv1` addresses on the live chain too.

## Flow 6: stake

The staking calendar is the prototype one (`PROTOTYPE_STAKING`: period 10 368 blocks, day window
3 456, action delay 3 457; 25 s target spacing, so a three-day period with a one-day window).
Create, unbond and withdraw are only mined while `height % 10368 < 3456`; retarget and reward
conversion are exempt (`zebra-consensus/src/transaction.rs::check_staking_day_window`).

At the time of the run the tip was 69 026, position 6 818 in the period: the window is closed and
opens again at 72 576, about 24 h away at the target spacing.

The proposal built regardless (`kind: create_bond`, bond key derived, `fee: 10010000` = bond plus
10 000 zats) and `confirm` was rejected by the node:

```text
Send error. Transmission error. Transmission failed. Send Error: code: 'Unknown error', message: "the transaction will be rejected from the mempool until the next chain tip block: transaction did not pass consensus validation: staking action outside staking window: block 69030 is not within the staking window (staking allowed when block_height _ 10368 < 3456)"
```

The wallet kept the transaction as `status: failed` with the notes it would have spent still
spendable; `remove_transaction <txid>` cleared it.

Fix: the wallet now checks the calendar before building. `stake` at 69 038 answers

```text
the staking window is closed at height 69038; the next window opens at height 72576
```

Flows 6 and 7 (`stake`, `get_bonds`, `redelegate`, `begin_unstake`, `withdraw_stake`) therefore
cannot be driven to completion in this run: the first window opens at 72 576, unbonding needs the
following window (action delay 3 457 blocks puts it in the next period, 82 944) and withdrawal the
one after that (93 312), about a week of chain time end to end.

## Flow 8 on the live node: restart and restore

Every command above ran as a separate `zingo-cli` process, so the wallet file was reloaded from
disk before each one and the state (notes, spends, failed-then-removed transaction) carried over.
A restore of the same seed into a new directory with `--birthday 1` synced in 1 min 46 s and
showed the same 49 960 000 zats of Ironwood and all four transactions. Bond recovery could not be
exercised because no bond could be created (see flow 6).

## Local node

The pinned-commit `zebrad` (config `/tmp/claude/zebrad.toml`, data `/tmp/claude/zebrad-data`)
syncs the live chain through the crosslink peer protocol at a few blocks a second early on and
about one block a second once finality checking kicks in ("Deferring ... crosslink: child fp");
it was at 38 907 after 35 min. Once at the tip its `http://127.0.0.1:18234` is interchangeable
with the public live endpoint. Its own faucet pays from its own wallet, which is empty, so
`request_faucet` must go to a funded node such as `88.198.67.189`.

## Status

| Flow | Result |
|---|---|
| 1 fresh wallet, sync, balance, addresses | works (birthday forced to 1, full scan about 2 min) |
| 2 faucet | works against a live-chain node; the configured public peers are stale |
| 3 send to own address | works |
| 4 Ironwood to transparent, shield back | works; Orchard to Ironwood has no Orchard notes to move |
| 5 roster, decode finalizer address | works |
| 6 stake, get_bonds | blocked by the staking calendar until height 72 576 (about a day); the wallet now reports this up front |
| 7 redelegate, begin_unstake, withdraw_stake | blocked on 6, and on two further staking periods |
| 8 restart, restore from seed | works for notes and history; bonds untested |

To resume flows 6 and 7 once `height % 10368 < 3456`:

```text
stake <zfinv1 from roster_info> 10000000
confirm
get_bonds
redelegate <bond key> <another zfinv1>
confirm
# next staking day (height >= bond height + 3457 and inside the window):
begin_unstake <bond key>
confirm
# the staking day after that:
withdraw_stake <bond key>
```

## Indexer lists: nightly and stable from one consumer

A throwaway binary outside the workspace (depends on `zingolib` by path) registered, from an
empty `IndexerList`, the nightly peers from the node's `dev` config (`45.32.186.234`,
`64.177.122.73`) under `IndexerNetwork::CROSSLINK_NIGHTLY`, the four stable hosts under
`IndexerNetwork::CrosslinkFeaturenet`, and a `custom("my-devnet")` entry. It then built one
`LightClient` per network with `set_indexer_network`, probed, and selected the best indexer:

| Network | Probe | Selected |
|---|---|---|
| nightly | `45.32.186.234` height 9826, branch `37a5165b`; `64.177.122.73` port open, no gRPC answer in 5 s | `45.32.186.234` |
| stable | `88.198.67.189` height 69315, branch `46545531`; the three Shielded Labs hosts at 62149 | `88.198.67.189` |

Whole run 12 s, the unreachable nightly host bounded by the probe timeout.
