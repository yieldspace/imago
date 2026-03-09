//! CBOR codec helpers shared by protocol-facing crates.

pub mod cbor;

/// CBOR serialization and deserialization helpers.
pub use cbor::{CborError, from_cbor, to_cbor};
