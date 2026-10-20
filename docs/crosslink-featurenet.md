# Crosslink featurenet

What this wallet talks to, and what changed with the `0735e958` monolith pin.

## The network

| Item | Value |
|---|---|
| Base | Zcash testnet parameters, network magic `ClT1` |
| Genesis | `05a60a92d99d85997cce3b87616c089f6124d7342af37106edc76126334a2c38` |
| Upgrades | every upgrade through NU6.3 at height 1; `FeaturenetUpgrade1` at 55 296 (branch id `0x46545531`) |
| Pools | transparent, Sprout, Sapling, Orchard, Ironwood, deferred, staking bonded, staking unbonded |

The node's default peers, all Shielded Labs hosts, serve the PoW network on port 8233, the
crosslink peer protocol on UDP 12001 and Tenderlink on 12301:
`45.63.75.9`, `70.34.220.94`, `64.177.44.176`. Every node embeds a lightwalletd-compatible gRPC
server on its P2P port plus 10001 (`18234`).

As of 2026-10-19 those three hosts sit on a stale fork at height 62149 without
`FeaturenetUpgrade1`; the live chain (branch id `46545531`, near height 69 000) is served by other
peers, for example `http://88.198.67.189:18234`. See [featurenet-e2e.md](featurenet-e2e.md).

## What the node's lightwalletd does not serve

`GetTreeState` returns no Ironwood tree, `GetSubtreeRoots` rejects the Ironwood protocol and compact
blocks carry no `chainMetadata`. The wallet copes: it counts Ironwood actions from the nullifier
stream to size the tree, builds the Ironwood tree from scanned blocks alone, and starts a fresh
wallet at the Ironwood activation height (1) because the Ironwood frontier at any later birthday
is unavailable. A restore from seed on this network must pass `--birthday 1`; a later birthday
fails the first sync with `ServerError::IronwoodTreeStateUnavailable`.

`ConfiguredActivationHeights::crosslink_featurenet()` is the schedule the wallet uses for
`ChainType::Testnet`; it must match the node or every transaction carries the wrong branch id.

## Ironwood

NU6.3 brings the Ironwood pool (ZIP 2005). Ironwood notes are Orchard-shaped, decrypt with the
Orchard viewing keys, and are received at the Orchard receiver of a unified address. On this
network the pool is active from height 1, so every payment to an Orchard receiver is built in
the Ironwood bundle and all shielded funds arrive in Ironwood.

`pepper-sync` scans the compact block's `ironwoodActions`, keeps a third shard tree, tracks
Ironwood nullifiers and spends, and `zingolib` selects and spends Ironwood notes like any other.

### Moving funds between pools

```text
transfer_pool <from-pool> <to-pool> <amount in zatoshis>
confirm
```

`LightClient::propose_pool_transfer(from, to, amount, account)` builds a payment to one of the
wallet's own addresses, selecting inputs only from `from` and returning change there, so exactly
`amount` crosses. `transfer_pool orchard ironwood <amount>` migrates Orchard funds. Transparent
funds are shielded with `shield` as before.

## Staking

All staking goes through proposals, confirmed with `confirm`:

| Command | Action |
|---|---|
| `stake <zfinv1 address> <zats>` | `CreateNewDelegationBond`; amount must be a power of 10 from `FeaturenetUpgrade1`; the bond amount is paid on top of the fee |
| `begin_unstake <bond key>` | `BeginDelegationUnbonding` |
| `redelegate <bond key> <zfinv1 address>` | `RetargetDelegationBond`, from the bond's current target |
| `withdraw_stake <bond key>` | `WithdrawDelegationBond`; builds and broadcasts at once |
| `get_bonds`, `roster_info` | wallet bonds (node amount, status, `last_action_height`, `next_action_height`) and the finalizer roster, with `zfinv1` addresses |

Bond keys derive from the account's bond root key plus the create action's terms, so a wallet
restored from seed recovers them; `LightWallet::bond_create_action` finds the create action the
backend signs later actions from.

Create, unbond and withdraw are only mined inside the staking day window,
`height % 10368 < 3456` on the public calendar (`zingolib::crosslink::PUBLIC_STAKING_CALENDAR`);
retarget is exempt. The wallet refuses to build outside the window and names the next height it
opens (`StakingWindowClosed`).

Each bond action also waits for the action delay (3 457 blocks) after the bond's previous action.
`GetBondInfo` reports the bond's status (`BondStatus`: active, unbonding, withdrawn, burned) and
the height of that previous action; `zingolib::crosslink::BondInfo::next_action_height` turns it
into the first height consensus accepts the next action, which `get_bonds` prints. `begin_unstake`
and `redelegate` ask the node first and fail with `BondNotActive` or `StakingActionDelayNotMet`
rather than at the mempool; `withdraw_stake` requires the node to hold the bond as unbonding.

## Finalizer addresses

`zfinv1` + 128 base64url characters (134 in total): a 32-byte ed25519 public key and a 64-byte
signature over `Zcash Crosslink Finalizer Address v1`. `zingolib::crosslink::parse_finalizer_address`
decodes and verifies; `encode_finalizer_address` prints. `decode_finalizer_address` does the same
from the CLI. A `zfinv2` prefix is rejected by design.

## Wallet files

Older wallet files load. Ironwood data is absent and is rebuilt from the server on the next
sync. A spending key written before bond keys existed is loaded with a placeholder bond root and
replaced by the seed-derived one when the mnemonic is present; a wallet imported from bare
spending-key bytes keeps the placeholder, so its bond keys will not match a node wallet's.
