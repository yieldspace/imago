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
pub enum RunnerAtom {
    Runner0,
    Runner1,
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
pub enum ServiceAppAtom {
    Rpc,
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
pub enum WitAtom {
    Control,
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
pub enum BindingTargetAtom {
    Service0Control,
    Service1Control,
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
pub enum RpcConnectionAtom {
    Connection0,
    Connection1,
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
pub enum RpcCallAtom {
    Call0,
    Call1,
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
    CommandEvent,
    StateRequest,
    ServicesList,
    CommandCancel,
    LogsRequest,
    LogsChunk,
    LogsEnd,
    RpcInvoke,
    BindingsCertUpload,
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
pub enum CommandEventAtom {
    Accepted,
    Running,
    Succeeded,
    Failed,
    Canceled,
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
pub enum LogChunkAtom {
    Chunk0,
    Chunk1,
}

pub const fn service_runner(service: ServiceAtom) -> RunnerAtom {
    match service {
        ServiceAtom::Service0 => RunnerAtom::Runner0,
        ServiceAtom::Service1 => RunnerAtom::Runner1,
    }
}

pub const fn binding_target_for(source: ServiceAtom) -> BindingTargetAtom {
    match source {
        ServiceAtom::Service0 => BindingTargetAtom::Service1Control,
        ServiceAtom::Service1 => BindingTargetAtom::Service0Control,
    }
}

pub const fn binding_target_service(target: BindingTargetAtom) -> ServiceAtom {
    match target {
        BindingTargetAtom::Service0Control => ServiceAtom::Service0,
        BindingTargetAtom::Service1Control => ServiceAtom::Service1,
    }
}

pub const fn binding_target_wit(_target: BindingTargetAtom) -> WitAtom {
    WitAtom::Control
}
