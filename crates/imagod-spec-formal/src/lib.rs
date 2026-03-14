pub mod authz_view;
pub mod bounds;
pub mod control_view;
pub mod manager_view;
pub mod operation_view;
pub mod service_view;
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

pub use imagod_spec::{
    AuthorizationDecision, AuthorizationDenialReason, BindingGrantId, CommandKind,
    CommandLifecycleState, CommandTerminalState, ExternalMessage, InterfaceId, MaintenancePhase,
    ManagerAuthState, ManagerPhase, ManagerShutdownPhase, OperationPermission, RemoteAuthorityId,
    RpcOutcome, ServiceId, ServiceLifecyclePhase, SessionAuthState, SessionId, SessionRequestState,
    SessionRole, SystemEvent, SystemStateFragment, TransportPrincipal,
};
pub use system::{SystemAction, SystemSpec, SystemState};
