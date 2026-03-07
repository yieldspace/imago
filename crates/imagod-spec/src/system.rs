use imago_protocol::{CommandKind, CommandLifecycleState, CommandProtocolAction};
use nirvash_core::{
    BoundedDomain, Fairness, Ltl, Signature, StatePredicate, StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    Signature as FormalSignature, fairness, illegal, invariant, property, system_spec,
};

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

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
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

impl ImagodSystemStateSignatureSpec for ImagodSystemState {
    fn representatives() -> BoundedDomain<Self> {
        let spec = ImagodSystemSpec::new();
        let init = spec.initial_state();
        let manager_config_ready = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::ConfigReady,
                config_loaded: true,
                created_default: false,
                plugin_gc: crate::manager_shell::TaskState::NotStarted,
                boot_restore: crate::manager_shell::TaskState::NotStarted,
            },
            ..init.clone()
        };
        let manager_restoring = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::Restoring,
                config_loaded: true,
                created_default: false,
                plugin_gc: crate::manager_shell::TaskState::Succeeded,
                boot_restore: crate::manager_shell::TaskState::NotStarted,
            },
            ..init.clone()
        };
        let manager_listening = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::Listening,
                config_loaded: true,
                created_default: false,
                plugin_gc: crate::manager_shell::TaskState::Succeeded,
                boot_restore: crate::manager_shell::TaskState::Succeeded,
            },
            ..init.clone()
        };
        let upload_partial = ImagodSystemState {
            deploy: ArtifactDeployState {
                upload: crate::artifact_deploy::UploadStage::Partial,
                release: ReleaseStage::None,
                precondition_ok: false,
                auto_rollback: true,
                chunks: crate::bounds::ArtifactChunks::new(1).expect("within bounds"),
            },
            ..manager_listening.clone()
        };
        let upload_complete = ImagodSystemState {
            deploy: ArtifactDeployState {
                upload: crate::artifact_deploy::UploadStage::Complete,
                ..upload_partial.deploy
            },
            ..upload_partial.clone()
        };
        let upload_committed = ImagodSystemState {
            deploy: ArtifactDeployState {
                upload: crate::artifact_deploy::UploadStage::Committed,
                ..upload_complete.deploy
            },
            ..upload_complete.clone()
        };
        let deploy_prepared = ImagodSystemState {
            deploy: ArtifactDeployState {
                release: ReleaseStage::Prepared,
                precondition_ok: true,
                ..upload_committed.deploy
            },
            ..upload_committed.clone()
        };
        let deploy_promoted = ImagodSystemState {
            deploy: ArtifactDeployState {
                release: ReleaseStage::Promoted,
                ..deploy_prepared.deploy
            },
            ..deploy_prepared.clone()
        };
        let service_starting = ImagodSystemState {
            supervision: ServiceSupervisionState {
                active_services: crate::bounds::ServiceSlots::new(1).expect("within bounds"),
                phase: ServicePhase::Starting,
                retained_logs: false,
            },
            ..deploy_promoted.clone()
        };
        let service_waiting_ready = ImagodSystemState {
            supervision: ServiceSupervisionState {
                phase: ServicePhase::WaitingReady,
                ..service_starting.supervision
            },
            ..service_starting.clone()
        };
        let bootstrap_decoded = ImagodSystemState {
            bootstrap: RunnerBootstrapState {
                size: crate::runner_bootstrap::BootstrapSizeClass::WithinBounds,
                decoded: true,
                app_type: Some(imagod_ipc::RunnerAppType::Rpc),
                endpoint: crate::runner_bootstrap::EndpointState::Missing,
                auth: crate::runner_bootstrap::AuthProofState::Pending,
                registered: false,
                ready: false,
            },
            ..service_waiting_ready.clone()
        };
        let bootstrap_endpoint_prepared = ImagodSystemState {
            bootstrap: RunnerBootstrapState {
                endpoint: crate::runner_bootstrap::EndpointState::Prepared,
                ..bootstrap_decoded.bootstrap
            },
            ..bootstrap_decoded.clone()
        };
        let runner_registered = ImagodSystemState {
            bootstrap: RunnerBootstrapState {
                endpoint: crate::runner_bootstrap::EndpointState::Prepared,
                auth: crate::runner_bootstrap::AuthProofState::Verified,
                registered: true,
                ..bootstrap_endpoint_prepared.bootstrap
            },
            ..bootstrap_endpoint_prepared.clone()
        };
        let runner_ready_running = ImagodSystemState {
            bootstrap: RunnerBootstrapState {
                ready: true,
                ..runner_registered.bootstrap
            },
            supervision: ServiceSupervisionState {
                phase: ServicePhase::Running,
                ..runner_registered.supervision
            },
            ..runner_registered.clone()
        };
        let runtime_mode_selected = ImagodSystemState {
            runtime: RunnerRuntimeState {
                mode: Some(imagod_ipc::RunnerAppType::Rpc),
                phase: RuntimePhase::Idle,
                http_queue_depth: crate::bounds::HttpQueueDepth::new(0).expect("within bounds"),
                epoch_ticks: crate::bounds::EpochTicks::new(0).expect("within bounds"),
                component: crate::runner_runtime::ComponentLoadClass::Unknown,
                tuning: crate::runner_runtime::WasmTuningClass::Default,
                socket_policy: crate::runner_runtime::SocketPolicyClass::NotApplicable,
            },
            ..runner_ready_running.clone()
        };
        let runtime_component_validated = ImagodSystemState {
            runtime: RunnerRuntimeState {
                phase: RuntimePhase::ComponentValidated,
                component: crate::runner_runtime::ComponentLoadClass::Loadable,
                ..runtime_mode_selected.runtime
            },
            ..runtime_mode_selected.clone()
        };
        let runtime_serving = ImagodSystemState {
            runtime: RunnerRuntimeState {
                phase: RuntimePhase::Serving,
                ..runtime_component_validated.runtime
            },
            ..runtime_component_validated.clone()
        };
        let shutdown_signal_received = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::ShutdownRequested,
                ..runtime_serving.manager
            },
            transport: SessionTransportState {
                shutdown_requested: true,
                last_outcome: crate::session_transport::SessionOutcome::None,
                ..runtime_serving.transport
            },
            shutdown: ShutdownFlowState {
                phase: ShutdownPhase::SignalReceived,
                ..runtime_serving.shutdown
            },
            ..runtime_serving.clone()
        };
        let shutdown_draining_sessions = ImagodSystemState {
            shutdown: ShutdownFlowState {
                phase: ShutdownPhase::DrainingSessions,
                accepts_stopped: true,
                ..shutdown_signal_received.shutdown
            },
            ..shutdown_signal_received.clone()
        };
        let shutdown_stopping_services = ImagodSystemState {
            shutdown: ShutdownFlowState {
                phase: ShutdownPhase::StoppingServices,
                sessions_drained: true,
                ..shutdown_draining_sessions.shutdown
            },
            ..shutdown_draining_sessions.clone()
        };
        let shutdown_stopping_maintenance = ImagodSystemState {
            shutdown: ShutdownFlowState {
                phase: ShutdownPhase::StoppingMaintenance,
                services_stopped: true,
                ..shutdown_stopping_services.shutdown
            },
            ..shutdown_stopping_services.clone()
        };
        let shutdown_maintenance_stopped = ImagodSystemState {
            shutdown: ShutdownFlowState {
                maintenance_stopped: true,
                ..shutdown_stopping_maintenance.shutdown
            },
            ..shutdown_stopping_maintenance.clone()
        };
        let shutdown_completed = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::Stopped,
                ..shutdown_maintenance_stopped.manager
            },
            shutdown: ShutdownFlowState {
                phase: ShutdownPhase::Completed,
                ..shutdown_maintenance_stopped.shutdown
            },
            ..shutdown_maintenance_stopped.clone()
        };
        BoundedDomain::new(vec![
            init,
            manager_config_ready,
            manager_restoring,
            manager_listening,
            upload_partial,
            upload_complete,
            upload_committed,
            deploy_prepared,
            deploy_promoted,
            service_starting,
            service_waiting_ready,
            bootstrap_decoded,
            bootstrap_endpoint_prepared,
            runner_registered,
            runner_ready_running,
            runtime_mode_selected,
            runtime_component_validated,
            runtime_serving,
            shutdown_signal_received,
            shutdown_draining_sessions,
            shutdown_stopping_services,
            shutdown_stopping_maintenance,
            shutdown_maintenance_stopped,
            shutdown_completed,
        ])
    }

    fn signature_invariant(&self) -> bool {
        self.manager.invariant()
            && self.transport.invariant()
            && self.command.invariant()
            && self.deploy.invariant()
            && self.supervision.invariant()
            && self.bootstrap.invariant()
            && self.runtime.invariant()
            && self.plugin.invariant()
            && self.shutdown.invariant()
            && cross_links_hold(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
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

#[illegal(ImagodSystemSpec)]
fn serve_before_runner_ready() -> StepPredicate<ImagodSystemState, ImagodSystemAction> {
    StepPredicate::new("serve_before_runner_ready", |prev, action, _| {
        matches!(
            action,
            ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing)
        ) && !prev.bootstrap.ready
    })
}

#[illegal(ImagodSystemSpec)]
fn accept_after_shutdown() -> StepPredicate<ImagodSystemState, ImagodSystemAction> {
    StepPredicate::new("accept_after_shutdown", |prev, action, _| {
        matches!(
            action,
            ImagodSystemAction::Session(SessionTransportAction::AcceptSession)
        ) && prev.transport.shutdown_requested
    })
}

#[illegal(ImagodSystemSpec)]
fn runner_ready_without_registration() -> StepPredicate<ImagodSystemState, ImagodSystemAction> {
    StepPredicate::new("runner_ready_without_registration", |prev, action, _| {
        matches!(
            action,
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::MarkReady)
        ) && !prev.bootstrap.registered
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

#[system_spec(subsystems(
    "manager_shell",
    "session_transport",
    "command_protocol",
    "artifact_deploy",
    "service_supervision",
    "runner_bootstrap",
    "runner_runtime",
    "plugin_capability",
    "shutdown_flow"
))]
impl TransitionSystem for ImagodSystemSpec {
    type State = ImagodSystemState;
    type Action = ImagodSystemAction;

