use imago_protocol::{CommandErrorKind, CommandKind, CommandLifecycleState, CommandProtocolAction};
use imagod_ipc::{PluginKind, RunnerAppType};
use nirvash_core::{
    ActionConstraint, ModelCase, ModelCaseSource, ModelCheckConfig, StatePredicate, TemporalSpec,
    TransitionSystem,
};
use nirvash_macros::{invariant, system_spec};

use crate::{
    artifact_deploy::{
        ArtifactDeployAction, ArtifactDeploySpec, ArtifactDeployState, ReleaseStage,
    },
    command_protocol::CommandProtocolSpec,
    manager_shell::{ManagerShellAction, ManagerShellPhase, ManagerShellSpec, ManagerShellState},
    plugin_capability::{PluginCapabilityAction, PluginCapabilitySpec, PluginCapabilityState},
    runner_bootstrap::{RunnerBootstrapAction, RunnerBootstrapSpec, RunnerBootstrapState},
    runner_runtime::{RunnerRuntimeAction, RunnerRuntimeSpec, RunnerRuntimeState, RuntimePhase},
    service_supervision::{
        ServiceSupervisionAction, ServiceSupervisionSpec, ServiceSupervisionState,
    },
    session_transport::{
        SessionOutcome, SessionTransportAction, SessionTransportSpec, SessionTransportState,
    },
    shutdown_flow::{ShutdownFlowAction, ShutdownFlowSpec, ShutdownFlowState, ShutdownPhase},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagodSystemState {
    pub manager: ManagerShellState,
    pub transport: SessionTransportState,
    pub command: crate::command_protocol::CommandProtocolState,
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
            ImagodSystemAction::Manager(ManagerShellAction::CreateDefaultConfig),
            ImagodSystemAction::Manager(ManagerShellAction::RunPluginGcSucceeded),
            ImagodSystemAction::Manager(ManagerShellAction::RunPluginGcFailed),
            ImagodSystemAction::Manager(ManagerShellAction::RunBootRestoreSucceeded),
            ImagodSystemAction::Manager(ManagerShellAction::RunBootRestoreFailed),
            ImagodSystemAction::Manager(ManagerShellAction::StartListening),
            ImagodSystemAction::Command(CommandProtocolAction::Start(CommandKind::Run)),
            ImagodSystemAction::Command(CommandProtocolAction::SetRunning),
            ImagodSystemAction::Command(CommandProtocolAction::RequestCancel),
            ImagodSystemAction::Command(CommandProtocolAction::MarkSpawned),
            ImagodSystemAction::Command(CommandProtocolAction::FinishCanceled),
            ImagodSystemAction::Command(CommandProtocolAction::FinishFailed(
                CommandErrorKind::Internal,
            )),
            ImagodSystemAction::Command(CommandProtocolAction::Remove),
            ImagodSystemAction::Session(SessionTransportAction::AcceptSession),
            ImagodSystemAction::Session(SessionTransportAction::RejectTooMany),
            ImagodSystemAction::Session(SessionTransportAction::JoinSession),
            ImagodSystemAction::Deploy(ArtifactDeployAction::ReceiveChunk),
            ImagodSystemAction::Deploy(ArtifactDeployAction::CompleteUpload),
            ImagodSystemAction::Deploy(ArtifactDeployAction::CommitUpload),
            ImagodSystemAction::Deploy(ArtifactDeployAction::StartDeployMatched),
            ImagodSystemAction::Deploy(ArtifactDeployAction::StartDeployMismatched),
            ImagodSystemAction::Deploy(ArtifactDeployAction::PromoteRelease),
            ImagodSystemAction::Deploy(ArtifactDeployAction::TriggerRollback),
            ImagodSystemAction::Deploy(ArtifactDeployAction::FinishRollback),
            ImagodSystemAction::Supervision(ServiceSupervisionAction::StartService),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::ReadWithinBounds),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::ReadOversized),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::DecodeBootstrap(
                RunnerAppType::Rpc,
            )),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::PrepareEndpoint),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::RegisterRunner),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::RejectAuthProof),
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::MarkReady),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::SelectMode(RunnerAppType::Rpc)),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::ApplyDefaultTuning),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::ApplyInvalidTuning),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::ValidateComponentLoadable),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::ValidateComponentInvalid),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing),
            ImagodSystemAction::Runtime(RunnerRuntimeAction::FailRuntime),
            ImagodSystemAction::Plugin(PluginCapabilityAction::RegisterPlugin(PluginKind::Wasm)),
            ImagodSystemAction::Plugin(PluginCapabilityAction::ClassifyGraphAcyclic),
            ImagodSystemAction::Plugin(PluginCapabilityAction::ClassifyGraphMissingDependency),
            ImagodSystemAction::Plugin(PluginCapabilityAction::ResolveProviderSelf),
            ImagodSystemAction::Plugin(PluginCapabilityAction::ResolveProviderDependency),
            ImagodSystemAction::Plugin(PluginCapabilityAction::ResolveProviderMissing),
            ImagodSystemAction::Plugin(PluginCapabilityAction::AllowCapability),
            ImagodSystemAction::Plugin(PluginCapabilityAction::GrantPrivilegedCapability),
            ImagodSystemAction::Plugin(PluginCapabilityAction::AllowHttpHost),
            ImagodSystemAction::Plugin(PluginCapabilityAction::DenyHttpOutbound),
            ImagodSystemAction::Manager(ManagerShellAction::BeginShutdown),
            ImagodSystemAction::Shutdown(ShutdownFlowAction::StopAccepting),
            ImagodSystemAction::Shutdown(ShutdownFlowAction::DrainSessions),
            ImagodSystemAction::Shutdown(ShutdownFlowAction::StopServicesGraceful),
            ImagodSystemAction::Shutdown(ShutdownFlowAction::StopServicesForced),
            ImagodSystemAction::Shutdown(ShutdownFlowAction::StopMaintenance),
            ImagodSystemAction::Shutdown(ShutdownFlowAction::Finalize),
            ImagodSystemAction::Manager(ManagerShellAction::FinishShutdown),
        ]
    }

    fn transition_state(
        &self,
        prev: &ImagodSystemState,
        action: &ImagodSystemAction,
    ) -> Option<ImagodSystemState> {
        let manager_spec = ManagerShellSpec::new();
        let transport_spec = SessionTransportSpec::new();
        let command_spec = CommandProtocolSpec::new();
        let deploy_spec = ArtifactDeploySpec::new();
        let supervision_spec = ServiceSupervisionSpec::new();
        let bootstrap_spec = RunnerBootstrapSpec::new();
        let runtime_spec = RunnerRuntimeSpec::new();
        let plugin_spec = PluginCapabilitySpec::new();
        let shutdown_spec = ShutdownFlowSpec::new();

        let mut candidate = prev.clone();
        match action {
            ImagodSystemAction::Manager(ManagerShellAction::BeginShutdown) => {
                candidate.manager =
                    manager_spec.transition(&prev.manager, &ManagerShellAction::BeginShutdown)?;
                candidate.transport = transport_spec
                    .transition(&prev.transport, &SessionTransportAction::BeginShutdown)?;
                candidate.shutdown =
                    shutdown_spec.transition(&prev.shutdown, &ShutdownFlowAction::ReceiveSignal)?;
            }
            ImagodSystemAction::Manager(ManagerShellAction::FinishShutdown) => {
                if !matches!(prev.shutdown.phase, ShutdownPhase::Completed) {
                    return None;
                }
                candidate.manager =
                    manager_spec.transition(&prev.manager, &ManagerShellAction::FinishShutdown)?;
            }
            ImagodSystemAction::Manager(manager_action) => {
                candidate.manager = manager_spec.transition(&prev.manager, manager_action)?;
            }
            ImagodSystemAction::Session(session_action) => {
                candidate.transport = transport_spec.transition(&prev.transport, session_action)?;
            }
            ImagodSystemAction::Command(command_action) => {
                candidate.command = command_spec.transition(&prev.command, command_action)?;
            }
            ImagodSystemAction::Deploy(deploy_action) => {
                candidate.deploy = deploy_spec.transition(&prev.deploy, deploy_action)?;
            }
            ImagodSystemAction::Supervision(supervision_action) => {
                candidate.supervision =
                    supervision_spec.transition(&prev.supervision, supervision_action)?;
            }
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::RegisterRunner) => {
                candidate.bootstrap = bootstrap_spec
                    .transition(&prev.bootstrap, &RunnerBootstrapAction::RegisterRunner)?;
                candidate.supervision = supervision_spec
                    .transition(&prev.supervision, &ServiceSupervisionAction::RegisterRunner)?;
            }
            ImagodSystemAction::Bootstrap(RunnerBootstrapAction::MarkReady) => {
                candidate.bootstrap = bootstrap_spec
                    .transition(&prev.bootstrap, &RunnerBootstrapAction::MarkReady)?;
                candidate.supervision = supervision_spec.transition(
                    &prev.supervision,
                    &ServiceSupervisionAction::MarkRunnerReady,
                )?;
            }
            ImagodSystemAction::Bootstrap(bootstrap_action) => {
                candidate.bootstrap =
                    bootstrap_spec.transition(&prev.bootstrap, bootstrap_action)?;
            }
            ImagodSystemAction::Runtime(runtime_action) => {
                candidate.runtime = runtime_spec.transition(&prev.runtime, runtime_action)?;
            }
            ImagodSystemAction::Plugin(plugin_action) => {
                candidate.plugin = plugin_spec.transition(&prev.plugin, plugin_action)?;
            }
            ImagodSystemAction::Shutdown(shutdown_action) => {
                candidate.shutdown = shutdown_spec.transition(&prev.shutdown, shutdown_action)?;
            }
        }

        system_state_valid(&candidate).then_some(candidate)
    }
}

