use imago_protocol::{CommandLifecycleState, CommandProtocolAction};
use nirvash_core::{
    Fairness, Ltl, ModelCase, ModelCaseSource, StatePredicate, StepPredicate, TemporalSpec,
    TransitionSystem,
};
use nirvash_macros::{fairness, invariant, property, system_spec};

use crate::{
    artifact_deploy::{
        ArtifactDeployAction, ArtifactDeploySpec, ArtifactDeployState, ReleaseStage,
    },
    command_protocol::{CommandProtocolSpec, CommandProtocolState},
    manager_shell::{ManagerShellAction, ManagerShellPhase, ManagerShellSpec, ManagerShellState},
    plugin_capability::{PluginCapabilityAction, PluginCapabilitySpec, PluginCapabilityState},
    runner_bootstrap::{RunnerBootstrapAction, RunnerBootstrapSpec, RunnerBootstrapState},
    runner_runtime::{RunnerRuntimeAction, RunnerRuntimeSpec, RunnerRuntimeState, RuntimePhase},
    service_supervision::{
        ServicePhase, ServiceSupervisionAction, ServiceSupervisionSpec, ServiceSupervisionState,
    },
    session_transport::{SessionTransportAction, SessionTransportSpec, SessionTransportState},
    shutdown_flow::{ShutdownFlowAction, ShutdownFlowSpec, ShutdownFlowState, ShutdownPhase},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagodSystemState {
    pub manager: ManagerShellState,
    pub transport: SessionTransportState,
    pub command: CommandProtocolState,
    pub deploy: ArtifactDeployState,
    pub supervision: ServiceSupervisionState,
    pub bootstrap: RunnerBootstrapState,
    pub runtime: RunnerRuntimeState,
    pub plugin: PluginCapabilityState,
    pub shutdown: ShutdownFlowState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImagodSystemAction {
    Manager(ManagerShellAction),
    Session(SessionTransportAction),
    Command(CommandProtocolAction),
    Deploy(ArtifactDeployAction),
    Supervision(ServiceSupervisionAction),
    Bootstrap(RunnerBootstrapAction),
    Runtime(RunnerRuntimeAction),
    Plugin(PluginCapabilityAction),
    Shutdown(ShutdownFlowAction),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ImagodSystemSpec;

impl ImagodSystemSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> ImagodSystemState {
        ImagodSystemState {
            manager: ManagerShellSpec::new().initial_state(),
            transport: SessionTransportSpec::new().initial_state(),
            command: CommandProtocolSpec::new().initial_state(),
            deploy: ArtifactDeploySpec::new().initial_state(),
            supervision: ServiceSupervisionSpec::new().initial_state(),
            bootstrap: RunnerBootstrapSpec::new().initial_state(),
            runtime: RunnerRuntimeSpec::new().initial_state(),
            plugin: PluginCapabilitySpec::new().initial_state(),
            shutdown: ShutdownFlowSpec::new().initial_state(),
        }
    }

    fn action_vocabulary(&self) -> Vec<ImagodSystemAction> {
        vec![
            ImagodSystemAction::Manager(ManagerShellAction::LoadExistingConfig),
            ImagodSystemAction::Deploy(ArtifactDeployAction::StartDeployMatched),
            ImagodSystemAction::Deploy(ArtifactDeployAction::PromoteRelease),
            ImagodSystemAction::Supervision(ServiceSupervisionAction::StartService),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::RegisterRunner),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::MarkReady),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::ValidateComponentLoadable),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing),
            ImagodSystemAction::Manager(ManagerShellAction::BeginShutdown),
            ImagodSystemAction::Shutdown(ShutdownFlowAction::Finalize),
        ]
    }

    fn transition_state(
        &self,
        prev: &ImagodSystemState,
        action: &ImagodSystemAction,
    ) -> Option<ImagodSystemState> {
        let mut candidate = prev.clone();
        let allowed = match action {
            ImagodSystemAction::Manager(ManagerShellAction::LoadExistingConfig)
                if matches!(prev.manager.phase, ManagerShellPhase::Booting) =>
            {
                candidate.manager.phase = ManagerShellPhase::Listening;
                candidate.manager.config_loaded = true;
                candidate.manager.plugin_gc = crate::manager_shell::TaskState::Succeeded;
                candidate.manager.boot_restore = crate::manager_shell::TaskState::Succeeded;
                true
            }
            ImagodSystemAction::Deploy(ArtifactDeployAction::StartDeployMatched)
                if matches!(prev.manager.phase, ManagerShellPhase::Listening)
                    && matches!(prev.deploy.release, ReleaseStage::None) =>
            {
                candidate.deploy.upload = crate::artifact_deploy::UploadStage::Committed;
                candidate.deploy.chunks =
                    crate::bounds::ArtifactChunks::new(1).expect("within bounds");
                candidate.deploy.release = ReleaseStage::Prepared;
                candidate.deploy.precondition_ok = true;
                true
            }
            ImagodSystemAction::Deploy(ArtifactDeployAction::PromoteRelease)
                if matches!(prev.manager.phase, ManagerShellPhase::Listening)
                    && matches!(prev.deploy.release, ReleaseStage::Prepared) =>
            {
                candidate.deploy.release = ReleaseStage::Promoted;
                true
            }
            ImagodSystemAction::Supervision(ServiceSupervisionAction::StartService)
                if matches!(prev.manager.phase, ManagerShellPhase::Listening)
                    && matches!(prev.deploy.release, ReleaseStage::Promoted)
                    && matches!(prev.supervision.phase, ServicePhase::Idle) =>
            {
                candidate.supervision.phase = ServicePhase::WaitingReady;
                candidate.supervision.active_services =
                    candidate.supervision.active_services.saturating_inc();
                true
            }
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::RegisterRunner)
                if matches!(prev.supervision.phase, ServicePhase::WaitingReady)
                    && !prev.bootstrap.registered =>
            {
                candidate.bootstrap.decoded = true;
                candidate.bootstrap.app_type = Some(imagod_ipc::RunnerAppType::Rpc);
                candidate.bootstrap.endpoint = crate::runner_bootstrap::EndpointState::Prepared;
                candidate.bootstrap.registered = true;
                candidate.bootstrap.auth = crate::runner_bootstrap::AuthProofState::Verified;
                true
            }
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::MarkReady)
                if prev.bootstrap.registered && !prev.bootstrap.ready =>
            {
                candidate.bootstrap.ready = true;
                candidate.supervision.phase = ServicePhase::Running;
                true
            }
            ImagodSystemAction::Runtime(RunnerRuntimeAction::ValidateComponentLoadable)
                if prev.bootstrap.ready
                    && matches!(
                        prev.runtime.component,
                        crate::runner_runtime::ComponentLoadClass::Unknown
                    ) =>
            {
                candidate.runtime.mode = Some(imagod_ipc::RunnerAppType::Rpc);
                candidate.runtime.phase = RuntimePhase::ComponentValidated;
                candidate.runtime.component = crate::runner_runtime::ComponentLoadClass::Loadable;
                true
            }
            ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing)
                if prev.bootstrap.ready
                    && matches!(prev.supervision.phase, ServicePhase::Running)
                    && matches!(prev.deploy.release, ReleaseStage::Promoted)
                    && matches!(prev.runtime.phase, RuntimePhase::ComponentValidated) =>
            {
                candidate.runtime.phase = RuntimePhase::Serving;
                true
            }
            ImagodSystemAction::Manager(ManagerShellAction::BeginShutdown)
                if matches!(prev.manager.phase, ManagerShellPhase::Listening) =>
            {
                candidate.manager.phase = ManagerShellPhase::ShutdownRequested;
                candidate.transport.shutdown_requested = true;
                candidate.transport.last_outcome = crate::session_transport::SessionOutcome::None;
                candidate.shutdown.phase = ShutdownPhase::SignalReceived;
                true
            }
            ImagodSystemAction::Shutdown(ShutdownFlowAction::Finalize)
                if matches!(prev.shutdown.phase, ShutdownPhase::SignalReceived) =>
            {
                candidate.shutdown.accepts_stopped = true;
                candidate.shutdown.sessions_drained = true;
                candidate.shutdown.phase = ShutdownPhase::Completed;
                candidate.shutdown.services_stopped = true;
                candidate.shutdown.maintenance_stopped = true;
                candidate.manager.phase = ManagerShellPhase::Stopped;
                true
            }
            _ => false,
        };
        (allowed && system_state_valid(&candidate)).then_some(candidate)
    }
}

