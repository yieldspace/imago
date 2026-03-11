use nirvash_core::{
    BoolExpr, ModelCase, ModelCaseSource, ModelCheckConfig, StepExpr, TemporalSpec,
    TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, Signature as FormalSignature, action_constraint, invariant, nirvash_expr,
    nirvash_step_expr, nirvash_transition_program, system_spec,
};

use crate::{
    CommandErrorKind, CommandKind, CommandLifecycleState, CommandProtocolAction, PluginKind,
    RunnerAppType,
    artifact_deploy::{
        ArtifactDeployAction, ArtifactDeploySpec, ArtifactDeployState, ReleaseStage,
    },
    command_protocol::CommandProtocolSpec,
    manager_runtime::{
        ManagerRuntimeAction, ManagerRuntimePhase, ManagerRuntimeSpec, ManagerRuntimeState,
    },
    plugin_platform::{PluginPlatformAction, PluginPlatformSpec, PluginPlatformState},
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

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
pub struct ImagodSystemState {
    pub manager: ManagerRuntimeState,
    pub transport: SessionTransportState,
    pub command: crate::command_protocol::CommandProtocolState,
    pub deploy: ArtifactDeployState,
    pub supervision: ServiceSupervisionState,
    pub bootstrap: RunnerBootstrapState,
    pub runtime: RunnerRuntimeState,
    pub plugin: PluginPlatformState,
    pub shutdown: ShutdownFlowState,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature, ActionVocabulary)]