fn system_model_cases() -> Vec<ModelCase<ImagodSystemState, ImagodSystemAction>> {
    vec![
        startup_command_model_case(),
        deploy_runtime_serving_model_case(),
        deploy_rollback_model_case(),
        bootstrap_runtime_failure_model_case(),
        plugin_dependency_model_case(),
        shutdown_model_case(),
    ]
}

fn startup_command_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("startup_command")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_action_constraint(startup_command_action_constraint())
}

fn deploy_runtime_serving_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("deploy_runtime_serving")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_action_constraint(deploy_runtime_serving_action_constraint())
}

fn deploy_rollback_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("deploy_rollback")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_action_constraint(deploy_rollback_action_constraint())
}

fn bootstrap_runtime_failure_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("bootstrap_runtime_failure")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_action_constraint(bootstrap_runtime_failure_action_constraint())
}

fn plugin_dependency_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("plugin_dependency")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_action_constraint(plugin_dependency_action_constraint())
}

fn shutdown_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("shutdown")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_action_constraint(shutdown_action_constraint())
}

fn system_checker_config() -> ModelCheckConfig {
    ModelCheckConfig::reachable_graph()
}

fn system_doc_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        exploration: nirvash_core::ExplorationMode::ReachableGraph,
        bounded_depth: None,
        max_states: Some(128),
        max_transitions: Some(384),
        check_deadlocks: true,
        stop_on_first_violation: false,
    }
}

