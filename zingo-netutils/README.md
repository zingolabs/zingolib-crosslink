# zingo-netutils

Network utilities for connecting to Zcash light-wallet indexers (`lightwalletd` / `zainod`).

## Overview

This crate provides a single async entry point, `get_client`, that returns a
ready-to-use gRPC `CompactTxStreamerClient`. TLS, HTTP/2 enforcement, and
webpki root certificates are handled internally so callers only need to supply
a URI.

## Usage

```rust
use zingo_netutils::get_client;

let uri = "https://mainnet.zainod.com:9067".parse().unwrap();
let mut client = get_client(uri).await?;
```

## Error handling

`get_client` returns a `GetClientError` with the following variants:

| Variant              | Cause                                   |
|----------------------|-----------------------------------------|
| `InvalidScheme`      | URI scheme is not `http` or `https`     |
| `InvalidAuthority`   | URI has no host/authority component     |
| `InvalidPathAndQuery`| URI contains an unexpected path / query |
| `Transport`          | Underlying tonic transport failure      |

## TLS

HTTPS connections use rustls with webpki root certificates (via tonic's
`tls-webpki-roots` feature). No system OpenSSL installation is required.

## License

[MIT](LICENSE-MIT)