/// Top-level system actions delegated to subsystem specifications.
pub enum ImagodSystemAction {
    Manager(#[sig(domain = imagod_system_manager_action_vocabulary)] ManagerRuntimeAction),
    Session(#[sig(domain = imagod_system_session_action_vocabulary)] SessionTransportAction),
    Command(#[sig(domain = imagod_system_command_action_vocabulary)] CommandProtocolAction),
    Deploy(#[sig(domain = imagod_system_deploy_action_vocabulary)] ArtifactDeployAction),
    Supervision(
        #[sig(domain = imagod_system_supervision_action_vocabulary)] ServiceSupervisionAction,
    ),
    Bootstrap(#[sig(domain = imagod_system_bootstrap_action_vocabulary)] RunnerBootstrapAction),
    Runtime(#[sig(domain = imagod_system_runtime_action_vocabulary)] RunnerRuntimeAction),
    Plugin(#[sig(domain = imagod_system_plugin_action_vocabulary)] PluginPlatformAction),
    Shutdown(#[sig(domain = imagod_system_shutdown_action_vocabulary)] ShutdownFlowAction),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ImagodSystemSpec;

impl ImagodSystemSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> ImagodSystemState {
        ImagodSystemState {
            manager: ManagerRuntimeSpec::new().initial_state(),
            transport: SessionTransportSpec::new().initial_state(),
            command: CommandProtocolSpec::new().initial_state(),
            deploy: ArtifactDeploySpec::new().initial_state(),
            supervision: ServiceSupervisionSpec::new().initial_state(),
            bootstrap: RunnerBootstrapSpec::new().initial_state(),
            runtime: RunnerRuntimeSpec::new().initial_state(),
            plugin: PluginPlatformSpec::new().initial_state(),
            shutdown: ShutdownFlowSpec::new().initial_state(),
        }
    }

    fn transition_state(
        &self,
        prev: &ImagodSystemState,
        action: &ImagodSystemAction,
    ) -> Option<ImagodSystemState> {
        let manager_spec = ManagerRuntimeSpec::new();
        let transport_spec = SessionTransportSpec::new();
        let command_spec = CommandProtocolSpec::new();
        let deploy_spec = ArtifactDeploySpec::new();
        let supervision_spec = ServiceSupervisionSpec::new();
        let bootstrap_spec = RunnerBootstrapSpec::new();
        let runtime_spec = RunnerRuntimeSpec::new();
        let plugin_spec = PluginPlatformSpec::new();
        let shutdown_spec = ShutdownFlowSpec::new();

        let mut candidate = prev.clone();
        match action {
            ImagodSystemAction::Manager(ManagerRuntimeAction::BeginShutdown) => {
                candidate.manager =
                    manager_spec.transition(&prev.manager, &ManagerRuntimeAction::BeginShutdown)?;
                candidate.transport = transport_spec
                    .transition(&prev.transport, &SessionTransportAction::BeginShutdown)?;
                candidate.shutdown =
                    shutdown_spec.transition(&prev.shutdown, &ShutdownFlowAction::ReceiveSignal)?;
            }
            ImagodSystemAction::Manager(ManagerRuntimeAction::FinishShutdown) => {
                if !matches!(prev.shutdown.phase, ShutdownPhase::Completed) {
                    return None;
                }
                candidate.manager = manager_spec
                    .transition(&prev.manager, &ManagerRuntimeAction::FinishShutdown)?;
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

fn imagod_system_manager_action_vocabulary() -> Vec<ManagerRuntimeAction> {
    vec![
        ManagerRuntimeAction::LoadExistingConfig,
        ManagerRuntimeAction::CreateDefaultConfig,
        ManagerRuntimeAction::RunPluginGcSucceeded,
        ManagerRuntimeAction::RunPluginGcFailed,
        ManagerRuntimeAction::RunBootRestoreSucceeded,
        ManagerRuntimeAction::RunBootRestoreFailed,
        ManagerRuntimeAction::StartListening,
        ManagerRuntimeAction::BeginShutdown,
        ManagerRuntimeAction::FinishShutdown,
    ]
}

fn imagod_system_session_action_vocabulary() -> Vec<SessionTransportAction> {
    vec![
        SessionTransportAction::AcceptSession,
        SessionTransportAction::RejectTooMany,
        SessionTransportAction::JoinSession,
    ]
}

fn imagod_system_command_action_vocabulary() -> Vec<CommandProtocolAction> {
    vec![
        CommandProtocolAction::Start(CommandKind::Run),
        CommandProtocolAction::SetRunning,
        CommandProtocolAction::RequestCancel,
        CommandProtocolAction::MarkSpawned,
        CommandProtocolAction::FinishFailed(CommandErrorKind::Internal),
        CommandProtocolAction::FinishCanceled,
        CommandProtocolAction::Remove,
    ]
}

fn imagod_system_deploy_action_vocabulary() -> Vec<ArtifactDeployAction> {
    vec![
        ArtifactDeployAction::ReceiveChunk,
        ArtifactDeployAction::CompleteUpload,
        ArtifactDeployAction::CommitUpload,
        ArtifactDeployAction::StartDeployMatched,
        ArtifactDeployAction::StartDeployMismatched,
        ArtifactDeployAction::PromoteRelease,
        ArtifactDeployAction::TriggerRollback,
        ArtifactDeployAction::FinishRollback,
    ]
}

fn imagod_system_supervision_action_vocabulary() -> Vec<ServiceSupervisionAction> {
    vec![ServiceSupervisionAction::StartService]
}

fn imagod_system_bootstrap_action_vocabulary() -> Vec<RunnerBootstrapAction> {
    vec![
        RunnerBootstrapAction::ReadWithinBounds,
        RunnerBootstrapAction::ReadOversized,
        RunnerBootstrapAction::DecodeBootstrap(RunnerAppType::Rpc),
        RunnerBootstrapAction::PrepareEndpoint,
        RunnerBootstrapAction::RegisterRunner,
        RunnerBootstrapAction::RejectAuthProof,
        RunnerBootstrapAction::MarkReady,
    ]
}

fn imagod_system_runtime_action_vocabulary() -> Vec<RunnerRuntimeAction> {
    vec![
        RunnerRuntimeAction::SelectMode(RunnerAppType::Rpc),
        RunnerRuntimeAction::ApplyDefaultTuning,
        RunnerRuntimeAction::ApplyInvalidTuning,
        RunnerRuntimeAction::ValidateComponentLoadable,
        RunnerRuntimeAction::ValidateComponentInvalid,
        RunnerRuntimeAction::StartServing,
        RunnerRuntimeAction::FailRuntime,
    ]
}

fn imagod_system_plugin_action_vocabulary() -> Vec<PluginPlatformAction> {
    vec![
        PluginPlatformAction::RegisterPlugin(PluginKind::Wasm),
        PluginPlatformAction::ClassifyGraphAcyclic,
        PluginPlatformAction::ClassifyGraphMissingDependency,
        PluginPlatformAction::ResolveProviderSelf,
        PluginPlatformAction::ResolveProviderDependency,
        PluginPlatformAction::ResolveProviderMissing,
        PluginPlatformAction::AllowCapability,
        PluginPlatformAction::GrantPrivilegedCapability,
        PluginPlatformAction::AllowHttpHost,
        PluginPlatformAction::DenyHttpOutbound,
    ]
}

fn imagod_system_shutdown_action_vocabulary() -> Vec<ShutdownFlowAction> {
    vec![
        ShutdownFlowAction::StopAccepting,
        ShutdownFlowAction::DrainSessions,
        ShutdownFlowAction::StopServicesGraceful,
        ShutdownFlowAction::StopServicesForced,
        ShutdownFlowAction::StopMaintenance,
        ShutdownFlowAction::Finalize,
    ]
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
}

fn deploy_runtime_serving_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("deploy_runtime_serving")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
}

fn deploy_rollback_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("deploy_rollback")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
}

fn bootstrap_runtime_failure_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("bootstrap_runtime_failure")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
}

fn plugin_dependency_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("plugin_dependency")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
}

fn shutdown_model_case() -> ModelCase<ImagodSystemState, ImagodSystemAction> {
    ModelCase::new("shutdown")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
}

fn system_checker_config() -> ModelCheckConfig {
    ModelCheckConfig::reachable_graph()
}

fn system_doc_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        backend: None,
        exploration: nirvash_core::ExplorationMode::ReachableGraph,
        bounded_depth: None,
        max_states: Some(128),
        max_transitions: Some(384),
        check_deadlocks: true,
        stop_on_first_violation: false,
    }
}

#[action_constraint(ImagodSystemSpec, cases("startup_command"))]
fn startup_command_action_constraint() -> StepExpr<ImagodSystemState, ImagodSystemAction> {
    nirvash_step_expr! { startup_command_actions(prev, action, _next) =>
        startup_command_session_progress_allowed(prev, action)
            || startup_command_non_session_action_allowed(action)
    }
}

#[action_constraint(ImagodSystemSpec, cases("deploy_runtime_serving"))]
fn deploy_runtime_serving_action_constraint() -> StepExpr<ImagodSystemState, ImagodSystemAction> {
    nirvash_step_expr! { deploy_runtime_serving_actions(prev, action, _next) =>
        deterministic_session_cycle_allowed(prev, action)
            || deploy_runtime_serving_non_session_action_allowed(action)
    }
}

#[action_constraint(ImagodSystemSpec, cases("deploy_rollback"))]
fn deploy_rollback_action_constraint() -> StepExpr<ImagodSystemState, ImagodSystemAction> {
    nirvash_step_expr! { deploy_rollback_actions(prev, action, _next) =>
        deterministic_session_cycle_allowed(prev, action)
            || deploy_rollback_non_session_action_allowed(action)
    }
}

#[action_constraint(ImagodSystemSpec, cases("bootstrap_runtime_failure"))]
fn bootstrap_runtime_failure_action_constraint() -> StepExpr<ImagodSystemState, ImagodSystemAction>
{
    nirvash_step_expr! { bootstrap_runtime_failure_actions(prev, action, _next) =>
        deterministic_session_cycle_allowed(prev, action)
            || bootstrap_runtime_failure_non_session_action_allowed(action)
    }
}

#[action_constraint(ImagodSystemSpec, cases("plugin_dependency"))]
fn plugin_dependency_action_constraint() -> StepExpr<ImagodSystemState, ImagodSystemAction> {
    nirvash_step_expr! { plugin_dependency_actions(prev, action, _next) =>
        deterministic_session_cycle_allowed(prev, action)
            || plugin_dependency_non_session_action_allowed(action)
    }
}

#[action_constraint(ImagodSystemSpec, cases("shutdown"))]
fn shutdown_action_constraint() -> StepExpr<ImagodSystemState, ImagodSystemAction> {
    nirvash_step_expr! { shutdown_actions(prev, action, _next) =>
        shutdown_session_progress_allowed(prev, action)
            || shutdown_non_session_action_allowed(action)
    }
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
        ImagodSystemAction::Manager(ManagerRuntimeAction::LoadExistingConfig)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::CreateDefaultConfig)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::RunPluginGcSucceeded)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::RunPluginGcFailed)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::RunBootRestoreSucceeded)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::RunBootRestoreFailed)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::StartListening)
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
        ImagodSystemAction::Plugin(PluginPlatformAction::RegisterPlugin(PluginKind::Wasm))
            | ImagodSystemAction::Plugin(PluginPlatformAction::ClassifyGraphAcyclic)
            | ImagodSystemAction::Plugin(PluginPlatformAction::ClassifyGraphMissingDependency)
            | ImagodSystemAction::Plugin(PluginPlatformAction::ResolveProviderSelf)
            | ImagodSystemAction::Plugin(PluginPlatformAction::ResolveProviderDependency)
            | ImagodSystemAction::Plugin(PluginPlatformAction::ResolveProviderMissing)
            | ImagodSystemAction::Plugin(PluginPlatformAction::AllowCapability)
            | ImagodSystemAction::Plugin(PluginPlatformAction::GrantPrivilegedCapability)
            | ImagodSystemAction::Plugin(PluginPlatformAction::AllowHttpHost)
            | ImagodSystemAction::Plugin(PluginPlatformAction::DenyHttpOutbound)
    )
}