fn system_model_cases() -> Vec<ModelCase<ImagodSystemState, ImagodSystemAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

fn state_respects_spec<T>(spec: &T, state: &T::State) -> bool
where
    T: TemporalSpec + ModelCaseSource,
{
    spec.invariants()
        .iter()
        .all(|predicate| predicate.eval(state))
        && spec
            .model_cases()
            .iter()
            .flat_map(|model_case| model_case.state_constraints().iter())
            .all(|constraint| constraint.eval(state))
}

fn system_state_valid(state: &ImagodSystemState) -> bool {
    state_respects_spec(&ManagerShellSpec::new(), &state.manager)
        && state_respects_spec(&SessionTransportSpec::new(), &state.transport)
        && state_respects_spec(&CommandProtocolSpec::new(), &state.command)
        && state_respects_spec(&ArtifactDeploySpec::new(), &state.deploy)
        && state_respects_spec(&ServiceSupervisionSpec::new(), &state.supervision)
        && state_respects_spec(&RunnerBootstrapSpec::new(), &state.bootstrap)
        && state_respects_spec(&RunnerRuntimeSpec::new(), &state.runtime)
        && state_respects_spec(&PluginCapabilitySpec::new(), &state.plugin)
        && state_respects_spec(&ShutdownFlowSpec::new(), &state.shutdown)
        && cross_links_hold(state)
}

