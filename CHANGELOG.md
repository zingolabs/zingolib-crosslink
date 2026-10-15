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