fn startup_command_action_constraint() -> ActionConstraint<ImagodSystemState, ImagodSystemAction> {
    ActionConstraint::new("startup_command_actions", |prev, action, _| {
        startup_command_session_progress_allowed(prev, action)
            || startup_command_non_session_action_allowed(action)
    })
}

fn deploy_runtime_serving_action_constraint()
-> ActionConstraint<ImagodSystemState, ImagodSystemAction> {
    ActionConstraint::new("deploy_runtime_serving_actions", |prev, action, _| {
        deterministic_session_cycle_allowed(prev, action)
            || deploy_runtime_serving_non_session_action_allowed(action)
    })
}

fn deploy_rollback_action_constraint() -> ActionConstraint<ImagodSystemState, ImagodSystemAction> {
    ActionConstraint::new("deploy_rollback_actions", |prev, action, _| {
        deterministic_session_cycle_allowed(prev, action)
            || deploy_rollback_non_session_action_allowed(action)
    })
}

fn bootstrap_runtime_failure_action_constraint()
-> ActionConstraint<ImagodSystemState, ImagodSystemAction> {
    ActionConstraint::new("bootstrap_runtime_failure_actions", |prev, action, _| {
        deterministic_session_cycle_allowed(prev, action)
            || bootstrap_runtime_failure_non_session_action_allowed(action)
    })
}

fn plugin_dependency_action_constraint() -> ActionConstraint<ImagodSystemState, ImagodSystemAction>
{
    ActionConstraint::new("plugin_dependency_actions", |prev, action, _| {
        deterministic_session_cycle_allowed(prev, action)
            || plugin_dependency_non_session_action_allowed(action)
    })
}

fn shutdown_action_constraint() -> ActionConstraint<ImagodSystemState, ImagodSystemAction> {
    ActionConstraint::new("shutdown_actions", |prev, action, _| {
        shutdown_session_progress_allowed(prev, action)
            || shutdown_non_session_action_allowed(action)
    })
}

