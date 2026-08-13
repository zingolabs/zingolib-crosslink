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