    fn name(&self) -> &'static str {
        "imagod_system"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let manager_spec = ManagerShellSpec::new();
        let session_spec = SessionTransportSpec::new();
        let command_spec = CommandProtocolSpec::new();
        let deploy_spec = ArtifactDeploySpec::new();
        let supervision_spec = ServiceSupervisionSpec::new();
        let bootstrap_spec = RunnerBootstrapSpec::new();
        let runtime_spec = RunnerRuntimeSpec::new();
        let plugin_spec = PluginCapabilitySpec::new();
        let shutdown_spec = ShutdownFlowSpec::new();

        let mut candidate = prev.clone();
        match action {
            ImagodSystemAction::Manager(manager_action) => {
                if !manager_spec.next(&prev.manager, manager_action, &next.manager) {
                    return false;
                }
                candidate.manager = next.manager;

                if matches!(manager_action, ManagerShellAction::BeginShutdown) {
                    let transport_next = SessionTransportState {
                        shutdown_requested: true,
                        last_outcome: crate::session_transport::SessionOutcome::None,
                        ..prev.transport
                    };
                    if !session_spec.next(
                        &prev.transport,
                        &SessionTransportAction::BeginShutdown,
                        &transport_next,
                    ) {
                        return false;
                    }
                    candidate.transport = transport_next;

                    let shutdown_next = ShutdownFlowState {
                        phase: ShutdownPhase::SignalReceived,
                        ..prev.shutdown
                    };
                    if !shutdown_spec.next(
                        &prev.shutdown,
                        &ShutdownFlowAction::ReceiveSignal,
                        &shutdown_next,
                    ) {
                        return false;
                    }
                    candidate.shutdown = shutdown_next;
                }
            }
            ImagodSystemAction::Session(session_action) => {
                if !session_spec.next(&prev.transport, session_action, &next.transport) {
                    return false;
                }
                candidate.transport = next.transport;
            }
            ImagodSystemAction::Command(command_action) => {
                if matches!(
                    command_action,
                    CommandProtocolAction::Start(
                        CommandKind::Deploy | CommandKind::Run | CommandKind::Stop
                    )
                ) && !matches!(prev.manager.phase, ManagerShellPhase::Listening)
                {
                    return false;
                }
                if !command_spec.next(&prev.command, command_action, &next.command) {
                    return false;
                }
                candidate.command = next.command;
            }
            ImagodSystemAction::Deploy(deploy_action) => {
                if !deploy_spec.next(&prev.deploy, deploy_action, &next.deploy) {
                    return false;
                }
                candidate.deploy = next.deploy;
            }
            ImagodSystemAction::Supervision(supervision_action) => {
                if matches!(supervision_action, ServiceSupervisionAction::StartService)
                    && (!matches!(prev.manager.phase, ManagerShellPhase::Listening)
                        || !matches!(prev.deploy.release, ReleaseStage::Promoted))
                {
                    return false;
                }
                if !supervision_spec.next(&prev.supervision, supervision_action, &next.supervision)
                {
                    return false;
                }
                candidate.supervision = next.supervision;
            }
            ImagodSystemAction::Bootstrap(bootstrap_action) => {
                if !bootstrap_spec.next(&prev.bootstrap, bootstrap_action, &next.bootstrap) {
                    return false;
                }
                candidate.bootstrap = next.bootstrap;

                if matches!(bootstrap_action, RunnerBootstrapAction::MarkReady) {
                    if !matches!(prev.supervision.phase, ServicePhase::WaitingReady) {
                        return false;
                    }
                    let supervision_next = ServiceSupervisionState {
                        phase: ServicePhase::Running,
                        ..prev.supervision
                    };
                    if !supervision_spec.next(
                        &prev.supervision,
                        &ServiceSupervisionAction::MarkRunnerReady,
                        &supervision_next,
                    ) {
                        return false;
                    }
                    candidate.supervision = supervision_next;
                }
            }
            ImagodSystemAction::Runtime(runtime_action) => {
                if matches!(runtime_action, RunnerRuntimeAction::StartServing)
                    && (!prev.bootstrap.ready
                        || !matches!(prev.supervision.phase, ServicePhase::Running)
                        || !matches!(prev.deploy.release, ReleaseStage::Promoted))
                {
                    return false;
                }
                if !runtime_spec.next(&prev.runtime, runtime_action, &next.runtime) {
                    return false;
                }
                candidate.runtime = next.runtime;
            }
            ImagodSystemAction::Plugin(plugin_action) => {
                if !plugin_spec.next(&prev.plugin, plugin_action, &next.plugin) {
                    return false;
                }
                candidate.plugin = next.plugin.clone();
            }
            ImagodSystemAction::Shutdown(shutdown_action) => match shutdown_action {
                ShutdownFlowAction::ReceiveSignal => {
                    let manager_next = ManagerShellState {
                        phase: ManagerShellPhase::ShutdownRequested,
                        ..prev.manager
                    };
                    if !manager_spec.next(
                        &prev.manager,
                        &ManagerShellAction::BeginShutdown,
                        &manager_next,
                    ) {
                        return false;
                    }
                    candidate.manager = manager_next;

                    let transport_next = SessionTransportState {
                        shutdown_requested: true,
                        last_outcome: crate::session_transport::SessionOutcome::None,
                        ..prev.transport
                    };
                    if !session_spec.next(
                        &prev.transport,
                        &SessionTransportAction::BeginShutdown,
                        &transport_next,
                    ) {
                        return false;
                    }
                    candidate.transport = transport_next;

                    if !shutdown_spec.next(&prev.shutdown, shutdown_action, &next.shutdown) {
                        return false;
                    }
                    candidate.shutdown = next.shutdown;
                }
                ShutdownFlowAction::Finalize => {
                    if !shutdown_spec.next(&prev.shutdown, shutdown_action, &next.shutdown) {
                        return false;
                    }
                    candidate.shutdown = next.shutdown;

                    let manager_next = ManagerShellState {
                        phase: ManagerShellPhase::Stopped,
                        ..prev.manager
                    };
                    if !manager_spec.next(
                        &prev.manager,
                        &ManagerShellAction::FinishShutdown,
                        &manager_next,
                    ) {
                        return false;
                    }
                    candidate.manager = manager_next;
                }
                _ => {
                    if !shutdown_spec.next(&prev.shutdown, shutdown_action, &next.shutdown) {
                        return false;
                    }
                    candidate.shutdown = next.shutdown;
                }
            },
        }

        candidate == *next && candidate.invariant()
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
    init = initial_state,
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
                http_queue_depth: crate::bounds::HttpQueueDepth::new(0).expect("within bounds"),
                epoch_ticks: crate::bounds::EpochTicks::new(0).expect("within bounds"),
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
        assert!(!spec.next(
            &prev,
            &ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing),
            &next,
        ));
    }

    #[test]
    fn shutdown_signal_propagates_manager_and_transport_links() {
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
        assert!(spec.next(
            &prev,
            &ImagodSystemAction::Shutdown(ShutdownFlowAction::ReceiveSignal),
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
