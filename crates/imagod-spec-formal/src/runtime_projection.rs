use imagod_spec::{
    RuntimeOutputSummary, RuntimeProbeOutput, RuntimeProbeState, RuntimeStateSummary,
};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    concurrent::ConcurrentAction, conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature, nirvash_projection_model};

use crate::{
    atoms::ServiceAtom,
    deploy::DeployAction,
    deploy::DeployState,
    manager_runtime::ManagerRuntimeAction,
    rpc::RpcAction,
    rpc::RpcState,
    session_transport::SessionTransportAction,
    shutdown_flow::ShutdownFlowAction,
    summary_mapping::{shutdown_phase, system_effect},
    supervision::SupervisionAction,
    supervision::SupervisionState,
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
        let mut pre_shutdown = state.clone();
        let rpc_summary = RuntimeStateSummary {
            binding_granted_service0: state.rpc.binding_allowed(ServiceAtom::Service0),
            ..RuntimeStateSummary::default()
        };
        pre_shutdown.rpc = RpcState::from_runtime_summary(&rpc_summary);
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
                actions.push(SystemAtomicAction::Supervision(
                    SupervisionAction::ReapService(service),
                ));
            }
        }
        actions.extend([
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopServicesForced),
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopMaintenance),
            SystemAtomicAction::Shutdown(ShutdownFlowAction::Finalize),
            SystemAtomicAction::Manager(ManagerRuntimeAction::FinishShutdown),
        ]);
        let mut next = self.apply_many(&pre_shutdown, actions)?;
        next.rpc = RpcState::from_runtime_summary(&rpc_summary);
        Some(normalize_runtime_state(self, next))
    }
}

fn runtime_summary_from_state(state: &SystemState) -> RuntimeStateSummary {
    let service0_running = state.supervision.service_is_running(ServiceAtom::Service0);
    let service1_running = state.supervision.service_is_running(ServiceAtom::Service1);
    let manager_stopped = matches!(
        state.manager.phase,
        crate::manager_runtime::ManagerRuntimePhase::Stopped
    );

    RuntimeStateSummary {
        service0_promoted: state.deploy.release_promoted(ServiceAtom::Service0),
        service1_promoted: state.deploy.release_promoted(ServiceAtom::Service1),
        service0_running,
        service1_running,
        service0_reaped: !service0_running && state.deploy.release_promoted(ServiceAtom::Service0),
        service1_reaped: !service1_running && state.deploy.release_promoted(ServiceAtom::Service1),
        service0_rolled_back: state.deploy.rollback_observed(ServiceAtom::Service0),
        binding_granted_service0: state.rpc.binding_allowed(ServiceAtom::Service0),
        remote_connected: state.rpc.has_remote_connection_for(ServiceAtom::Service0),
        manager_shutdown_started: manager_stopped
            || matches!(
                state.manager.phase,
                crate::manager_runtime::ManagerRuntimePhase::ShutdownRequested
            ),
        manager_stopped,
        session_shutdown_requested: state.session.shutdown_requested,
        shutdown: imagod_spec::ShutdownStateSummary {
            phase: match state.shutdown.phase {
                crate::shutdown_flow::ShutdownPhase::Idle => {
                    imagod_spec::SummaryShutdownPhase::Idle
                }
                crate::shutdown_flow::ShutdownPhase::SignalReceived => {
                    imagod_spec::SummaryShutdownPhase::SignalReceived
                }
                crate::shutdown_flow::ShutdownPhase::DrainingSessions => {
                    imagod_spec::SummaryShutdownPhase::DrainingSessions
                }
                crate::shutdown_flow::ShutdownPhase::StoppingServices => {
                    imagod_spec::SummaryShutdownPhase::StoppingServices
                }
                crate::shutdown_flow::ShutdownPhase::StoppingMaintenance => {
                    imagod_spec::SummaryShutdownPhase::StoppingMaintenance
                }
                crate::shutdown_flow::ShutdownPhase::Completed => {
                    imagod_spec::SummaryShutdownPhase::Completed
                }
            },
            accepts_stopped: state.shutdown.accepts_stopped,
            sessions_drained: state.shutdown.sessions_drained,
            services_stopped: state.shutdown.services_stopped,
            maintenance_stopped: state.shutdown.maintenance_stopped,
            forced_stop_attempted: state.shutdown.forced_stop_attempted,
        },
    }
}