fn deterministic_session_cycle_allowed(
    prev: &ImagodSystemState,
    action: &ImagodSystemAction,
) -> bool {
    matches!(
        (prev.transport.active_session_count(), action),
        (
            0,
            ImagodSystemAction::Session(SessionTransportAction::AcceptSession)
        ) | (
            1,
            ImagodSystemAction::Session(SessionTransportAction::JoinSession)
        )
    )
}

fn startup_command_non_session_action_allowed(action: &ImagodSystemAction) -> bool {
    matches!(
        action,
        ImagodSystemAction::Manager(ManagerShellAction::LoadExistingConfig)
            | ImagodSystemAction::Manager(ManagerShellAction::CreateDefaultConfig)
            | ImagodSystemAction::Manager(ManagerShellAction::RunPluginGcSucceeded)
            | ImagodSystemAction::Manager(ManagerShellAction::RunPluginGcFailed)
            | ImagodSystemAction::Manager(ManagerShellAction::RunBootRestoreSucceeded)
            | ImagodSystemAction::Manager(ManagerShellAction::RunBootRestoreFailed)
            | ImagodSystemAction::Manager(ManagerShellAction::StartListening)
            | ImagodSystemAction::Command(CommandProtocolAction::Start(CommandKind::Run))
            | ImagodSystemAction::Command(CommandProtocolAction::SetRunning)
            | ImagodSystemAction::Command(CommandProtocolAction::RequestCancel)
            | ImagodSystemAction::Command(CommandProtocolAction::MarkSpawned)
            | ImagodSystemAction::Command(CommandProtocolAction::FinishCanceled)
            | ImagodSystemAction::Command(CommandProtocolAction::FinishFailed(
                CommandErrorKind::Internal
            ))
            | ImagodSystemAction::Command(CommandProtocolAction::Remove)
    )
}

fn startup_command_session_progress_allowed(
    prev: &ImagodSystemState,
    action: &ImagodSystemAction,
) -> bool {
    matches!(
        (
            prev.transport.active_session_count(),
            prev.transport.last_outcome,
            action,
        ),
        (
            0,
            _,
            ImagodSystemAction::Session(SessionTransportAction::AcceptSession)
        ) | (
            1,
            SessionOutcome::Accepted,
            ImagodSystemAction::Session(SessionTransportAction::AcceptSession),
        ) | (
            1,
            SessionOutcome::Joined,
            ImagodSystemAction::Session(SessionTransportAction::JoinSession),
        ) | (
            2,
            SessionOutcome::Accepted,
            ImagodSystemAction::Session(SessionTransportAction::RejectTooMany),
        ) | (
            2,
            SessionOutcome::RejectedTooMany,
            ImagodSystemAction::Session(SessionTransportAction::JoinSession),
        )
    )
}

fn deploy_runtime_serving_non_session_action_allowed(action: &ImagodSystemAction) -> bool {
    matches!(
        action,
        ImagodSystemAction::Deploy(ArtifactDeployAction::ReceiveChunk)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::CompleteUpload)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::CommitUpload)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::StartDeployMatched)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::PromoteRelease)
            | ImagodSystemAction::Supervision(ServiceSupervisionAction::StartService)
            | ImagodSystemAction::Bootstrap(RunnerBootstrapAction::DecodeBootstrap(
                RunnerAppType::Rpc
            ))
            | ImagodSystemAction::Bootstrap(RunnerBootstrapAction::PrepareEndpoint)
            | ImagodSystemAction::Bootstrap(RunnerBootstrapAction::RegisterRunner)
            | ImagodSystemAction::Bootstrap(RunnerBootstrapAction::MarkReady)
            | ImagodSystemAction::Runtime(RunnerRuntimeAction::SelectMode(RunnerAppType::Rpc))
            | ImagodSystemAction::Runtime(RunnerRuntimeAction::ValidateComponentLoadable)
            | ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing)
    )
}

