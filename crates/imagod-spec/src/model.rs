//! Re-exported source-of-truth models shared with runtime crates.
//!
//! Simple command-domain enums and observed command state live in
//! `imago-protocol::command_contract`, while this crate adds spec-local
//! projections and temporal behavior on top.

pub mod command {
    pub use imago_protocol::command_contract::*;
}
