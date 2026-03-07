use imago_formal_core::{
    BoundedDomain, Fairness, Ltl, ModelCheckConfig, Signature as FormalSignature, StatePredicate,
    StepPredicate, TransitionSystem,
};
use imago_formal_macros::{
    Signature, imago_fairness, imago_illegal, imago_invariant, imago_property, imago_subsystem_spec,
};

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

#[imago_invariant]
fn listening_requires_config() -> StatePredicate<ManagerShellState> {
    StatePredicate::new("listening_requires_config", |state| {
        !matches!(
            state.phase,
            ManagerShellPhase::Listening
                | ManagerShellPhase::ShutdownRequested
                | ManagerShellPhase::Stopped
        ) || state.config_loaded
    })
}

#[imago_invariant]
fn restore_depends_on_plugin_gc() -> StatePredicate<ManagerShellState> {
    StatePredicate::new("restore_depends_on_plugin_gc", |state| {
        !matches!(state.phase, ManagerShellPhase::Restoring)
            || !matches!(state.plugin_gc, TaskState::NotStarted)
    })
}

#[imago_invariant]
fn booting_keeps_boot_tasks_idle() -> StatePredicate<ManagerShellState> {
    StatePredicate::new("booting_keeps_boot_tasks_idle", |state| {
        !matches!(state.phase, ManagerShellPhase::Booting)
            || (matches!(state.plugin_gc, TaskState::NotStarted)
                && matches!(state.boot_restore, TaskState::NotStarted))
    })
}

#[imago_illegal]
fn listen_without_config() -> StepPredicate<ManagerShellState, ManagerShellAction> {
    StepPredicate::new("listen_without_config", |prev, action, _| {
        matches!(action, ManagerShellAction::StartListening) && !prev.config_loaded
    })
}

#[imago_illegal]
fn shutdown_before_listen() -> StepPredicate<ManagerShellState, ManagerShellAction> {
    StepPredicate::new("shutdown_before_listen", |prev, action, _| {
        matches!(action, ManagerShellAction::BeginShutdown)
            && !matches!(prev.phase, ManagerShellPhase::Listening)
    })
}

#[imago_illegal]
fn restore_before_plugin_gc() -> StepPredicate<ManagerShellState, ManagerShellAction> {
    StepPredicate::new("restore_before_plugin_gc", |prev, action, _| {
        matches!(
            action,
            ManagerShellAction::RunBootRestoreSucceeded | ManagerShellAction::RunBootRestoreFailed
        ) && matches!(prev.plugin_gc, TaskState::NotStarted)
    })
}

#[imago_property]
fn booting_leads_to_config_ready() -> Ltl<ManagerShellState, ManagerShellAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("booting", |state| {
            matches!(state.phase, ManagerShellPhase::Booting)
        })),
        Ltl::pred(StatePredicate::new("config_ready_or_beyond", |state| {
            !matches!(state.phase, ManagerShellPhase::Booting)
        })),
    )
}

#[imago_property]
fn config_ready_leads_to_listening() -> Ltl<ManagerShellState, ManagerShellAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("config_ready", |state| {
            matches!(state.phase, ManagerShellPhase::ConfigReady)
        })),
        Ltl::pred(StatePredicate::new("listening", |state| {
            matches!(state.phase, ManagerShellPhase::Listening)
        })),
    )
}

#[imago_property]
fn shutdown_requested_leads_to_stopped() -> Ltl<ManagerShellState, ManagerShellAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("shutdown_requested", |state| {
            matches!(state.phase, ManagerShellPhase::ShutdownRequested)
        })),
        Ltl::pred(StatePredicate::new("stopped", |state| {
            matches!(state.phase, ManagerShellPhase::Stopped)
        })),
    )
}

#[imago_fairness]
fn boot_config_progress() -> Fairness<ManagerShellState, ManagerShellAction> {
    Fairness::weak(StepPredicate::new(
        "boot_config_progress",
        |prev, action, next| {
            matches!(prev.phase, ManagerShellPhase::Booting)
                && matches!(
                    action,
                    ManagerShellAction::LoadExistingConfig
                        | ManagerShellAction::CreateDefaultConfig
                )
                && matches!(next.phase, ManagerShellPhase::ConfigReady)
        },
    ))
}

#[imago_fairness]
fn config_ready_progress() -> Fairness<ManagerShellState, ManagerShellAction> {
    Fairness::weak(StepPredicate::new(
        "config_ready_progress",
        |prev, action, next| {
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
        },
    ))
}

#[imago_fairness]
fn shutdown_completion_progress() -> Fairness<ManagerShellState, ManagerShellAction> {
    Fairness::weak(StepPredicate::new(
        "shutdown_completion_progress",
        |prev, action, next| {
            matches!(prev.phase, ManagerShellPhase::ShutdownRequested)
                && matches!(action, ManagerShellAction::FinishShutdown)
                && matches!(next.phase, ManagerShellPhase::Stopped)
        },
    ))
}

#[imago_fairness]
fn restore_progress() -> Fairness<ManagerShellState, ManagerShellAction> {
    Fairness::weak(StepPredicate::new(
        "restore_progress",
        |prev, action, next| {
            matches!(prev.phase, ManagerShellPhase::Restoring)
                && matches!(
                    action,
                    ManagerShellAction::RunBootRestoreSucceeded
                        | ManagerShellAction::RunBootRestoreFailed
                )
                && matches!(next.phase, ManagerShellPhase::Listening)
        },
    ))
}

#[imago_subsystem_spec(
    invariants(
        listening_requires_config,
        restore_depends_on_plugin_gc,
        booting_keeps_boot_tasks_idle
    ),
    illegal(
        listen_without_config,
        shutdown_before_listen,
        restore_before_plugin_gc
    ),
    properties(
        booting_leads_to_config_ready,
        config_ready_leads_to_listening,
        shutdown_requested_leads_to_stopped
    ),
    fairness(
        boot_config_progress,
        config_ready_progress,
        restore_progress,
        shutdown_completion_progress
    ),
    checker_config(manager_shell_checker_config)
)]
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

#[cfg(test)]
#[imago_formal_macros::imago_formal_tests(spec = ManagerShellSpec, init = initial_state)]
const _: () = ();
