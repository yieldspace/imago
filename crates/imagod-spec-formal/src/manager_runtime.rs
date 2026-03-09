use nirvash_core::{ModelCase, TransitionSystem};
use nirvash_macros::{ActionVocabulary, Signature, fairness, invariant, property, subsystem_spec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum TaskState {
    NotStarted,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum ManagerRuntimePhase {
    Booting,
    ConfigReady,
    Restoring,
    Listening,
    ShutdownRequested,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagerRuntimeState {
    pub phase: ManagerRuntimePhase,
    pub config_loaded: bool,
    pub created_default: bool,
    pub plugin_gc: TaskState,
    pub boot_restore: TaskState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum ManagerRuntimeAction {
    /// Load config
    LoadExistingConfig,
    /// Create config
    CreateDefaultConfig,
    /// Record GC success
    RunPluginGcSucceeded,
    /// Record GC failure
    RunPluginGcFailed,
    /// Record restore success
    RunBootRestoreSucceeded,
    /// Record restore failure
    RunBootRestoreFailed,
    /// Start listening
    StartListening,
    /// Begin shutdown
    BeginShutdown,
    /// Finish shutdown
    FinishShutdown,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ManagerRuntimeSpec;

impl ManagerRuntimeSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> ManagerRuntimeState {
        ManagerRuntimeState {
            phase: ManagerRuntimePhase::Booting,
            config_loaded: false,
            created_default: false,
            plugin_gc: TaskState::NotStarted,
            boot_restore: TaskState::NotStarted,
        }
    }
}

fn manager_runtime_model_cases() -> Vec<ModelCase<ManagerRuntimeState, ManagerRuntimeAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

#[invariant(ManagerRuntimeSpec)]
fn listening_requires_config() -> nirvash_core::StatePredicate<ManagerRuntimeState> {
    nirvash_core::StatePredicate::new("listening_requires_config", |state| {
        !matches!(
            state.phase,
            ManagerRuntimePhase::Listening
                | ManagerRuntimePhase::ShutdownRequested
                | ManagerRuntimePhase::Stopped
        ) || state.config_loaded
    })
}

#[invariant(ManagerRuntimeSpec)]
fn restore_depends_on_plugin_gc() -> nirvash_core::StatePredicate<ManagerRuntimeState> {
    nirvash_core::StatePredicate::new("restore_depends_on_plugin_gc", |state| {
        !matches!(state.phase, ManagerRuntimePhase::Restoring)
            || !matches!(state.plugin_gc, TaskState::NotStarted)
    })
}

#[invariant(ManagerRuntimeSpec)]
fn booting_keeps_boot_tasks_idle() -> nirvash_core::StatePredicate<ManagerRuntimeState> {
    nirvash_core::StatePredicate::new("booting_keeps_boot_tasks_idle", |state| {
        !matches!(state.phase, ManagerRuntimePhase::Booting)
            || (matches!(state.plugin_gc, TaskState::NotStarted)
                && matches!(state.boot_restore, TaskState::NotStarted))
    })
}

#[property(ManagerRuntimeSpec)]
fn booting_leads_to_config_ready() -> nirvash_core::Ltl<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(nirvash_core::StatePredicate::new("booting", |state| {
            matches!(state.phase, ManagerRuntimePhase::Booting)
        })),
        nirvash_core::Ltl::pred(nirvash_core::StatePredicate::new(
            "config_ready_or_beyond",
            |state| !matches!(state.phase, ManagerRuntimePhase::Booting),
        )),
    )
}

#[property(ManagerRuntimeSpec)]
fn config_ready_leads_to_listening() -> nirvash_core::Ltl<ManagerRuntimeState, ManagerRuntimeAction>
{
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(nirvash_core::StatePredicate::new("config_ready", |state| {
            matches!(state.phase, ManagerRuntimePhase::ConfigReady)
        })),
        nirvash_core::Ltl::pred(nirvash_core::StatePredicate::new("listening", |state| {
            matches!(state.phase, ManagerRuntimePhase::Listening)
        })),
    )
}

#[property(ManagerRuntimeSpec)]
fn shutdown_requested_leads_to_stopped()
-> nirvash_core::Ltl<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(nirvash_core::StatePredicate::new(
            "shutdown_requested",
            |state| matches!(state.phase, ManagerRuntimePhase::ShutdownRequested),
        )),
        nirvash_core::Ltl::pred(nirvash_core::StatePredicate::new("stopped", |state| {
            matches!(state.phase, ManagerRuntimePhase::Stopped)
        })),
    )
}

#[fairness(ManagerRuntimeSpec)]
fn boot_config_progress() -> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(nirvash_core::StepPredicate::new(
        "boot_config_progress",
        |prev, action, next| {
            matches!(prev.phase, ManagerRuntimePhase::Booting)
                && matches!(
                    action,
                    ManagerRuntimeAction::LoadExistingConfig
                        | ManagerRuntimeAction::CreateDefaultConfig
                )
                && matches!(next.phase, ManagerRuntimePhase::ConfigReady)
        },
    ))
}

