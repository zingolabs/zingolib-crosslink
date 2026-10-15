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
`45.63.75.9`, `70.34.220.94`, `64.177.44.176`.

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
| `stake <zfinv1 address> <zats>` | `CreateNewDelegationBond`; amount must be a power of 10 from `FeaturenetUpgrade1` |
| `begin_unstake <bond key>` | `BeginDelegationUnbonding` |
| `redelegate <bond key> <zfinv1 address>` | `RetargetDelegationBond`, from the bond's current target |
| `withdraw_stake <bond key>` | `WithdrawDelegationBond`; builds and broadcasts at once |
| `get_bonds`, `roster_info` | wallet bonds and the finalizer roster, with `zfinv1` addresses |

Bond keys derive from the account's bond root key plus the create action's terms, so a wallet
restored from seed recovers them; `LightWallet::bond_create_action` finds the create action the
backend signs later actions from.

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
