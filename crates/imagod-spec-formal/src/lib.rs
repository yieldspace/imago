pub mod atoms;
pub mod bounds;
pub mod control_plane;
pub mod manager_plane;
pub mod operation_plane;
pub mod service_plane;
pub mod system;

#[cfg(test)]
pub(crate) fn lowered_spec<T>(spec: &T) -> nirvash_lower::LoweredSpec<'_, T::State, T::Action>
where
    T: nirvash_lower::TemporalSpec,
    T::State: PartialEq + nirvash_lower::FiniteModelDomain,
    T::Action: PartialEq,
{
    let mut lowering_cx = nirvash_lower::LoweringCx;
    spec.lower(&mut lowering_cx).expect("spec should lower")
}

pub use atoms::{RemoteAuthorityAtom, RequestKindAtom, ServiceAtom, SessionAtom, StreamAtom};
pub use control_plane::{ControlPlaneAction, ControlPlaneSpec, ControlPlaneState, RequestPhase};
pub use imagod_spec::{
    CommandErrorKind, CommandKind, CommandLifecycleState, CommandProtocolAction,
    CommandProtocolStageId, OperationPhase,
};
pub use manager_plane::{ManagerPhase, ManagerPlaneAction, ManagerPlaneSpec, ManagerPlaneState};
pub use operation_plane::{
    OperationPlaneAction, OperationPlaneSpec, OperationPlaneState, RpcOutcome,
};
pub use service_plane::{
    ServiceLifecyclePhase, ServicePlaneAction, ServicePlaneSpec, ServicePlaneState,
};
pub use system::{SystemAction, SystemSpec, SystemState};