fn normalize_runtime_state(spec: RuntimeProjectionSpec, state: SystemState) -> SystemState {
    let summary = runtime_summary_from_state(&state);
    <RuntimeProjectionSpec as ProtocolConformanceSpec>::abstract_state(&spec, &summary)
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
        let shutdown_idle = matches!(
            state.shutdown.phase,
            crate::shutdown_flow::ShutdownPhase::Idle
        );
        match action {
            RuntimeProjectionAction::DeployService0 => {
                if !shutdown_idle
                    || state.deploy.release_promoted(ServiceAtom::Service0)
                    || state.supervision.service_is_running(ServiceAtom::Service0)
                {
                    return None;
                }
                self.deploy_service(state, ServiceAtom::Service0)
                    .map(|next| normalize_runtime_state(*self, next))
            }
            RuntimeProjectionAction::DeployService1 => {
                if !shutdown_idle
                    || state.deploy.release_promoted(ServiceAtom::Service1)
                    || state.supervision.service_is_running(ServiceAtom::Service1)
                {
                    return None;
                }
                self.deploy_service(state, ServiceAtom::Service1)
                    .map(|next| normalize_runtime_state(*self, next))
            }
            RuntimeProjectionAction::RollbackService0 => self
                .apply_many(
                    state,
                    [
                        SystemAtomicAction::Deploy(DeployAction::TriggerRollback(
                            ServiceAtom::Service0,
                        )),
                        SystemAtomicAction::Deploy(DeployAction::FinishRollback(
                            ServiceAtom::Service0,
                        )),
                    ],
                )
                .filter(|_| {
                    shutdown_idle
                        && state.deploy.release_promoted(ServiceAtom::Service0)
                        && !state.supervision.service_is_running(ServiceAtom::Service0)
                        && !state.deploy.rollback_observed(ServiceAtom::Service0)
                })
                .map(|next| normalize_runtime_state(*self, next)),
            RuntimeProjectionAction::LocalRpcResolved => {
                if !shutdown_idle
                    || !state.supervision.service_is_running(ServiceAtom::Service0)
                    || !state.supervision.service_is_running(ServiceAtom::Service1)
                    || state.rpc.has_local_resolution_for(ServiceAtom::Service0)
                    || state.rpc.has_denied_local_call_for(ServiceAtom::Service0)
                {
                    return None;
                }
                self.apply_many(
                    state,
                    self.with_binding_prefix(
                        state,
                        vec![SystemAtomicAction::Rpc(RpcAction::ResolveLocal(
                            ServiceAtom::Service0,
                        ))],
                    ),
                )
                .map(|next| normalize_runtime_state(*self, next))
            }
            RuntimeProjectionAction::LocalRpcDenied => {
                if !shutdown_idle
                    || !state.supervision.service_is_running(ServiceAtom::Service0)
                    || state.supervision.service_is_running(ServiceAtom::Service1)
                    || state.rpc.has_local_resolution_for(ServiceAtom::Service0)
                    || state.rpc.has_denied_local_call_for(ServiceAtom::Service0)
                {
                    return None;
                }
                self.apply_atomic(
                    state,
                    SystemAtomicAction::Rpc(RpcAction::RejectLocal(ServiceAtom::Service0)),
                )
                .map(|next| normalize_runtime_state(*self, next))
            }
            RuntimeProjectionAction::RemoteRpcLifecycle => {
                if !shutdown_idle
                    || !state.supervision.service_is_running(ServiceAtom::Service0)
                    || state
                        .rpc
                        .has_completed_remote_call_for(ServiceAtom::Service0)
                    || state.rpc.has_denied_remote_call_for(ServiceAtom::Service0)
                {
                    return None;
                }
                self.apply_many(
                    state,
                    self.with_binding_prefix(
                        state,
                        vec![
                            SystemAtomicAction::Rpc(RpcAction::ConnectRemote(
                                ServiceAtom::Service0,
                            )),
                            SystemAtomicAction::Rpc(RpcAction::InvokeRemote(ServiceAtom::Service0)),
                            SystemAtomicAction::Rpc(RpcAction::CompleteRemoteCall(
                                ServiceAtom::Service0,
                            )),
                            SystemAtomicAction::Rpc(RpcAction::DisconnectRemote(
                                ServiceAtom::Service0,
                            )),
                        ],
                    ),
                )
                .map(|next| normalize_runtime_state(*self, next))
            }
            RuntimeProjectionAction::StopService0 => {
                if !state.supervision.service_is_running(ServiceAtom::Service0)
                    || !matches!(
                        state.manager.phase,
                        crate::manager_runtime::ManagerRuntimePhase::ShutdownRequested
                    )
                {
                    return None;
                }
                self.stop_service(state, ServiceAtom::Service0)
                    .map(|next| normalize_runtime_state(*self, next))
            }
            RuntimeProjectionAction::ReapExitedService0 => {
                if !state.supervision.service_is_running(ServiceAtom::Service0)
                    || !matches!(
                        state.manager.phase,
                        crate::manager_runtime::ManagerRuntimePhase::ShutdownRequested
                    )
                {
                    return None;
                }
                self.stop_service(state, ServiceAtom::Service0)
                    .map(|next| normalize_runtime_state(*self, next))
            }
            RuntimeProjectionAction::ShutdownDrain => {
                if !shutdown_idle
                    || !matches!(
                        state.manager.phase,
                        crate::manager_runtime::ManagerRuntimePhase::Listening
                    )
                {
                    return None;
                }
                self.shutdown_drain(state)
            }
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

nirvash_projection_model! {
    probe_state = RuntimeProbeState,
    probe_output = RuntimeProbeOutput,
    summary_state = RuntimeStateSummary,
    summary_output = RuntimeOutputSummary,
    abstract_state = SystemState,
    expected_output = Vec<SystemEffect>,
    state_seed = spec.initial_state(),
    state_summary {
        service0_promoted <= probe.service0_promoted,
        service1_promoted <= probe.service1_promoted,
        service0_running <= probe.service0_running,
        service1_running <= probe.service1_running,
        service0_reaped <= probe.service0_reaped,
        service1_reaped <= probe.service1_reaped,
        service0_rolled_back <= probe.service0_rolled_back,
        binding_granted_service0 <= probe.binding_granted_service0,
        remote_connected <= probe.remote_connected,
        manager_shutdown_started <= probe.manager_shutdown_started,
        manager_stopped <= probe.manager_stopped,
        session_shutdown_requested <= probe.session_shutdown_requested,
        shutdown <= probe.shutdown,
    }
    output_summary {
        effects <= probe.output.effects.clone()
    }
    state_abstract {
        state.deploy <= DeployState::from_runtime_summary(summary),
        state.supervision <= SupervisionState::from_runtime_summary(summary),
        state.rpc <= RpcState::from_runtime_summary(summary),
        state.manager.phase <= if summary.manager_stopped {
            crate::manager_runtime::ManagerRuntimePhase::Stopped
        } else if summary.manager_shutdown_started {
            crate::manager_runtime::ManagerRuntimePhase::ShutdownRequested
        } else {
            crate::manager_runtime::ManagerRuntimePhase::Listening
        },
        state.session.shutdown_requested <= summary.session_shutdown_requested,
        state.shutdown.phase <= shutdown_phase(summary.shutdown.phase),
        state.shutdown.accepts_stopped <= summary.shutdown.accepts_stopped,
        state.shutdown.sessions_drained <= summary.shutdown.sessions_drained,
        state.shutdown.services_stopped <= summary.shutdown.services_stopped,
        state.shutdown.maintenance_stopped <= summary.shutdown.maintenance_stopped,
        state.shutdown.forced_stop_attempted <= summary.shutdown.forced_stop_attempted,
    }
    output_abstract {
        imagod_spec::ContractEffectSummary::RequestObserved(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::Response(_, _) => system_effect(effect).expect("response effect should map"),
        effect @ imagod_spec::ContractEffectSummary::CommandEvent(_, _) => system_effect(effect).expect("command event should map"),
        effect @ imagod_spec::ContractEffectSummary::LogChunk(_, _) => system_effect(effect).expect("log chunk should map"),
        effect @ imagod_spec::ContractEffectSummary::LogsEnd(_) => system_effect(effect).expect("logs end should map"),
        imagod_spec::ContractEffectSummary::AuthorizationGranted(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::AuthorizationRejected(_, _) => system_effect(effect).expect("authorization rejection should map"),
        imagod_spec::ContractEffectSummary::LocalRpcResolved(_) => drop,
        imagod_spec::ContractEffectSummary::LocalRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcConnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcCompleted(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDisconnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::TaskMilestone(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::ShutdownComplete => system_effect(effect).expect("shutdown completion should map")
    }
    impl ProtocolConformanceSpec for RuntimeProjectionSpec {
        type ExpectedOutput = Vec<SystemEffect>;

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