#[invariant(ImagodSystemSpec)]
fn runtime_serving_requires_ready_and_promoted_release() -> StatePredicate<ImagodSystemState> {
    StatePredicate::new(
        "runtime_serving_requires_ready_and_promoted_release",
        |state| {
            !matches!(state.runtime.phase, RuntimePhase::Serving)
                || (state.bootstrap.ready
                    && matches!(state.supervision.phase, ServicePhase::Running)
                    && matches!(state.deploy.release, ReleaseStage::Promoted))
        },
    )
}

#[invariant(ImagodSystemSpec)]
fn shutdown_requires_transport_gate_and_manager_shutdown() -> StatePredicate<ImagodSystemState> {
    StatePredicate::new(
        "shutdown_requires_transport_gate_and_manager_shutdown",
        |state| {
            matches!(state.shutdown.phase, ShutdownPhase::Idle)
                || (state.transport.shutdown_requested
                    && matches!(
                        state.manager.phase,
                        ManagerShellPhase::ShutdownRequested | ManagerShellPhase::Stopped
                    ))
        },
    )
}

#[invariant(ImagodSystemSpec)]
fn ready_runner_requires_running_supervision() -> StatePredicate<ImagodSystemState> {
    StatePredicate::new("ready_runner_requires_running_supervision", |state| {
        !state.bootstrap.ready || matches!(state.supervision.phase, ServicePhase::Running)
    })
}

#[invariant(ImagodSystemSpec)]
fn active_command_requires_listening_manager() -> StatePredicate<ImagodSystemState> {
    StatePredicate::new("active_command_requires_listening_manager", |state| {
        !matches!(
            state.command.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        ) || matches!(state.manager.phase, ManagerShellPhase::Listening)
    })
}

#[property(ImagodSystemSpec)]
fn runtime_can_serve_after_release_and_ready() -> Ltl<ImagodSystemState, ImagodSystemAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("runtime_can_start", |state| {
            matches!(state.deploy.release, ReleaseStage::Promoted)
                && state.bootstrap.ready
                && matches!(state.supervision.phase, ServicePhase::Running)
                && matches!(state.runtime.phase, RuntimePhase::ComponentValidated)
                && matches!(
                    state.runtime.component,
                    crate::runner_runtime::ComponentLoadClass::Loadable
                )
        })),
        Ltl::pred(StatePredicate::new("runtime_serving", |state| {
            matches!(state.runtime.phase, RuntimePhase::Serving)
        })),
    )
}