fn shutdown_non_session_action_allowed(action: &ImagodSystemAction) -> bool {
    matches!(
        action,
        ImagodSystemAction::Manager(ManagerRuntimeAction::LoadExistingConfig)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::RunPluginGcSucceeded)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::RunBootRestoreSucceeded)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::BeginShutdown)
            | ImagodSystemAction::Manager(ManagerRuntimeAction::FinishShutdown)
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
}

fn system_state_valid(state: &ImagodSystemState) -> bool {
    state_respects_spec(&ManagerRuntimeSpec::new(), &state.manager)
        && state_respects_spec(&SessionTransportSpec::new(), &state.transport)
        && state_respects_spec(&CommandProtocolSpec::new(), &state.command)
        && state_respects_spec(&ArtifactDeploySpec::new(), &state.deploy)
        && state_respects_spec(&ServiceSupervisionSpec::new(), &state.supervision)
        && state_respects_spec(&RunnerBootstrapSpec::new(), &state.bootstrap)
        && state_respects_spec(&RunnerRuntimeSpec::new(), &state.runtime)
        && state_respects_spec(&PluginPlatformSpec::new(), &state.plugin)
        && state_respects_spec(&ShutdownFlowSpec::new(), &state.shutdown)
        && cross_links_hold(state)
}

