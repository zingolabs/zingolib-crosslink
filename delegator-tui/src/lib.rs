//! delegator-tui: a watch-only terminal wallet for Zcash crosslink delegators.
//!
//! The binary in `main.rs` wires these modules together; they live in a library target so the
//! crate has one, which the workspace's feature-powerset check and the doc build expect.

pub mod app;
pub mod backend;
pub mod cli;
pub mod clipboard;
pub mod format;
pub mod input;
pub mod qr;
pub mod settings;
pub mod theme;
pub mod ui;
