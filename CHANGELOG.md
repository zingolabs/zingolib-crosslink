# Changelog

All notable changes corresponding to a workspace level to this project
will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Updated `zcash_local_net` and `zingo_test_vectors` to versions supporting Zebra 4.1.0
- Ironwood pool (ZIP 2005, NU6.3) support in `pepper-sync` and `zingolib`: scanning, nullifiers,
  a third note commitment tree, balances, note selection and spending. On the Crosslink
  featurenet every payment to an Orchard receiver lands in Ironwood.
- `LightClient::propose_pool_transfer` and the `transfer_pool` CLI command: move value from one of
  the wallet's pools into another with a single self-payment (the one-step Orchard to Ironwood
  migration).
- `zingolib::crosslink`: parse, verify and encode `zfinv1` finalizer addresses, and helpers over
  staking actions. The CLI gains `decode_finalizer_address`.
- Staking proposals take verified finalizer addresses and derive bond keys from the account's
  bond root key, so a wallet restored from seed recovers its bonds. The CLI registers `stake`,
  `begin_unstake`, `redelegate`, `withdraw_stake`, `get_bonds` and `roster_info`.
- `ConfiguredActivationHeights::crosslink_featurenet()`: the featurenet schedule, with
  `FeaturenetUpgrade1` at the height the node ships.
- `pepper-sync` syncs against a server that serves no Ironwood tree state, subtree roots or chain
  metadata (the crosslink node today): Ironwood subtree-root rejections are tolerated, Ironwood tree
  sizes are counted from the nullifier stream when the server's tree state omits the pool, and a
  birthday above the pool's activation fails with `ServerError::IronwoodTreeStateUnavailable`
  instead of mis-positioning notes. `LightClient::fresh_wallet_birthday` probes the server and
  starts a fresh wallet at the Ironwood activation height when needed; `zingo-cli` uses it.
- `AccountBalance` reports Ironwood balances; transaction summaries and value transfers list
  Ironwood notes, report `pool_received: Ironwood`, and `WalletTransaction` value totals and the
  transaction-kind classifier include the Ironwood pool.
- `zingolib::crosslink::{staking_window_open, next_staking_window_start, PUBLIC_STAKING_CALENDAR}`;
  `stake`, `begin_unstake` and `withdraw_stake` fail before building when the staking day window
  is closed at the target height (`StakingWindowClosed`), naming the next height it opens. The
  check is skipped on regtest, whose nets may run a shrunk calendar.
- `zingolib::crosslink::{BondInfo, BondStatus, earliest_bond_action_height, staking_calendar}`:
  the node's `GetBondInfo` response typed. `LightClient::get_bond_info` returns it; `get_bonds`
  reports `status_name`, `last_action_height` and `next_action_height` (the first height at which
  an active bond can be unbonded or an unbonding bond withdrawn); the staked amount counts only
  bonds the node holds as active or unbonding. `begin_unstake` and `redelegate` ask the node for
  the bond first and fail with `BondNotActive` or `StakingActionDelayNotMet` where consensus
  would reject them; `withdraw_stake` likewise requires the node to hold the bond as unbonding
  for at least the action delay.
- `zingolib::config::{IndexerList, IndexerNetwork, Indexer}`: lightwalletd endpoints
  grouped by network (mainnet, testnet, crosslink featurenet, regtest), with a built-in list and
  consumer injection through `ZingoConfigBuilder::{set_indexers, add_indexer, prefer_indexer}`,
  `ZingoConfig::{indexers, add_indexer, prefer_indexer}` and `LightClient::{indexers, add_indexer,
  set_server}`. `LightClient::probe_indexers` and `select_best_indexer` ask every listed endpoint
  for its tip and switch to the highest. `zingo-cli --server` is repeatable, defaults to the
  built-in list for the chain, and gains `indexers [probe]` and `change_server <uri|index|best>`.
  `IndexerNetwork::Custom` names deployments no chain can (`IndexerNetwork::CROSSLINK_NIGHTLY`
  ships with the node's nightly peers); `ZingoConfigBuilder::set_indexer_network` and
  `zingo-cli --network <name>` select which list a client uses.
- `zingo_netutils::get_client` bounds the connection attempt with `CONNECT_TIMEOUT` (10 s).
- `docs/featurenet-e2e.md`: the running log of the end-to-end run against the live featurenet.
### Changed
- removed `dev` branch reference from `zingo_common_components` crate and pointed to version `0.2`
- `crosslink_monolith` pinned to `0735e958` (NU6.3 at height 1, `FeaturenetUpgrade1` at 55296,
  self-certifying finalizer addresses, `StakingAction` as an enum, bond spending keys in the
  unified spending key). `orchard` 0.15, `sapling-crypto` 0.7, `shardtree` 0.7.
- `ConfiguredActivationHeights` is now defined in `zingolib::config` (the `zebra-chain` dependency
  is gone) and knows NU6.2, NU6.3 and `FeaturenetUpgrade1`.
- Wallet files: `pepper-sync` bumps the versions of transactions, tree bounds, nullifier maps,
  sync state, shard trees and Orchard-family notes to carry Ironwood data and the note plaintext
  version. Older files still load; the Ironwood tree is rebuilt from the server on the next sync.
- Spending keys written before the bond component existed are given a placeholder bond root on
  load and replaced with the seed-derived one when the wallet holds its mnemonic.
- `ShieldedProtocol` (deprecated alias) replaced by `ShieldedPool` throughout.
### Removed
- `StakingAction` struct fields (`arg32_*`, `arg64_*`); use the enum variants and accessors.
- The `zebra-chain` dependency.