#[property(ImagodSystemSpec)]
fn shutdown_started_leads_to_completed() -> Ltl<ImagodSystemState, ImagodSystemAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("shutdown_started", |state| {
            !matches!(state.shutdown.phase, ShutdownPhase::Idle)
        })),
        Ltl::pred(StatePredicate::new("shutdown_completed", |state| {
            matches!(state.shutdown.phase, ShutdownPhase::Completed)
        })),
    )
}

#[property(ImagodSystemSpec)]
fn runner_registered_leads_to_ready() -> Ltl<ImagodSystemState, ImagodSystemAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("runner_registered", |state| {
            state.bootstrap.registered
        })),
        Ltl::pred(StatePredicate::new("runner_ready", |state| {
            state.bootstrap.ready
        })),
    )
}

#[fairness(ImagodSystemSpec)]
fn runtime_start_fairness() -> Fairness<ImagodSystemState, ImagodSystemAction> {
    Fairness::weak(StepPredicate::new(
        "runtime_start_serving",
        |prev, action, next| {
            matches!(
                action,
                ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing)
            ) && matches!(prev.deploy.release, ReleaseStage::Promoted)
                && prev.bootstrap.ready
                && matches!(prev.supervision.phase, ServicePhase::Running)
                && matches!(next.runtime.phase, RuntimePhase::Serving)
        },
    ))
}

#[fairness(ImagodSystemSpec)]
fn shutdown_progress_fairness() -> Fairness<ImagodSystemState, ImagodSystemAction> {
    Fairness::weak(StepPredicate::new(
        "shutdown_progress",
        |prev, action, next| {
            matches!(action, ImagodSystemAction::Shutdown(_)) && prev.shutdown != next.shutdown
        },
    ))
}

#[fairness(ImagodSystemSpec)]
fn runner_ready_fairness() -> Fairness<ImagodSystemState, ImagodSystemAction> {
    Fairness::weak(StepPredicate::new(
        "runner_mark_ready",
        |prev, action, next| {
            matches!(
                action,
                ImagodSystemAction::Bootstrap(RunnerBootstrapAction::MarkReady)
            ) && prev.bootstrap.registered
                && next.bootstrap.ready
        },
    ))
}

#[system_spec(
    model_cases(system_model_cases),
    subsystems(
        "manager_shell",
        "session_transport",
        "command_protocol",
        "artifact_deploy",
        "service_supervision",
        "runner_bootstrap",
        "runner_runtime",
        "plugin_capability",
        "shutdown_flow"
    )
)]
impl TransitionSystem for ImagodSystemSpec {
    type State = ImagodSystemState;
    type Action = ImagodSystemAction;

    fn name(&self) -> &'static str {
        "imagod_system"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        self.action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.transition_state(state, action)
    }
}

fn cross_links_hold(state: &ImagodSystemState) -> bool {
    (!matches!(state.runtime.phase, RuntimePhase::Serving)
        || (state.bootstrap.ready
            && matches!(state.supervision.phase, ServicePhase::Running)
            && matches!(state.deploy.release, ReleaseStage::Promoted)))
        && (matches!(state.shutdown.phase, ShutdownPhase::Idle)
            || (state.transport.shutdown_requested
                && matches!(
                    state.manager.phase,
                    ManagerShellPhase::ShutdownRequested | ManagerShellPhase::Stopped
                )))
        && (!state.bootstrap.ready || matches!(state.supervision.phase, ServicePhase::Running))
        && (!matches!(
            state.command.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        ) || matches!(state.manager.phase, ManagerShellPhase::Listening))
}