#[invariant(ImagodSystemSpec)]
fn runtime_serving_requires_ready_and_promoted_release() -> BoolExpr<ImagodSystemState> {
    nirvash_expr! { runtime_serving_requires_ready_and_promoted_release(state) =>
        !matches!(state.runtime.phase, RuntimePhase::Serving)
            || (state.bootstrap.ready
                && state.supervision.has_ready_service()
                && matches!(state.deploy.release, ReleaseStage::Promoted))
    }
}

#[invariant(ImagodSystemSpec)]
fn shutdown_requires_transport_gate_and_manager_shutdown() -> BoolExpr<ImagodSystemState> {
    nirvash_expr! { shutdown_requires_transport_gate_and_manager_shutdown(state) =>
        matches!(state.shutdown.phase, ShutdownPhase::Idle)
            || (state.transport.shutdown_requested
                && matches!(
                    state.manager.phase,
                    ManagerRuntimePhase::ShutdownRequested | ManagerRuntimePhase::Stopped
                ))
    }
}

#[invariant(ImagodSystemSpec)]
fn ready_runner_requires_running_supervision() -> BoolExpr<ImagodSystemState> {
    nirvash_expr! { ready_runner_requires_running_supervision(state) =>
        !state.bootstrap.ready || state.supervision.has_ready_service()
    }
}

#[invariant(ImagodSystemSpec)]
fn active_command_requires_listening_manager() -> BoolExpr<ImagodSystemState> {
    nirvash_expr! { active_command_requires_listening_manager(state) =>
        !matches!(
            state.command.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        ) || matches!(state.manager.phase, ManagerRuntimePhase::Listening)
    }
}

#[invariant(ImagodSystemSpec)]
fn stopped_manager_requires_completed_shutdown() -> BoolExpr<ImagodSystemState> {
    nirvash_expr! { stopped_manager_requires_completed_shutdown(state) =>
        !matches!(state.manager.phase, ManagerRuntimePhase::Stopped)
            || matches!(state.shutdown.phase, ShutdownPhase::Completed)
    }
}

#[invariant(ImagodSystemSpec)]
fn dependency_provider_requires_acyclic_plugin_graph() -> BoolExpr<ImagodSystemState> {
    nirvash_expr! { dependency_provider_requires_acyclic_plugin_graph(state) =>
        !state.plugin.provider_is_dependency() || state.plugin.graph_is_acyclic()
    }
}