fn bootstrap_runtime_failure_non_session_action_allowed(action: &ImagodSystemAction) -> bool {
    matches!(
        action,
        ImagodSystemAction::Bootstrap(RunnerBootstrapAction::ReadOversized)
            | ImagodSystemAction::Bootstrap(RunnerBootstrapAction::DecodeBootstrap(
                RunnerAppType::Rpc
            ))
            | ImagodSystemAction::Bootstrap(RunnerBootstrapAction::PrepareEndpoint)
            | ImagodSystemAction::Bootstrap(RunnerBootstrapAction::RejectAuthProof)
            | ImagodSystemAction::Runtime(RunnerRuntimeAction::SelectMode(RunnerAppType::Rpc))
            | ImagodSystemAction::Runtime(RunnerRuntimeAction::ApplyInvalidTuning)
            | ImagodSystemAction::Runtime(RunnerRuntimeAction::ValidateComponentInvalid)
            | ImagodSystemAction::Runtime(RunnerRuntimeAction::FailRuntime)
    )
}

fn deploy_rollback_non_session_action_allowed(action: &ImagodSystemAction) -> bool {
    matches!(
        action,
        ImagodSystemAction::Deploy(ArtifactDeployAction::ReceiveChunk)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::CompleteUpload)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::CommitUpload)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::StartDeployMatched)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::StartDeployMismatched)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::PromoteRelease)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::TriggerRollback)
            | ImagodSystemAction::Deploy(ArtifactDeployAction::FinishRollback)
    )
}

fn plugin_dependency_non_session_action_allowed(action: &ImagodSystemAction) -> bool {
    matches!(
        action,
        ImagodSystemAction::Plugin(PluginCapabilityAction::RegisterPlugin(PluginKind::Wasm))
            | ImagodSystemAction::Plugin(PluginCapabilityAction::ClassifyGraphAcyclic)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::ClassifyGraphMissingDependency)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::ResolveProviderSelf)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::ResolveProviderDependency)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::ResolveProviderMissing)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::AllowCapability)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::GrantPrivilegedCapability)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::AllowHttpHost)
            | ImagodSystemAction::Plugin(PluginCapabilityAction::DenyHttpOutbound)
    )
}

fn shutdown_non_session_action_allowed(action: &ImagodSystemAction) -> bool {
    matches!(
        action,
        ImagodSystemAction::Manager(ManagerShellAction::LoadExistingConfig)
            | ImagodSystemAction::Manager(ManagerShellAction::RunPluginGcSucceeded)
            | ImagodSystemAction::Manager(ManagerShellAction::RunBootRestoreSucceeded)
            | ImagodSystemAction::Manager(ManagerShellAction::BeginShutdown)
            | ImagodSystemAction::Manager(ManagerShellAction::FinishShutdown)
            | ImagodSystemAction::Shutdown(ShutdownFlowAction::StopAccepting)
            | ImagodSystemAction::Shutdown(ShutdownFlowAction::DrainSessions)
            | ImagodSystemAction::Shutdown(ShutdownFlowAction::StopServicesGraceful)
            | ImagodSystemAction::Shutdown(ShutdownFlowAction::StopServicesForced)
            | ImagodSystemAction::Shutdown(ShutdownFlowAction::StopMaintenance)
            | ImagodSystemAction::Shutdown(ShutdownFlowAction::Finalize)
    )
}

fn shutdown_session_progress_allowed(
    prev: &ImagodSystemState,
    action: &ImagodSystemAction,
) -> bool {
    if !prev.transport.shutdown_requested {
        return false;
    }

    matches!(
        (prev.transport.active_session_count(), action),
        (
            0,
            ImagodSystemAction::Session(SessionTransportAction::RejectTooMany)
        ) | (
            1 | 2,
            ImagodSystemAction::Session(SessionTransportAction::JoinSession)
        )
    )
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
                    && state.supervision.has_ready_service()
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
        !state.bootstrap.ready || state.supervision.has_ready_service()
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

#[invariant(ImagodSystemSpec)]
fn stopped_manager_requires_completed_shutdown() -> StatePredicate<ImagodSystemState> {
    StatePredicate::new("stopped_manager_requires_completed_shutdown", |state| {
        !matches!(state.manager.phase, ManagerShellPhase::Stopped)
            || matches!(state.shutdown.phase, ShutdownPhase::Completed)
    })
}

#[invariant(ImagodSystemSpec)]
fn dependency_provider_requires_acyclic_plugin_graph() -> StatePredicate<ImagodSystemState> {
    StatePredicate::new(
        "dependency_provider_requires_acyclic_plugin_graph",
        |state| !state.plugin.provider_is_dependency() || state.plugin.graph_is_acyclic(),
    )
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
            && state.supervision.has_ready_service()
            && matches!(state.deploy.release, ReleaseStage::Promoted)))
        && (matches!(state.shutdown.phase, ShutdownPhase::Idle)
            || (state.transport.shutdown_requested
                && matches!(
                    state.manager.phase,
                    ManagerShellPhase::ShutdownRequested | ManagerShellPhase::Stopped
                )))
        && (!state.bootstrap.ready || state.supervision.has_ready_service())
        && (!matches!(
            state.command.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        ) || matches!(state.manager.phase, ManagerShellPhase::Listening))
        && (!matches!(state.manager.phase, ManagerShellPhase::Stopped)
            || matches!(state.shutdown.phase, ShutdownPhase::Completed))
        && (!state.plugin.provider_is_dependency() || state.plugin.graph_is_acyclic())
}

