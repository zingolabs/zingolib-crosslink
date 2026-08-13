# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added
`--seed-derivation` option - lets the user select the seed-derivation scheme when the user makes a wallet.

### Changed
`remove_transaction` command - now only allows transactions with the new `Failed` status to be removed.

### Removed
`resend` command - see zingolib CHANGELOG.md on `LightClient::resend`
`send_progress` command

## [0.2.0]

