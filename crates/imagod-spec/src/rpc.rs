//! Canonical RPC outcome fragments.

use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
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
)]
pub enum RpcOutcome {
    None,
    LocalInvoked,
    LocalDenied,
    RemoteConnected,
    RemoteInvoked,
    RemoteDenied,
    RemoteDisconnected,
}