#[nirvash_macros::formal_tests(
    spec = ImagodSystemSpec,
    composition = composition
)]
const _: () = ();

#[cfg(test)]
mod tests {
    use nirvash_core::{ModelCaseSource, ModelChecker};

    use super::*;
    use crate::{
        manager_shell::TaskState,
        runner_bootstrap::{AuthProofState, BootstrapSizeClass, EndpointState},
        runner_runtime::{ComponentLoadClass, WasmTuningClass},
        session_transport::SessionOutcome,
    };

    fn model_case(
        spec: &ImagodSystemSpec,
        label: &str,
    ) -> ModelCase<ImagodSystemState, ImagodSystemAction> {
        spec.model_cases()
            .into_iter()
            .find(|model_case| model_case.label() == label)
            .unwrap_or_else(|| panic!("missing system model case: {label}"))
    }

    fn reachable_snapshot_for_case(
        spec: &ImagodSystemSpec,
        label: &str,
    ) -> nirvash_core::ReachableGraphSnapshot<ImagodSystemState, ImagodSystemAction> {
        ModelChecker::for_case(spec, model_case(spec, label))
            .full_reachable_graph_snapshot()
            .expect("reachable graph snapshot")
    }

    fn listening_state(spec: &ImagodSystemSpec) -> ImagodSystemState {
        let config_ready = spec
            .transition(
                &spec.initial_state(),
                &ImagodSystemAction::Manager(ManagerShellAction::LoadExistingConfig),
            )
            .expect("config should load");
        let restoring = spec
            .transition(
                &config_ready,
                &ImagodSystemAction::Manager(ManagerShellAction::RunPluginGcSucceeded),
            )
            .expect("plugin gc should complete");
        spec.transition(
            &restoring,
            &ImagodSystemAction::Manager(ManagerShellAction::RunBootRestoreSucceeded),
        )
        .expect("boot restore should complete")
    }

    fn running_supervision_state() -> ServiceSupervisionState {
        let spec = ServiceSupervisionSpec::new();
        let starting = spec
            .transition(
                &spec.initial_state(),
                &ServiceSupervisionAction::StartService,
            )
            .expect("start service");
        let waiting = spec
            .transition(&starting, &ServiceSupervisionAction::RegisterRunner)
            .expect("register runner");
        spec.transition(&waiting, &ServiceSupervisionAction::MarkRunnerReady)
            .expect("mark runner ready")
    }

    fn component_validated_runtime_state(mode: RunnerAppType) -> RunnerRuntimeState {
        let spec = RunnerRuntimeSpec::new();
        let selected = spec
            .transition(
                &spec.initial_state(),
                &RunnerRuntimeAction::SelectMode(mode),
            )
            .expect("select runtime mode");
        spec.transition(&selected, &RunnerRuntimeAction::ValidateComponentLoadable)
            .expect("validate component")
    }

