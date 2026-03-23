# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added

### Changed

- Support for Zebra 4.1.0 through `zebra-chain = "5.0"`
- Bump `tonic` from `0.13` to `0.14`, with `tls-webpki-roots` enabled.
- **Breaking:** Replace `GrpcConnector` struct with free function `get_client(uri: http::Uri)`.
  Callers must change `GrpcConnector::new(uri).get_client().await` to `get_client(uri).await`.
- **Breaking:** `get_client` now returns `CompactTxStreamerClient<Channel>` instead of
  `CompactTxStreamerClient<UnderlyingService>`. TLS and transport are handled internally
  by tonic.
- **Breaking:** `GetClientError` gains a `Transport` variant (wrapping `tonic::transport::Error`).

### Removed

- `client` module and `client_from_connector` utility function.
- `http-body` dependency.
- `GrpcConnector` struct, `GrpcConnector::new()`, and `GrpcConnector::uri()`.
- `UnderlyingService` type alias (`BoxCloneService<...>`).
- Manual URI rewrite logic (scheme/authority injection into requests); now handled
  internally by tonic's `Endpoint`.
- Direct dependencies on `tower` and `webpki-roots` (TLS root certs now provided by
  tonic's `tls-webpki-roots` feature).

## [1.1.0]

NOT PUBLISHED
