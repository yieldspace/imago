use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    concurrent::ConcurrentAction, conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature};

use crate::{
    atoms::ServiceAtom,
    deploy::DeployAction,
    manager_runtime::ManagerRuntimeAction,
    rpc::RpcAction,
    session_transport::SessionTransportAction,
    shutdown_flow::ShutdownFlowAction,
    supervision::SupervisionAction,
    system::{SystemAtomicAction, SystemEffect, SystemSpec, SystemState},
};

/// imagod-control runtime surface projected from the unified `system` spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum RuntimeProjectionAction {
    /// Upload, commit, promote, and start service0.
    DeployService0,
    /// Upload, commit, promote, and start service1.
    DeployService1,
    /// Roll back the promoted release for service0.
    RollbackService0,
    /// Resolve one local RPC from service0 to service1.
    LocalRpcResolved,
    /// Reject one local RPC from service0.
    LocalRpcDenied,
    /// Connect, invoke, complete, and disconnect one remote RPC for service0.
    RemoteRpcLifecycle,
    /// Stop service0 through the complete stop path.
    StopService0,
    /// Reap an already-exited service0 instance.
    ReapExitedService0,
    /// Drain shutdown from signal to finalize.
    ShutdownDrain,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeProjectionSpec;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeProjectionObservedState {
    pub trace: Vec<RuntimeProjectionAction>,
}

impl RuntimeProjectionSpec {
    pub const fn new() -> Self {
        Self
    }

    fn system(self) -> SystemSpec {
        SystemSpec::new()
    }

    pub fn initial_state(self) -> SystemState {
        self.system().initial_state()
    }

    fn apply_atomic(self, state: &SystemState, action: SystemAtomicAction) -> Option<SystemState> {
        self.system()
            .transition(state, &ConcurrentAction::from_atomic(action))
    }

    fn apply_many(
        self,
        state: &SystemState,
        actions: impl IntoIterator<Item = SystemAtomicAction>,
    ) -> Option<SystemState> {
        actions
            .into_iter()
            .try_fold(state.clone(), |candidate, action| {
                self.apply_atomic(&candidate, action)
            })
    }

    fn deploy_service(self, state: &SystemState, service: ServiceAtom) -> Option<SystemState> {
        self.apply_many(
            state,
            [
                SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(service)),
                SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(service)),
                SystemAtomicAction::Deploy(DeployAction::CommitUpload(service)),
                SystemAtomicAction::Deploy(DeployAction::SetRestartPolicy(service)),
                SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(service)),
                SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(service)),
                SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(service)),
                SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(service)),
                SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(service)),
                SystemAtomicAction::Supervision(SupervisionAction::StartServing(service)),
            ],
        )
    }

    fn stop_service(self, state: &SystemState, service: ServiceAtom) -> Option<SystemState> {
        self.apply_many(
            state,
            [
                SystemAtomicAction::Supervision(SupervisionAction::RequestStop(service)),
                SystemAtomicAction::Supervision(SupervisionAction::ReapService(service)),
            ],
        )
    }

    fn with_binding_prefix(
        self,
        state: &SystemState,
        mut actions: Vec<SystemAtomicAction>,
    ) -> Vec<SystemAtomicAction> {
        if !state.rpc.binding_allowed(ServiceAtom::Service0) {
            actions.insert(
                0,
                SystemAtomicAction::Rpc(RpcAction::GrantBinding(ServiceAtom::Service0)),
            );
        }
        actions
    }

    fn shutdown_drain(self, state: &SystemState) -> Option<SystemState> {
        let mut actions = vec![
            SystemAtomicAction::Shutdown(ShutdownFlowAction::ReceiveSignal),
            SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown),
            SystemAtomicAction::Session(SessionTransportAction::BeginShutdown),
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopAccepting),
            SystemAtomicAction::Shutdown(ShutdownFlowAction::DrainSessions),
        ];
        for service in [ServiceAtom::Service0, ServiceAtom::Service1] {
            if state.supervision.service_is_running(service) {
                actions.push(SystemAtomicAction::Supervision(
                    SupervisionAction::RequestStop(service),
                ));
            }
        }
        actions.extend([
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopServicesGraceful),
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopMaintenance),
            SystemAtomicAction::Shutdown(ShutdownFlowAction::Finalize),
            SystemAtomicAction::Manager(ManagerRuntimeAction::FinishShutdown),
        ]);
        self.apply_many(state, actions)
    }
}

