use nirvash_core::{ModelCase, TransitionSystem};
use nirvash_macros::{
    ActionVocabulary, Signature, fairness, invariant, nirvash_expr, nirvash_step_expr,
    nirvash_transition_program, property, subsystem_spec,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub struct ManagerRuntimeState {
    pub phase: ManagerRuntimePhase,
    pub config_loaded: bool,
    pub created_default: bool,
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
        }
    }
}

fn manager_runtime_model_cases() -> Vec<ModelCase<ManagerRuntimeState, ManagerRuntimeAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

#[invariant(ManagerRuntimeSpec)]
fn listening_requires_config() -> nirvash_core::BoolExpr<ManagerRuntimeState> {
    nirvash_expr! { listening_requires_config(state) =>
        !matches!(
            state.phase,
            ManagerRuntimePhase::Listening
                | ManagerRuntimePhase::ShutdownRequested
                | ManagerRuntimePhase::Stopped
        ) || state.config_loaded
    }
}

#[property(ManagerRuntimeSpec)]
fn booting_leads_to_config_ready() -> nirvash_core::Ltl<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(nirvash_expr! { booting(state) =>
            matches!(state.phase, ManagerRuntimePhase::Booting)
        }),
        nirvash_core::Ltl::pred(nirvash_expr! { config_ready_or_beyond(state) =>
            !matches!(state.phase, ManagerRuntimePhase::Booting)
        }),
    )
}

#[property(ManagerRuntimeSpec)]
fn config_ready_leads_to_listening() -> nirvash_core::Ltl<ManagerRuntimeState, ManagerRuntimeAction>
{
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(nirvash_expr! { config_ready(state) =>
            matches!(state.phase, ManagerRuntimePhase::ConfigReady)
        }),
        nirvash_core::Ltl::pred(nirvash_expr! { listening(state) =>
            matches!(state.phase, ManagerRuntimePhase::Listening)
        }),
    )
}

#[property(ManagerRuntimeSpec)]
fn shutdown_requested_leads_to_stopped()
-> nirvash_core::Ltl<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(nirvash_expr! { shutdown_requested(state) =>
            matches!(state.phase, ManagerRuntimePhase::ShutdownRequested)
        }),
        nirvash_core::Ltl::pred(nirvash_expr! { stopped(state) =>
            matches!(state.phase, ManagerRuntimePhase::Stopped)
        }),
    )
}

#[fairness(ManagerRuntimeSpec)]
fn boot_config_progress() -> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(
        nirvash_step_expr! { boot_config_progress(prev, action, next) =>
            matches!(prev.phase, ManagerRuntimePhase::Booting)
                && matches!(
                    action,
                    ManagerRuntimeAction::LoadExistingConfig
                        | ManagerRuntimeAction::CreateDefaultConfig
                )
                && matches!(next.phase, ManagerRuntimePhase::ConfigReady)
        },
    )
}

#[fairness(ManagerRuntimeSpec)]
fn config_ready_progress() -> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(
        nirvash_step_expr! { config_ready_progress(prev, action, next) =>
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
    )
}

#[fairness(ManagerRuntimeSpec)]
fn shutdown_completion_progress()
-> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(
        nirvash_step_expr! { shutdown_completion_progress(prev, action, next) =>
            matches!(prev.phase, ManagerRuntimePhase::ShutdownRequested)
                && matches!(action, ManagerRuntimeAction::FinishShutdown)
                && matches!(next.phase, ManagerRuntimePhase::Stopped)
        },
    )
}

