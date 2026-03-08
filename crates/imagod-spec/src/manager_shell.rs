use nirvash_core::{ModelCase, TransitionSystem};
use nirvash_macros::{Signature, subsystem_spec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum TaskState {
    NotStarted,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum ManagerShellPhase {
    Booting,
    ConfigReady,
    Restoring,
    Listening,
    ShutdownRequested,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagerShellState {
    pub phase: ManagerShellPhase,
    pub config_loaded: bool,
    pub created_default: bool,
    pub plugin_gc: TaskState,
    pub boot_restore: TaskState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum ManagerShellAction {
    LoadExistingConfig,
    CreateDefaultConfig,
    RunPluginGcSucceeded,
    RunPluginGcFailed,
    RunBootRestoreSucceeded,
    RunBootRestoreFailed,
    StartListening,
    BeginShutdown,
    FinishShutdown,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ManagerShellSpec;

impl ManagerShellSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> ManagerShellState {
        ManagerShellState {
            phase: ManagerShellPhase::Booting,
            config_loaded: false,
            created_default: false,
            plugin_gc: TaskState::NotStarted,
            boot_restore: TaskState::NotStarted,
        }
    }
}

fn manager_shell_model_cases() -> Vec<ModelCase<ManagerShellState, ManagerShellAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

nirvash_core::invariant!(ManagerShellSpec, listening_requires_config(state) => {
    !matches!(
        state.phase,
        ManagerShellPhase::Listening
            | ManagerShellPhase::ShutdownRequested
            | ManagerShellPhase::Stopped
    ) || state.config_loaded
});

nirvash_core::invariant!(ManagerShellSpec, restore_depends_on_plugin_gc(state) => {
    !matches!(state.phase, ManagerShellPhase::Restoring)
        || !matches!(state.plugin_gc, TaskState::NotStarted)
});

nirvash_core::invariant!(ManagerShellSpec, booting_keeps_boot_tasks_idle(state) => {
    !matches!(state.phase, ManagerShellPhase::Booting)
        || (matches!(state.plugin_gc, TaskState::NotStarted)
            && matches!(state.boot_restore, TaskState::NotStarted))
});

nirvash_core::property!(ManagerShellSpec, booting_leads_to_config_ready => leads_to(
    (pred!(booting(state) => matches!(state.phase, ManagerShellPhase::Booting))),
    (pred!(config_ready_or_beyond(state) => !matches!(state.phase, ManagerShellPhase::Booting)))
));

nirvash_core::property!(ManagerShellSpec, config_ready_leads_to_listening => leads_to(
    (pred!(config_ready(state) => matches!(state.phase, ManagerShellPhase::ConfigReady))),
    (pred!(listening(state) => matches!(state.phase, ManagerShellPhase::Listening)))
));

nirvash_core::property!(ManagerShellSpec, shutdown_requested_leads_to_stopped => leads_to(
    (
        pred!(shutdown_requested(state) => matches!(
            state.phase,
            ManagerShellPhase::ShutdownRequested
        ))
    ),
    (pred!(stopped(state) => matches!(state.phase, ManagerShellPhase::Stopped)))
));

nirvash_core::fairness!(
    weak ManagerShellSpec,
    boot_config_progress(prev, action, next) => {
        matches!(prev.phase, ManagerShellPhase::Booting)
            && matches!(
                action,
                ManagerShellAction::LoadExistingConfig | ManagerShellAction::CreateDefaultConfig
            )
            && matches!(next.phase, ManagerShellPhase::ConfigReady)
    }
);

nirvash_core::fairness!(
    weak ManagerShellSpec,
    config_ready_progress(prev, action, next) => {
        matches!(prev.phase, ManagerShellPhase::ConfigReady)
            && matches!(
                action,
                ManagerShellAction::RunPluginGcSucceeded
                    | ManagerShellAction::RunPluginGcFailed
                    | ManagerShellAction::StartListening
            )
            && matches!(
                next.phase,
                ManagerShellPhase::Restoring | ManagerShellPhase::Listening
            )
    }
);

nirvash_core::fairness!(
    weak ManagerShellSpec,
    shutdown_completion_progress(prev, action, next) => {
        matches!(prev.phase, ManagerShellPhase::ShutdownRequested)
            && matches!(action, ManagerShellAction::FinishShutdown)
            && matches!(next.phase, ManagerShellPhase::Stopped)
    }
);

nirvash_core::fairness!(weak ManagerShellSpec, restore_progress(prev, action, next) => {
    matches!(prev.phase, ManagerShellPhase::Restoring)
        && matches!(
            action,
            ManagerShellAction::RunBootRestoreSucceeded
                | ManagerShellAction::RunBootRestoreFailed
        )
        && matches!(next.phase, ManagerShellPhase::Listening)
});

#[subsystem_spec(model_cases(manager_shell_model_cases))]
impl TransitionSystem for ManagerShellSpec {
    type State = ManagerShellState;
    type Action = ManagerShellAction;

    fn name(&self) -> &'static str {
        "manager_shell"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        action_vocabulary()
    }

    fn transition(&self, prev: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_state(prev, action)
    }
}

#[nirvash_macros::formal_tests(spec = ManagerShellSpec)]
const _: () = ();

fn action_vocabulary() -> Vec<ManagerShellAction> {
    vec![
        ManagerShellAction::LoadExistingConfig,
        ManagerShellAction::CreateDefaultConfig,
        ManagerShellAction::RunPluginGcSucceeded,
        ManagerShellAction::RunPluginGcFailed,
        ManagerShellAction::RunBootRestoreSucceeded,
        ManagerShellAction::RunBootRestoreFailed,
        ManagerShellAction::StartListening,
        ManagerShellAction::BeginShutdown,
        ManagerShellAction::FinishShutdown,
    ]
}

fn transition_state(
    prev: &ManagerShellState,
    action: &ManagerShellAction,
) -> Option<ManagerShellState> {
    let mut candidate = *prev;
    match action {
        ManagerShellAction::LoadExistingConfig
            if matches!(prev.phase, ManagerShellPhase::Booting) =>
        {
            candidate.phase = ManagerShellPhase::ConfigReady;
            candidate.config_loaded = true;
            candidate.created_default = false;
            Some(candidate)
        }
        ManagerShellAction::CreateDefaultConfig
            if matches!(prev.phase, ManagerShellPhase::Booting) =>
        {
            candidate.phase = ManagerShellPhase::ConfigReady;
            candidate.config_loaded = true;
            candidate.created_default = true;
            Some(candidate)
        }
        ManagerShellAction::RunPluginGcSucceeded
            if matches!(prev.phase, ManagerShellPhase::ConfigReady) =>
        {
            candidate.phase = ManagerShellPhase::Restoring;
            candidate.plugin_gc = TaskState::Succeeded;
            Some(candidate)
        }
        ManagerShellAction::RunPluginGcFailed
            if matches!(prev.phase, ManagerShellPhase::ConfigReady) =>
        {
            candidate.phase = ManagerShellPhase::Restoring;
            candidate.plugin_gc = TaskState::Failed;
            Some(candidate)
        }
        ManagerShellAction::RunBootRestoreSucceeded
            if matches!(prev.phase, ManagerShellPhase::Restoring) =>
        {
            candidate.phase = ManagerShellPhase::Listening;
            candidate.boot_restore = TaskState::Succeeded;
            Some(candidate)
        }
        ManagerShellAction::RunBootRestoreFailed
            if matches!(prev.phase, ManagerShellPhase::Restoring) =>
        {
            candidate.phase = ManagerShellPhase::Listening;
            candidate.boot_restore = TaskState::Failed;
            Some(candidate)
        }
        ManagerShellAction::StartListening
            if matches!(prev.phase, ManagerShellPhase::ConfigReady) =>
        {
            candidate.phase = ManagerShellPhase::Listening;
            candidate.boot_restore = TaskState::Succeeded;
            Some(candidate)
        }
        ManagerShellAction::BeginShutdown if matches!(prev.phase, ManagerShellPhase::Listening) => {
            candidate.phase = ManagerShellPhase::ShutdownRequested;
            Some(candidate)
        }
        ManagerShellAction::FinishShutdown
            if matches!(prev.phase, ManagerShellPhase::ShutdownRequested) =>
        {
            candidate.phase = ManagerShellPhase::Stopped;
            Some(candidate)
        }
        _ => None,
    }
}