impl TransitionSystem for RuntimeProjectionSpec {
    type State = SystemState;
    type Action = RuntimeProjectionAction;

    fn name(&self) -> &'static str {
        "runtime_projection"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match action {
            RuntimeProjectionAction::DeployService0 => {
                self.deploy_service(state, ServiceAtom::Service0)
            }
            RuntimeProjectionAction::DeployService1 => {
                self.deploy_service(state, ServiceAtom::Service1)
            }
            RuntimeProjectionAction::RollbackService0 => self.apply_many(
                state,
                [
                    SystemAtomicAction::Deploy(DeployAction::TriggerRollback(
                        ServiceAtom::Service0,
                    )),
                    SystemAtomicAction::Deploy(DeployAction::FinishRollback(ServiceAtom::Service0)),
                ],
            ),
            RuntimeProjectionAction::LocalRpcResolved => self.apply_many(
                state,
                self.with_binding_prefix(
                    state,
                    vec![SystemAtomicAction::Rpc(RpcAction::ResolveLocal(
                        ServiceAtom::Service0,
                    ))],
                ),
            ),
            RuntimeProjectionAction::LocalRpcDenied => self.apply_atomic(
                state,
                SystemAtomicAction::Rpc(RpcAction::RejectLocal(ServiceAtom::Service0)),
            ),
            RuntimeProjectionAction::RemoteRpcLifecycle => self.apply_many(
                state,
                self.with_binding_prefix(
                    state,
                    vec![
                        SystemAtomicAction::Rpc(RpcAction::ConnectRemote(ServiceAtom::Service0)),
                        SystemAtomicAction::Rpc(RpcAction::InvokeRemote(ServiceAtom::Service0)),
                        SystemAtomicAction::Rpc(RpcAction::CompleteRemoteCall(
                            ServiceAtom::Service0,
                        )),
                        SystemAtomicAction::Rpc(RpcAction::DisconnectRemote(ServiceAtom::Service0)),
                    ],
                ),
            ),
            RuntimeProjectionAction::StopService0 => {
                self.stop_service(state, ServiceAtom::Service0)
            }
            RuntimeProjectionAction::ReapExitedService0 => {
                self.stop_service(state, ServiceAtom::Service0)
            }
            RuntimeProjectionAction::ShutdownDrain => self.shutdown_drain(state),
        }
    }
}

impl TemporalSpec for RuntimeProjectionSpec {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
        self.system().invariants()
    }
}

impl ModelCaseSource for RuntimeProjectionSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_check_deadlocks(false)]
    }
}

impl ProtocolConformanceSpec for RuntimeProjectionSpec {
    type ExpectedOutput = Vec<SystemEffect>;
    type ObservedState = RuntimeProjectionObservedState;
    type ObservedOutput = Vec<SystemEffect>;

    fn expected_output(
        &self,
        _prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        if matches!(action, RuntimeProjectionAction::ShutdownDrain) && next.is_some() {
            vec![SystemEffect::ShutdownComplete]
        } else {
            Vec::new()
        }
    }

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State {
        observed
            .trace
            .iter()
            .fold(self.initial_state(), |state, action| {
                self.transition(&state, action)
                    .expect("runtime projection trace should stay valid")
            })
    }

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput {
        observed.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_service_action_reaches_running_state() {
        let spec = RuntimeProjectionSpec::new();
        let state = spec
            .transition(
                &spec.initial_state(),
                &RuntimeProjectionAction::DeployService0,
            )
            .expect("deploy action should succeed");

        assert!(state.supervision.service_is_running(ServiceAtom::Service0));
    }

    #[test]
    fn shutdown_drain_finishes_with_shutdown_effect() {
        let spec = RuntimeProjectionSpec::new();
        let state = spec
            .transition(
                &spec.initial_state(),
                &RuntimeProjectionAction::DeployService0,
            )
            .expect("deploy action should succeed");
        let next = spec
            .transition(&state, &RuntimeProjectionAction::ShutdownDrain)
            .expect("shutdown should drain");

        assert_eq!(
            spec.expected_output(&state, &RuntimeProjectionAction::ShutdownDrain, Some(&next)),
            vec![SystemEffect::ShutdownComplete]
        );
    }
}