#[system_spec(
    model_cases(system_model_cases),
    subsystems(
        "manager_runtime",
        "session_transport",
        "command_protocol",
        "artifact_deploy",
        "service_supervision",
        "runner_bootstrap",
        "runner_runtime",
        "plugin_platform",
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
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash_core::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule imagod_system_transition when imagod_system_transition(prev, action).is_some() => {
                set self <= imagod_system_transition(prev, action)
                    .expect("imagod_system_transition guard matched");
            }
        })
    }
}

fn imagod_system_transition(
    prev: &ImagodSystemState,
    action: &ImagodSystemAction,
) -> Option<ImagodSystemState> {
    ImagodSystemSpec::new().transition_state(prev, action)
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
                    ManagerRuntimePhase::ShutdownRequested | ManagerRuntimePhase::Stopped
                )))
        && (!state.bootstrap.ready || state.supervision.has_ready_service())
        && (!matches!(
            state.command.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        ) || matches!(state.manager.phase, ManagerRuntimePhase::Listening))
        && (!matches!(state.manager.phase, ManagerRuntimePhase::Stopped)
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
        runner_bootstrap::{AuthProofState, BootstrapSizeClass, EndpointState},
        runner_runtime::{ComponentLoadClass, WasmTuningClass},
        session_transport::SessionOutcome,
    };

    #[test]
    fn derived_action_vocabulary_preserves_representative_subset() {
        assert_eq!(
            <ImagodSystemAction as nirvash_core::ActionVocabulary>::action_vocabulary(),
            vec![
                ImagodSystemAction::Manager(ManagerRuntimeAction::LoadExistingConfig),
                ImagodSystemAction::Manager(ManagerRuntimeAction::CreateDefaultConfig),
                ImagodSystemAction::Manager(ManagerRuntimeAction::RunPluginGcSucceeded),
                ImagodSystemAction::Manager(ManagerRuntimeAction::RunPluginGcFailed),
                ImagodSystemAction::Manager(ManagerRuntimeAction::RunBootRestoreSucceeded),
                ImagodSystemAction::Manager(ManagerRuntimeAction::RunBootRestoreFailed),
                ImagodSystemAction::Manager(ManagerRuntimeAction::StartListening),
                ImagodSystemAction::Manager(ManagerRuntimeAction::BeginShutdown),
                ImagodSystemAction::Manager(ManagerRuntimeAction::FinishShutdown),
                ImagodSystemAction::Session(SessionTransportAction::AcceptSession),
                ImagodSystemAction::Session(SessionTransportAction::RejectTooMany),
                ImagodSystemAction::Session(SessionTransportAction::JoinSession),
                ImagodSystemAction::Command(CommandProtocolAction::Start(CommandKind::Run)),
                ImagodSystemAction::Command(CommandProtocolAction::SetRunning),
                ImagodSystemAction::Command(CommandProtocolAction::RequestCancel),
                ImagodSystemAction::Command(CommandProtocolAction::MarkSpawned),
                ImagodSystemAction::Command(CommandProtocolAction::FinishFailed(
                    CommandErrorKind::Internal,
                )),
                ImagodSystemAction::Command(CommandProtocolAction::FinishCanceled),
                ImagodSystemAction::Command(CommandProtocolAction::Remove),
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
                ImagodSystemAction::Plugin(PluginPlatformAction::RegisterPlugin(PluginKind::Wasm,)),
                ImagodSystemAction::Plugin(PluginPlatformAction::ClassifyGraphAcyclic),
                ImagodSystemAction::Plugin(PluginPlatformAction::ClassifyGraphMissingDependency,),
                ImagodSystemAction::Plugin(PluginPlatformAction::ResolveProviderSelf),
                ImagodSystemAction::Plugin(PluginPlatformAction::ResolveProviderDependency),
                ImagodSystemAction::Plugin(PluginPlatformAction::ResolveProviderMissing),
                ImagodSystemAction::Plugin(PluginPlatformAction::AllowCapability),
                ImagodSystemAction::Plugin(PluginPlatformAction::GrantPrivilegedCapability),
                ImagodSystemAction::Plugin(PluginPlatformAction::AllowHttpHost),
                ImagodSystemAction::Plugin(PluginPlatformAction::DenyHttpOutbound),
                ImagodSystemAction::Shutdown(ShutdownFlowAction::StopAccepting),
                ImagodSystemAction::Shutdown(ShutdownFlowAction::DrainSessions),
                ImagodSystemAction::Shutdown(ShutdownFlowAction::StopServicesGraceful),
                ImagodSystemAction::Shutdown(ShutdownFlowAction::StopServicesForced),
                ImagodSystemAction::Shutdown(ShutdownFlowAction::StopMaintenance),
                ImagodSystemAction::Shutdown(ShutdownFlowAction::Finalize),
            ]
        );
    }

    #[test]
    fn wrapper_action_labels_delegate_to_nested_action_docs() {
        assert_eq!(
            nirvash_core::format_doc_graph_action(&ImagodSystemAction::Manager(
                ManagerRuntimeAction::LoadExistingConfig,
            )),
            "Load config"
        );
    }

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
                &ImagodSystemAction::Manager(ManagerRuntimeAction::LoadExistingConfig),
            )
            .expect("config should load");
        let restoring = spec
            .transition(
                &config_ready,
                &ImagodSystemAction::Manager(ManagerRuntimeAction::RunPluginGcSucceeded),
            )
            .expect("plugin gc should complete");
        spec.transition(
            &restoring,
            &ImagodSystemAction::Manager(ManagerRuntimeAction::RunBootRestoreSucceeded),
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
            manager: ManagerRuntimeState {
                phase: ManagerRuntimePhase::Listening,
                config_loaded: true,
                created_default: false,
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
            plugin: PluginPlatformSpec::new().initial_state(),
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
                &ImagodSystemAction::Manager(ManagerRuntimeAction::LoadExistingConfig),
            )
            .expect("config load should advance to config ready");
        let restoring = spec
            .transition(
                &config_ready,
                &ImagodSystemAction::Manager(ManagerRuntimeAction::RunPluginGcSucceeded),
            )
            .expect("plugin gc should advance to restoring");
        let listening = spec
            .transition(
                &restoring,
                &ImagodSystemAction::Manager(ManagerRuntimeAction::RunBootRestoreSucceeded),
            )
            .expect("boot restore should advance to listening");

        assert!(matches!(
            config_ready.manager.phase,
            ManagerRuntimePhase::ConfigReady
        ));
        assert!(matches!(
            restoring.manager.phase,
            ManagerRuntimePhase::Restoring
        ));
        assert!(matches!(
            listening.manager.phase,
            ManagerRuntimePhase::Listening
        ));
        assert!(
            spec.transition(
                &initial,
                &ImagodSystemAction::Manager(ManagerRuntimeAction::StartListening),
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
                &ImagodSystemAction::Manager(ManagerRuntimeAction::BeginShutdown),
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
                &ImagodSystemAction::Manager(ManagerRuntimeAction::FinishShutdown),
            )
            .expect("manager finish should stop manager after shutdown completion");

        assert!(matches!(
            signal_received.shutdown.phase,
            ShutdownPhase::SignalReceived
        ));
        assert!(signal_received.transport.shutdown_requested);
        assert!(matches!(
            signal_received.manager.phase,
            ManagerRuntimePhase::ShutdownRequested
        ));
        assert!(matches!(completed.shutdown.phase, ShutdownPhase::Completed));
        assert!(matches!(
            completed.manager.phase,
            ManagerRuntimePhase::ShutdownRequested
        ));
        assert!(matches!(
            finished.manager.phase,
            ManagerRuntimePhase::Stopped
        ));
        assert!(
            spec.transition(
                &stopping_maintenance,
                &ImagodSystemAction::Manager(ManagerRuntimeAction::FinishShutdown),
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
                .any(|state| matches!(state.manager.phase, ManagerRuntimePhase::ConfigReady))
        );
        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.manager.phase, ManagerRuntimePhase::Restoring))
        );
        assert!(
            snapshot
                .states
                .iter()
                .any(|state| matches!(state.manager.phase, ManagerRuntimePhase::Listening))
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
                && matches!(state.manager.phase, ManagerRuntimePhase::ShutdownRequested)
        }));
        assert!(snapshot.states.iter().any(|state| {
            matches!(state.shutdown.phase, ShutdownPhase::Completed)
                && matches!(state.manager.phase, ManagerRuntimePhase::Stopped)
        }));
    }
}
