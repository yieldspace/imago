//! Layered configuration loading pipeline:
//! filesystem IO -> TOML parsing -> typed decode -> semantic validation.

pub(super) mod io;
pub(super) mod parsing;
pub(super) mod validation;