    #[test]
    fn runtime_cannot_start_serving_before_runner_ready() {
        let spec = ImagodSystemSpec::new();
        let runtime = component_validated_runtime_state(RunnerAppType::Rpc);
        let runtime_serving = RunnerRuntimeSpec::new()
            .transition(&runtime, &RunnerRuntimeAction::StartServing)
            .expect("runtime alone can serve after validation");
        let prev = ImagodSystemState {
            manager: ManagerShellState {
                phase: ManagerShellPhase::Listening,
                config_loaded: true,
                created_default: false,
                plugin_gc: TaskState::Succeeded,
                boot_restore: TaskState::Succeeded,
            },
            transport: SessionTransportSpec::new().initial_state(),
            command: CommandProtocolSpec::new().initial_state(),
            deploy: ArtifactDeployState {
                upload: crate::artifact_deploy::UploadStage::Committed,
                release: ReleaseStage::Promoted,
                precondition_ok: true,
                auto_rollback: true,
                chunks: crate::bounds::ArtifactChunks::new(2).expect("within bounds"),
            },
            supervision: running_supervision_state(),
            bootstrap: RunnerBootstrapState {
                size: BootstrapSizeClass::WithinBounds,
                decoded: true,
                app_type: Some(RunnerAppType::Rpc),
                endpoint: EndpointState::Prepared,
                auth: AuthProofState::Verified,
                registered: true,
                ready: false,
            },
            runtime,
            plugin: PluginCapabilitySpec::new().initial_state(),
            shutdown: ShutdownFlowSpec::new().initial_state(),
        };
        let next = ImagodSystemState {
            runtime: runtime_serving,
            ..prev.clone()
        };
        assert!(!spec.contains_transition(
            &prev,
            &ImagodSystemAction::Runtime(RunnerRuntimeAction::StartServing),
            &next,
        ));
    }

    #[test]
    fn startup_sequence_requires_config_and_restore_intermediates() {
        let spec = ImagodSystemSpec::new();
        let initial = spec.initial_state();
        let config_ready = spec
            .transition(
                &initial,
                &ImagodSystemAction::Manager(ManagerShellAction::LoadExistingConfig),
            )
            .expect("config load should advance to config ready");
        let restoring = spec
            .transition(
                &config_ready,
                &ImagodSystemAction::Manager(ManagerShellAction::RunPluginGcSucceeded),
            )
            .expect("plugin gc should advance to restoring");
        let listening = spec
            .transition(
                &restoring,
                &ImagodSystemAction::Manager(ManagerShellAction::RunBootRestoreSucceeded),
            )
            .expect("boot restore should advance to listening");

        assert!(matches!(
            config_ready.manager.phase,
            ManagerShellPhase::ConfigReady
        ));
        assert!(matches!(
            restoring.manager.phase,
            ManagerShellPhase::Restoring
        ));
        assert!(matches!(
            listening.manager.phase,
            ManagerShellPhase::Listening
        ));
        assert!(
            spec.transition(
                &initial,
                &ImagodSystemAction::Manager(ManagerShellAction::StartListening),
            )
            .is_none()
        );
    }

    #[test]
    fn shutdown_path_requires_explicit_manager_finish() {
        let spec = ImagodSystemSpec::new();
        let listening = listening_state(&spec);
        let signal_received = spec
            .transition(
                &listening,
                &ImagodSystemAction::Manager(ManagerShellAction::BeginShutdown),
            )
            .expect("begin shutdown should synchronize manager, transport, and shutdown");
        let draining = spec
            .transition(
                &signal_received,
                &ImagodSystemAction::Shutdown(ShutdownFlowAction::StopAccepting),
            )
            .expect("stop accepting should advance shutdown");
        let stopping_services = spec
            .transition(
                &draining,
                &ImagodSystemAction::Shutdown(ShutdownFlowAction::DrainSessions),
            )
            .expect("drain sessions should advance shutdown");
        let stopping_maintenance = spec
            .transition(
                &stopping_services,
                &ImagodSystemAction::Shutdown(ShutdownFlowAction::StopServicesGraceful),
            )
            .expect("service stop should advance shutdown");
        let maintenance_stopped = spec
            .transition(
                &stopping_maintenance,
                &ImagodSystemAction::Shutdown(ShutdownFlowAction::StopMaintenance),
            )
            .expect("maintenance stop should set maintenance flag");
        let completed = spec
            .transition(
                &maintenance_stopped,
                &ImagodSystemAction::Shutdown(ShutdownFlowAction::Finalize),
            )
            .expect("finalize should complete shutdown");
        let finished = spec
            .transition(
                &completed,
                &ImagodSystemAction::Manager(ManagerShellAction::FinishShutdown),
            )
            .expect("manager finish should stop manager after shutdown completion");

        assert!(matches!(
            signal_received.shutdown.phase,
            ShutdownPhase::SignalReceived
        ));
        assert!(signal_received.transport.shutdown_requested);
        assert!(matches!(
            signal_received.manager.phase,
            ManagerShellPhase::ShutdownRequested
        ));
        assert!(matches!(completed.shutdown.phase, ShutdownPhase::Completed));
        assert!(matches!(
            completed.manager.phase,
            ManagerShellPhase::ShutdownRequested
        ));
        assert!(matches!(finished.manager.phase, ManagerShellPhase::Stopped));
        assert!(
            spec.transition(
                &stopping_maintenance,
                &ImagodSystemAction::Manager(ManagerShellAction::FinishShutdown),
            )
            .is_none()
        );
    }