#[fairness(ManagerRuntimeSpec)]
fn config_ready_progress() -> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(nirvash_core::StepPredicate::new(
        "config_ready_progress",
        |prev, action, next| {
            matches!(prev.phase, ManagerRuntimePhase::ConfigReady)
                && matches!(
                    action,
                    ManagerRuntimeAction::RunPluginGcSucceeded
                        | ManagerRuntimeAction::RunPluginGcFailed
                        | ManagerRuntimeAction::StartListening
                )
                && matches!(
                    next.phase,
                    ManagerRuntimePhase::Restoring | ManagerRuntimePhase::Listening
                )
        },
    ))
}

#[fairness(ManagerRuntimeSpec)]
fn shutdown_completion_progress()
-> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(nirvash_core::StepPredicate::new(
        "shutdown_completion_progress",
        |prev, action, next| {
            matches!(prev.phase, ManagerRuntimePhase::ShutdownRequested)
                && matches!(action, ManagerRuntimeAction::FinishShutdown)
                && matches!(next.phase, ManagerRuntimePhase::Stopped)
        },
    ))
}

#[fairness(ManagerRuntimeSpec)]
fn restore_progress() -> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(nirvash_core::StepPredicate::new(
        "restore_progress",
        |prev, action, next| {
            matches!(prev.phase, ManagerRuntimePhase::Restoring)
                && matches!(
                    action,
                    ManagerRuntimeAction::RunBootRestoreSucceeded
                        | ManagerRuntimeAction::RunBootRestoreFailed
                )
                && matches!(next.phase, ManagerRuntimePhase::Listening)
        },
    ))
}

#[subsystem_spec(model_cases(manager_runtime_model_cases))]
impl TransitionSystem for ManagerRuntimeSpec {
    type State = ManagerRuntimeState;
    type Action = ManagerRuntimeAction;

    fn name(&self) -> &'static str {
        "manager_runtime"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, prev: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_state(prev, action)
    }
}

#[nirvash_macros::formal_tests(spec = ManagerRuntimeSpec)]
const _: () = ();

fn transition_state(
    prev: &ManagerRuntimeState,
    action: &ManagerRuntimeAction,
) -> Option<ManagerRuntimeState> {
    let mut candidate = *prev;
    match action {
        ManagerRuntimeAction::LoadExistingConfig
            if matches!(prev.phase, ManagerRuntimePhase::Booting) =>
        {
            candidate.phase = ManagerRuntimePhase::ConfigReady;
            candidate.config_loaded = true;
            candidate.created_default = false;
            Some(candidate)
        }
        ManagerRuntimeAction::CreateDefaultConfig
            if matches!(prev.phase, ManagerRuntimePhase::Booting) =>
        {
            candidate.phase = ManagerRuntimePhase::ConfigReady;
            candidate.config_loaded = true;
            candidate.created_default = true;
            Some(candidate)
        }
        ManagerRuntimeAction::RunPluginGcSucceeded
            if matches!(prev.phase, ManagerRuntimePhase::ConfigReady) =>
        {
            candidate.phase = ManagerRuntimePhase::Restoring;
            candidate.plugin_gc = TaskState::Succeeded;
            Some(candidate)
        }
        ManagerRuntimeAction::RunPluginGcFailed
            if matches!(prev.phase, ManagerRuntimePhase::ConfigReady) =>
        {
            candidate.phase = ManagerRuntimePhase::Restoring;
            candidate.plugin_gc = TaskState::Failed;
            Some(candidate)
        }
        ManagerRuntimeAction::RunBootRestoreSucceeded
            if matches!(prev.phase, ManagerRuntimePhase::Restoring) =>
        {
            candidate.phase = ManagerRuntimePhase::Listening;
            candidate.boot_restore = TaskState::Succeeded;
            Some(candidate)
        }
        ManagerRuntimeAction::RunBootRestoreFailed
            if matches!(prev.phase, ManagerRuntimePhase::Restoring) =>
        {
            candidate.phase = ManagerRuntimePhase::Listening;
            candidate.boot_restore = TaskState::Failed;
            Some(candidate)
        }
        ManagerRuntimeAction::StartListening
            if matches!(prev.phase, ManagerRuntimePhase::ConfigReady) =>
        {
            candidate.phase = ManagerRuntimePhase::Listening;
            candidate.boot_restore = TaskState::Succeeded;
            Some(candidate)
        }
        ManagerRuntimeAction::BeginShutdown
            if matches!(prev.phase, ManagerRuntimePhase::Listening) =>
        {
            candidate.phase = ManagerRuntimePhase::ShutdownRequested;
            Some(candidate)
        }
        ManagerRuntimeAction::FinishShutdown
            if matches!(prev.phase, ManagerRuntimePhase::ShutdownRequested) =>
        {
            candidate.phase = ManagerRuntimePhase::Stopped;
            Some(candidate)
        }
        _ => None,
    }
}
