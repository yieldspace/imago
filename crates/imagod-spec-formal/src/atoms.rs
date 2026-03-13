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
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum ServiceAtom {
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
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum RemoteAuthorityAtom {
    Edge0,
    Edge1,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum SessionAtom {
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
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum StreamAtom {
    Stream0,
    Stream1,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum SessionRoleAtom {
    Admin,
    Client,
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
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum RequestKindAtom {
    HelloNegotiate,
    DeployPrepare,
    ArtifactPush,
    ArtifactCommit,
    CommandStart,
    StateRequest,
    ServicesList,
    CommandCancel,
    LogsRequest,
    RpcInvoke,
    BindingsCertUpload,
}