#[nirvash_macros::formal_tests(
    spec = ImagodSystemSpec,
    composition = composition
)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_core::ModelChecker;

    use crate::{
        manager_shell::TaskState,
        runner_bootstrap::{AuthProofState, BootstrapSizeClass, EndpointState},
        runner_runtime::{ComponentLoadClass, SocketPolicyClass, WasmTuningClass},
        session_transport::SessionOutcome,
    };

    #[test]
    fn runtime_cannot_start_serving_before_runner_ready() {
        let spec = ImagodSystemSpec::new();
        let prev = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::Listening,
                config_loaded: true,
                created_default: false,
                plugin_gc: TaskState::Succeeded,
                boot_restore: TaskState::Succeeded,
            },
            transport: SessionTransportState {
                active_sessions: crate::bounds::SessionSlots::new(0).expect("within bounds"),
                shutdown_requested: false,
                last_outcome: SessionOutcome::None,
            },
            command: CommandProtocolSpec::new().initial_state(),
            deploy: ArtifactDeployState {
                upload: crate::artifact_deploy::UploadStage::Committed,
                release: ReleaseStage::Promoted,
                precondition_ok: true,
                auto_rollback: true,
                chunks: crate::bounds::ArtifactChunks::new(2).expect("within bounds"),
            },
            supervision: ServiceSupervisionState {
                active_services: crate::bounds::ServiceSlots::new(1).expect("within bounds"),
                phase: ServicePhase::Running,
                retained_logs: false,
            },
            bootstrap: RunnerBootstrapState {
                size: BootstrapSizeClass::WithinBounds,
                decoded: true,
                app_type: Some(imagod_ipc::RunnerAppType::Rpc),
                endpoint: EndpointState::Prepared,
                auth: AuthProofState::Verified,
                registered: true,
                ready: false,
            },
            runtime: RunnerRuntimeState {
                mode: Some(imagod_ipc::RunnerAppType::Rpc),
                phase: RuntimePhase::ComponentValidated,
                http_queue: crate::runner_runtime::HttpQueueClass::Empty,
                component: ComponentLoadClass::Loadable,
                tuning: WasmTuningClass::Default,
                socket_policy: SocketPolicyClass::NotApplicable,
            },
            plugin: PluginCapabilitySpec::new().initial_state(),
            shutdown: ShutdownFlowSpec::new().initial_state(),
        };
        let next = ImagodSystemState {
            runtime: RunnerRuntimeState {
                phase: RuntimePhase::Serving,
                ..prev.runtime
            },
            ..prev.clone()
        };
        assert!(!spec.contains_transition(
            &prev,
            &ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing),
            &next,
        ));
    }

    #[test]
    fn begin_shutdown_propagates_manager_and_transport_links() {
        let spec = ImagodSystemSpec::new();
        let prev = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::Listening,
                config_loaded: true,
                created_default: false,
                plugin_gc: TaskState::Succeeded,
                boot_restore: TaskState::Succeeded,
            },
            ..spec.initial_state()
        };
        let next = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::ShutdownRequested,
                ..prev.manager
            },
            transport: SessionTransportState {
                shutdown_requested: true,
                last_outcome: SessionOutcome::None,
                ..prev.transport
            },
            shutdown: ShutdownFlowState {
                phase: ShutdownPhase::SignalReceived,
                ..prev.shutdown
            },
            ..prev.clone()
        };
        assert!(spec.contains_transition(
            &prev,
            &ImagodSystemAction::Manager(ManagerShellAction::BeginShutdown),
            &next,
        ));
    }

    #[test]
    fn reachable_graph_contains_listening_serving_and_completed_checkpoints() {
        let spec = ImagodSystemSpec::new();
        let snapshot = ModelChecker::new(&spec)
            .reachable_graph_snapshot()
            .expect("reachable graph snapshot");

        assert!(snapshot.states.iter().any(|state| {
            matches!(state.manager.phase, ManagerShellPhase::Listening)
                && matches!(state.deploy.release, ReleaseStage::Promoted)
        }));
        assert!(snapshot.states.iter().any(|state| {
            matches!(state.runtime.phase, RuntimePhase::Serving)
                && state.bootstrap.ready
                && matches!(state.supervision.phase, ServicePhase::Running)
        }));
        assert!(snapshot.states.iter().any(|state| {
            matches!(state.shutdown.phase, ShutdownPhase::Completed)
                && matches!(state.manager.phase, ManagerShellPhase::Stopped)
        }));
    }
}
