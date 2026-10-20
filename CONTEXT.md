# Zingolib (Crosslink)

A Zcash light-wallet library, forked to interoperate with the Crosslink consensus node.
This glossary records terms whose meaning is specific to that interoperation, where the
fork deliberately diverges from upstream zingolib and from the Zcash specifications.

## Language

### Seed & key derivation

**Seed derivation**:
The scheme that maps a wallet's BIP-39 seed to its spending keys. Every wallet has
exactly one such scheme, fixed when the wallet is created. Two schemes exist in this
context: ZIP-32 standard and Crosslink-truncated.

**ZIP-32 standard derivation**:
Deriving spending keys from the full 64-byte BIP-39 seed, as ZIP-32 specifies. The
conforming default for every network.

**Crosslink-truncated derivation**:
Deriving spending keys from only the first 32 bytes of the 64-byte BIP-39 seed, matching
the zebra-crosslink node. A wallet uses this scheme so its addresses resolve identically
to the node's view — required to import a mnemonic from, or export one to, a crosslink
devnet where funds were created this way. A deliberate non-conformance with ZIP-32,
permitted only on test networks.
_Avoid_: "node wallet mode", "devnet seed", "32-byte seed".

**zebra-crosslink node**:
The Crosslink consensus/node implementation this library interoperates with. Its wallet
truncates the BIP-39 seed to 32 bytes, which is why Crosslink-truncated derivation exists.

### Pools & staking

**Ironwood pool**:
The shielded pool added by NU6.3 (ZIP 2005). Its notes are Orchard-shaped and are decrypted
with the account's Orchard viewing keys, but they carry version 3 note plaintexts, live in their
own note commitment tree, have their own nullifier set and are carried in a separate transaction
bundle. The sync engine tracks it as a third shielded pool next to Sapling and Orchard. On the
Crosslink featurenet every payment to an Orchard receiver is built into Ironwood.
_Avoid_: "Orchard v3", "new Orchard".

**Pool transfer**:
A payment from one of the wallet's pools to one of its own addresses in another pool, with inputs
restricted to the source pool. The wallet's one-step form of the ZIP 318 migration.
_Avoid_: "migration" alone (ZIP 318 names a scheduled, multi-transaction process).

**Finalizer address**:
A finalizer's self-certifying capability: its ed25519 public key plus that key's signature over a
fixed message, encoded as `zfinv1` followed by 128 base64url characters. Consensus rejects a stake
to an address whose signature does not verify, so the wallet verifies it before building.
_Avoid_: "finalizer pubkey" when the full address is meant; the key alone cannot be staked to.

**Bond key**:
The ed25519 key that owns a delegation bond, derived from the account's bond root key, the target
finalizer, the amount and a salt carried in the create action. A wallet restored from seed
rederives it from the mined action; the wallet signs later actions on the bond with it.
_Avoid_: "unique pubkey" (the wire field name) in prose.
