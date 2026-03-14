//! Canonical identity and session roles used by the formal system model.

use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, RelAtom,
    SymbolicEncoding as FormalSymbolicEncoding,
};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum SessionId {
    Session0,
    Session1,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum ServiceId {
    Service0,
    Service1,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum RemoteAuthorityId {
    Authority0,
    Authority1,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum TransportPrincipal {
    Admin,
    Client,
    ServiceRunner,
    Unknown,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum SessionRole {
    Admin,
    Client,
    ServiceRunner,
    Unknown,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum SessionAuthState {
    Disconnected,
    Accepted,
    Authenticated,
    Drained,
}