#[fairness(ManagerRuntimeSpec)]
fn restore_progress() -> nirvash_core::Fairness<ManagerRuntimeState, ManagerRuntimeAction> {
    nirvash_core::Fairness::weak(nirvash_step_expr! { restore_progress(prev, action, next) =>
        matches!(prev.phase, ManagerRuntimePhase::Restoring)
            && matches!(
                action,
                ManagerRuntimeAction::RunBootRestoreSucceeded
                    | ManagerRuntimeAction::RunBootRestoreFailed
            )
            && matches!(next.phase, ManagerRuntimePhase::Listening)
    })
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

    fn transition_program(
        &self,
    ) -> Option<::nirvash_core::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule load_existing_config when matches!(action, ManagerRuntimeAction::LoadExistingConfig)
                && matches!(prev.phase, ManagerRuntimePhase::Booting) => {
                set phase <= ManagerRuntimePhase::ConfigReady;
                set config_loaded <= true;
                set created_default <= false;
            }

            rule create_default_config when matches!(action, ManagerRuntimeAction::CreateDefaultConfig)
                && matches!(prev.phase, ManagerRuntimePhase::Booting) => {
                set phase <= ManagerRuntimePhase::ConfigReady;
                set config_loaded <= true;
                set created_default <= true;
            }

            rule run_plugin_gc_succeeded when matches!(action, ManagerRuntimeAction::RunPluginGcSucceeded)
                && matches!(prev.phase, ManagerRuntimePhase::ConfigReady) => {
                set phase <= ManagerRuntimePhase::Restoring;
            }

            rule run_plugin_gc_failed when matches!(action, ManagerRuntimeAction::RunPluginGcFailed)
                && matches!(prev.phase, ManagerRuntimePhase::ConfigReady) => {
                set phase <= ManagerRuntimePhase::Restoring;
            }

            rule run_boot_restore_succeeded when matches!(action, ManagerRuntimeAction::RunBootRestoreSucceeded)
                && matches!(prev.phase, ManagerRuntimePhase::Restoring) => {
                set phase <= ManagerRuntimePhase::Listening;
            }

            rule run_boot_restore_failed when matches!(action, ManagerRuntimeAction::RunBootRestoreFailed)
                && matches!(prev.phase, ManagerRuntimePhase::Restoring) => {
                set phase <= ManagerRuntimePhase::Listening;
            }

            rule start_listening when matches!(action, ManagerRuntimeAction::StartListening)
                && matches!(prev.phase, ManagerRuntimePhase::ConfigReady) => {
                set phase <= ManagerRuntimePhase::Listening;
            }

            rule begin_shutdown when matches!(action, ManagerRuntimeAction::BeginShutdown)
                && matches!(prev.phase, ManagerRuntimePhase::Listening) => {
                set phase <= ManagerRuntimePhase::ShutdownRequested;
            }

            rule finish_shutdown when matches!(action, ManagerRuntimeAction::FinishShutdown)
                && matches!(prev.phase, ManagerRuntimePhase::ShutdownRequested) => {
                set phase <= ManagerRuntimePhase::Stopped;
            }
        })
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
            Some(candidate)
        }
        ManagerRuntimeAction::RunPluginGcFailed
            if matches!(prev.phase, ManagerRuntimePhase::ConfigReady) =>
        {
            candidate.phase = ManagerRuntimePhase::Restoring;
            Some(candidate)
        }
        ManagerRuntimeAction::RunBootRestoreSucceeded
            if matches!(prev.phase, ManagerRuntimePhase::Restoring) =>
        {
            candidate.phase = ManagerRuntimePhase::Listening;
            Some(candidate)
        }
        ManagerRuntimeAction::RunBootRestoreFailed
            if matches!(prev.phase, ManagerRuntimePhase::Restoring) =>
        {
            candidate.phase = ManagerRuntimePhase::Listening;
            Some(candidate)
        }
        ManagerRuntimeAction::StartListening
            if matches!(prev.phase, ManagerRuntimePhase::ConfigReady) =>
        {
            candidate.phase = ManagerRuntimePhase::Listening;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_program_matches_transition_function() {
        let spec = ManagerRuntimeSpec::new();
        let program = spec.transition_program().expect("transition program");
        let initial = spec.initial_state();

        assert_eq!(
            program
                .evaluate(&initial, &ManagerRuntimeAction::LoadExistingConfig)
                .expect("evaluates"),
            transition_state(&initial, &ManagerRuntimeAction::LoadExistingConfig)
        );
        assert_eq!(
            program
                .evaluate(&initial, &ManagerRuntimeAction::FinishShutdown)
                .expect("evaluates"),
            transition_state(&initial, &ManagerRuntimeAction::FinishShutdown)
        );
    }
}
