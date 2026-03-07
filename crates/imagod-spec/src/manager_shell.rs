use nirvash_core::{
    BoundedDomain, ModelCheckConfig, Signature as FormalSignature, TransitionSystem,
};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
#[signature(custom)]
pub struct ManagerShellState {
    pub phase: ManagerShellPhase,
    pub config_loaded: bool,
    pub created_default: bool,
    pub plugin_gc: TaskState,
    pub boot_restore: TaskState,
}

impl ManagerShellStateSignatureSpec for ManagerShellState {
    fn representatives() -> BoundedDomain<Self> {
        let mut states = vec![ManagerShellSpec::new().initial_state()];

        for created_default in [false, true] {
            states.push(Self {
                phase: ManagerShellPhase::ConfigReady,
                config_loaded: true,
                created_default,
                plugin_gc: TaskState::NotStarted,
                boot_restore: TaskState::NotStarted,
            });

            states.push(Self {
                phase: ManagerShellPhase::Listening,
                config_loaded: true,
                created_default,
                plugin_gc: TaskState::NotStarted,
                boot_restore: TaskState::Succeeded,
            });
            states.push(Self {
                phase: ManagerShellPhase::ShutdownRequested,
                config_loaded: true,
                created_default,
                plugin_gc: TaskState::NotStarted,
                boot_restore: TaskState::Succeeded,
            });
            states.push(Self {
                phase: ManagerShellPhase::Stopped,
                config_loaded: true,
                created_default,
                plugin_gc: TaskState::NotStarted,
                boot_restore: TaskState::Succeeded,
            });

            for plugin_gc in [TaskState::Succeeded, TaskState::Failed] {
                states.push(Self {
                    phase: ManagerShellPhase::Restoring,
                    config_loaded: true,
                    created_default,
                    plugin_gc,
                    boot_restore: TaskState::NotStarted,
                });

                for boot_restore in [TaskState::Succeeded, TaskState::Failed] {
                    states.push(Self {
                        phase: ManagerShellPhase::Listening,
                        config_loaded: true,
                        created_default,
                        plugin_gc,
                        boot_restore,
                    });
                    states.push(Self {
                        phase: ManagerShellPhase::ShutdownRequested,
                        config_loaded: true,
                        created_default,
                        plugin_gc,
                        boot_restore,
                    });
                    states.push(Self {
                        phase: ManagerShellPhase::Stopped,
                        config_loaded: true,
                        created_default,
                        plugin_gc,
                        boot_restore,
                    });
                }
            }
        }

        BoundedDomain::new(states)
    }

    fn signature_invariant(&self) -> bool {
        match self.phase {
            ManagerShellPhase::Booting => {
                !self.config_loaded
                    && matches!(self.plugin_gc, TaskState::NotStarted)
                    && matches!(self.boot_restore, TaskState::NotStarted)
            }
            ManagerShellPhase::ConfigReady => self.config_loaded,
            ManagerShellPhase::Restoring => {
                self.config_loaded && !matches!(self.plugin_gc, TaskState::NotStarted)
            }
            ManagerShellPhase::Listening
            | ManagerShellPhase::ShutdownRequested
            | ManagerShellPhase::Stopped => {
                self.config_loaded && !matches!(self.boot_restore, TaskState::NotStarted)
            }
        }
    }
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

fn manager_shell_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        check_deadlocks: false,
        ..ModelCheckConfig::default()
    }
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

nirvash_core::illegal!(ManagerShellSpec, listen_without_config(prev, action, next) => {
    let _ = next;
    matches!(action, ManagerShellAction::StartListening) && !prev.config_loaded
});

nirvash_core::illegal!(ManagerShellSpec, shutdown_before_listen(prev, action, next) => {
    let _ = next;
    matches!(action, ManagerShellAction::BeginShutdown)
        && !matches!(prev.phase, ManagerShellPhase::Listening)
});

nirvash_core::illegal!(ManagerShellSpec, restore_before_plugin_gc(prev, action, next) => {
    let _ = next;
    matches!(
        action,
        ManagerShellAction::RunBootRestoreSucceeded | ManagerShellAction::RunBootRestoreFailed
    ) && matches!(prev.plugin_gc, TaskState::NotStarted)
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

#[subsystem_spec(checker_config(manager_shell_checker_config))]
impl TransitionSystem for ManagerShellSpec {
    type State = ManagerShellState;
    type Action = ManagerShellAction;

    fn name(&self) -> &'static str {
        "manager_shell"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            ManagerShellAction::LoadExistingConfig
                if matches!(prev.phase, ManagerShellPhase::Booting) =>
            {
                candidate.phase = ManagerShellPhase::ConfigReady;
                candidate.config_loaded = true;
                candidate.created_default = false;
            }
            ManagerShellAction::CreateDefaultConfig
                if matches!(prev.phase, ManagerShellPhase::Booting) =>
            {
                candidate.phase = ManagerShellPhase::ConfigReady;
                candidate.config_loaded = true;
                candidate.created_default = true;
            }
            ManagerShellAction::RunPluginGcSucceeded
                if matches!(prev.phase, ManagerShellPhase::ConfigReady) =>
            {
                candidate.phase = ManagerShellPhase::Restoring;
                candidate.plugin_gc = TaskState::Succeeded;
            }
            ManagerShellAction::RunPluginGcFailed
                if matches!(prev.phase, ManagerShellPhase::ConfigReady) =>
            {
                candidate.phase = ManagerShellPhase::Restoring;
                candidate.plugin_gc = TaskState::Failed;
            }
            ManagerShellAction::RunBootRestoreSucceeded
                if matches!(prev.phase, ManagerShellPhase::Restoring) =>
            {
                candidate.phase = ManagerShellPhase::Listening;
                candidate.boot_restore = TaskState::Succeeded;
            }
            ManagerShellAction::RunBootRestoreFailed
                if matches!(prev.phase, ManagerShellPhase::Restoring) =>
            {
                candidate.phase = ManagerShellPhase::Listening;
                candidate.boot_restore = TaskState::Failed;
            }
            ManagerShellAction::StartListening
                if matches!(prev.phase, ManagerShellPhase::ConfigReady) =>
            {
                candidate.phase = ManagerShellPhase::Listening;
                candidate.boot_restore = TaskState::Succeeded;
            }
            ManagerShellAction::BeginShutdown
                if matches!(prev.phase, ManagerShellPhase::Listening) =>
            {
                candidate.phase = ManagerShellPhase::ShutdownRequested;
            }
            ManagerShellAction::FinishShutdown
                if matches!(prev.phase, ManagerShellPhase::ShutdownRequested) =>
            {
                candidate.phase = ManagerShellPhase::Stopped;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[nirvash_macros::formal_tests(spec = ManagerShellSpec, init = initial_state)]
const _: () = ();