    #[test]
    fn startup_command_case_contains_cancel_failure_and_backpressure_paths() {
        let spec = ImagodSystemSpec::new();
        let snapshot = reachable_snapshot_for_case(&spec, "startup_command");

        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.manager.phase, ManagerShellPhase::ConfigReady))
        );
        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.manager.phase, ManagerShellPhase::Restoring))
        );
        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.manager.phase, ManagerShellPhase::Listening))
        );
        assert!(snapshot.states.iter().any(|state| {
            matches!(
                state.command.lifecycle_state,
                Some(CommandLifecycleState::Canceled)
            )
        }));
        assert!(snapshot.states.iter().any(|state| {
            matches!(
                state.command.lifecycle_state,
                Some(CommandLifecycleState::Failed)
            )
        }));
        assert!(snapshot.states.iter().any(|state| {
            matches!(
                state.transport.last_outcome,
                SessionOutcome::RejectedTooMany
            )
        }));
    }

    #[test]
    fn deploy_runtime_serving_case_contains_serving_path() {
        let spec = ImagodSystemSpec::new();
        let snapshot = reachable_snapshot_for_case(&spec, "deploy_runtime_serving");

        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.deploy.release, ReleaseStage::Promoted))
        );
        assert!(snapshot.states.iter().any(|state| {
            matches!(state.runtime.phase, RuntimePhase::Serving)
                && state.bootstrap.ready
                && state.supervision.has_ready_service()
                && matches!(state.deploy.release, ReleaseStage::Promoted)
        }));
    }

    #[test]
    fn deploy_rollback_case_contains_rollback_path() {
        let spec = ImagodSystemSpec::new();
        let snapshot = reachable_snapshot_for_case(&spec, "deploy_rollback");

        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.deploy.release, ReleaseStage::Promoted))
        );
        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.deploy.release, ReleaseStage::RollbackPending))
        );
        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.deploy.release, ReleaseStage::RolledBack))
        );
    }

    #[test]
    fn bootstrap_runtime_failure_case_contains_oversized_auth_reject_and_invalid_runtime() {
        let spec = ImagodSystemSpec::new();
        let snapshot = reachable_snapshot_for_case(&spec, "bootstrap_runtime_failure");

        assert!(snapshot.states.iter().any(|state| {
            matches!(state.bootstrap.size, BootstrapSizeClass::Oversized)
                && matches!(state.bootstrap.auth, AuthProofState::Rejected)
        }));
        assert!(snapshot.states.iter().any(|state| {
            state.bootstrap.decoded
                && matches!(state.bootstrap.endpoint, EndpointState::Prepared)
                && matches!(state.bootstrap.auth, AuthProofState::Rejected)
        }));
        assert!(snapshot.states.iter().any(|state| {
            matches!(state.runtime.phase, RuntimePhase::Failed)
                && matches!(state.runtime.component, ComponentLoadClass::Invalid)
        }));
        assert!(
            snapshot
                .states
                .iter()
                .any(|state| { matches!(state.runtime.tuning, WasmTuningClass::Invalid) })
        );
    }

    #[test]
    fn plugin_dependency_case_contains_dependency_provider_path() {
        let spec = ImagodSystemSpec::new();
        let snapshot = reachable_snapshot_for_case(&spec, "plugin_dependency");

        assert!(
            snapshot
                .states
                .iter()
                .any(|state| { state.plugin.provider_is_dependency() })
        );
        assert!(snapshot.states.iter().any(|state| {
            state.plugin.capability_decided() && state.plugin.provider_is_dependency()
        }));
    }

    #[test]
    fn shutdown_case_reaches_completed_and_stopped_manager_states() {
        let spec = ImagodSystemSpec::new();
        let snapshot = reachable_snapshot_for_case(&spec, "shutdown");

        assert!(snapshot.states.iter().any(|state| {
            matches!(state.shutdown.phase, ShutdownPhase::Completed)
                && matches!(state.manager.phase, ManagerShellPhase::ShutdownRequested)
        }));
        assert!(snapshot.states.iter().any(|state| {
            matches!(state.shutdown.phase, ShutdownPhase::Completed)
                && matches!(state.manager.phase, ManagerShellPhase::Stopped)
        }));
    }
}
