//! Re-exported source-of-truth models shared with runtime crates.
//!
//! Simple command-domain enums and observed command state derive
//! `nirvash_core::Signature` directly in `imagod-model`, while this crate adds
//! spec-local projections and temporal behavior on top.

pub mod command {
    pub use imagod_model::command::*;
}
