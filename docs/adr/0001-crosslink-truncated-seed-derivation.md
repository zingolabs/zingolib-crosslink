# 0001: Crosslink Truncated Seed Derivation

The zebra-crosslink node makes spending keys from only the first 32 bytes of the 64-byte BIP-39 seed. We add a seed-derivation scheme that can match the node or follow the ZIP-32 standard. This lets wallets work with the existing crosslink devnet funds.

## Considered Options

`Zip32Standard` uses the full 64-byte BIP-39 seed. This option follows the ZIP-32 standard. It is the default scheme for new wallets.

`CrosslinkTruncated32` uses only the first 32 bytes of the seed. This option matches the derivation method of the zebra-crosslink node. It does not follow the ZIP-32 standard.

## Consequences

The user selects the scheme when the user makes a new wallet. The wallet stores the selected scheme. The wallet file format keeps the scheme in version 40. The library does not allow `CrosslinkTruncated32` on mainnet.

This scheme does not follow the ZIP-32 standard. We accept this difference to work with the existing crosslink devnet wallets and funds. The correct long-term fix belongs in the zebra-crosslink node.
