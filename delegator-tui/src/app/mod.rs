//! The pure application model: state, actions, and the reducer that connects them.
//!
//! Nothing in this module touches the terminal, the network, or the wallet. The backend
//! translates wallet activity into [`Action`]s, the reducer folds them into [`State`], and the
//! views render the state. Side effects leave the reducer only as [`Command`]s.

pub mod action;
pub mod reduce;
pub mod roles;
pub mod state;

pub use action::{Action, Command, Key, OpenSpec, SyncNote};
pub use reduce::reduce;
pub use state::*;
