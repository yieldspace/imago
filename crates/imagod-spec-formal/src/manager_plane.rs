use nirvash::{BoolExpr, Fairness, Ltl, TransitionProgram};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    SymbolicEncoding as FormalSymbolicEncoding, fairness, invariant, nirvash_expr,
    nirvash_step_expr, nirvash_transition_program, property, subsystem_spec,
};

use crate::bounds::{MAX_LASSO_DEPTH, MaintenanceTicks, doc_cap_focus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum ManagerPhase {
    Booting,
    ConfigReady,
    Restoring,
    Listening,
    Maintenance,
    ShutdownRequested,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub struct ManagerPlaneState {
    pub config_loaded: bool,
    pub created_default: bool,
    pub phase: ManagerPhase,
    pub accepts_control: bool,
    pub shutdown_started: bool,
    pub services_drained: bool,
    pub maintenance_stopped: bool,
    pub maintenance_ticks: MaintenanceTicks,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    ActionVocabulary,
)]
pub enum ManagerPlaneAction {
    /// Load an existing config.
    LoadExistingConfig,
    /// Create a default config.
    CreateDefaultConfig,
    /// Start boot restore.
    StartRestore,
    /// Finish restore and begin listening.
    FinishRestore,
    /// Run one maintenance tick.
    TickMaintenance,
    /// Start shutdown.
    BeginShutdown,
    /// Mark service drain complete.
    DrainServices,
    /// Stop maintenance workers.
    StopMaintenance,
    /// Finish shutdown.
    FinishShutdown,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ManagerPlaneSpec;

impl ManagerPlaneSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> ManagerPlaneState {
        ManagerPlaneState {
            config_loaded: false,
            created_default: false,
            phase: ManagerPhase::Booting,
            accepts_control: false,
            shutdown_started: false,
            services_drained: false,
            maintenance_stopped: false,
            maintenance_ticks: MaintenanceTicks::new(0).expect("within bounds"),
        }
    }
}

fn manager_plane_model_cases() -> Vec<ModelInstance<ManagerPlaneState, ManagerPlaneAction>> {
    vec![
        ModelInstance::new("explicit_boot_shutdown")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false),
        ModelInstance::new("symbolic_boot_shutdown")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Symbolic),
                ..nirvash::ModelCheckConfig::bounded_lasso(MAX_LASSO_DEPTH)
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false),
    ]
}

#[invariant(ManagerPlaneSpec)]
fn accepting_control_requires_active_phase() -> BoolExpr<ManagerPlaneState> {
    nirvash_expr! { accepting_control_requires_active_phase(state) =>
        !state.accepts_control
            || matches!(state.phase, ManagerPhase::Listening | ManagerPhase::Maintenance)
    }
}

#[invariant(ManagerPlaneSpec)]
fn stopped_requires_shutdown_completion() -> BoolExpr<ManagerPlaneState> {
    nirvash_expr! { stopped_requires_shutdown_completion(state) =>
        !matches!(state.phase, ManagerPhase::Stopped)
            || (state.shutdown_started && state.services_drained && state.maintenance_stopped)
    }
}

#[property(ManagerPlaneSpec)]
fn config_load_leads_to_listening() -> Ltl<ManagerPlaneState, ManagerPlaneAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { config_loaded(state) => state.config_loaded }),
        Ltl::pred(nirvash_expr! { listening_or_stopped(state) =>
            matches!(state.phase, ManagerPhase::Listening | ManagerPhase::Maintenance | ManagerPhase::Stopped)
        }),
    )
}

#[property(ManagerPlaneSpec)]
fn shutdown_started_leads_to_stopped() -> Ltl<ManagerPlaneState, ManagerPlaneAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { shutdown_started(state) => state.shutdown_started }),
        Ltl::pred(nirvash_expr! { stopped(state) => matches!(state.phase, ManagerPhase::Stopped) }),
    )
}

#[fairness(ManagerPlaneSpec)]
fn restore_progress() -> Fairness<ManagerPlaneState, ManagerPlaneAction> {
    Fairness::weak(nirvash_step_expr! { restore_progress(prev, action, next) =>
        matches!(prev.phase, ManagerPhase::Restoring)
            && matches!(action, ManagerPlaneAction::FinishRestore)
            && matches!(next.phase, ManagerPhase::Listening)
    })
}

#[fairness(ManagerPlaneSpec)]
fn config_ready_progress() -> Fairness<ManagerPlaneState, ManagerPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { config_ready_progress(prev, action, next) =>
            matches!(prev.phase, ManagerPhase::ConfigReady)
                && matches!(action, ManagerPlaneAction::StartRestore)
                && matches!(next.phase, ManagerPhase::Restoring)
        },
    )
}

#[fairness(ManagerPlaneSpec)]
fn shutdown_finish_progress() -> Fairness<ManagerPlaneState, ManagerPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { shutdown_finish_progress(prev, action, next) =>
            matches!(prev.phase, ManagerPhase::ShutdownRequested)
                && prev.services_drained
                && prev.maintenance_stopped
                && matches!(action, ManagerPlaneAction::FinishShutdown)
                && matches!(next.phase, ManagerPhase::Stopped)
        },
    )
}

#[fairness(ManagerPlaneSpec)]
fn shutdown_drain_progress() -> Fairness<ManagerPlaneState, ManagerPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { shutdown_drain_progress(prev, action, next) =>
            matches!(prev.phase, ManagerPhase::ShutdownRequested)
                && !prev.services_drained
                && matches!(action, ManagerPlaneAction::DrainServices)
                && next.services_drained
        },
    )
}

#[fairness(ManagerPlaneSpec)]
fn shutdown_maintenance_progress() -> Fairness<ManagerPlaneState, ManagerPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { shutdown_maintenance_progress(prev, action, next) =>
            matches!(prev.phase, ManagerPhase::ShutdownRequested)
                && !prev.maintenance_stopped
                && matches!(action, ManagerPlaneAction::StopMaintenance)
                && next.maintenance_stopped
        },
    )
}

#[subsystem_spec(model_cases(manager_plane_model_cases))]
impl FrontendSpec for ManagerPlaneSpec {
    type State = ManagerPlaneState;
    type Action = ManagerPlaneAction;

    fn frontend_name(&self) -> &'static str {
        "manager_plane"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule load_existing_config when matches!(action, ManagerPlaneAction::LoadExistingConfig)
                && matches!(prev.phase, ManagerPhase::Booting) => {
                set config_loaded <= true;
                set created_default <= false;
                set phase <= ManagerPhase::ConfigReady;
            }

            rule create_default_config when matches!(action, ManagerPlaneAction::CreateDefaultConfig)
                && matches!(prev.phase, ManagerPhase::Booting) => {
                set config_loaded <= true;
                set created_default <= true;
                set phase <= ManagerPhase::ConfigReady;
            }

            rule start_restore when matches!(action, ManagerPlaneAction::StartRestore)
                && matches!(prev.phase, ManagerPhase::ConfigReady)
                && prev.config_loaded => {
                set phase <= ManagerPhase::Restoring;
            }

            rule finish_restore when matches!(action, ManagerPlaneAction::FinishRestore)
                && matches!(prev.phase, ManagerPhase::Restoring) => {
                set phase <= ManagerPhase::Listening;
                set accepts_control <= true;
            }

            rule tick_maintenance when matches!(action, ManagerPlaneAction::TickMaintenance)
                && matches!(prev.phase, ManagerPhase::Listening | ManagerPhase::Maintenance)
                && !prev.shutdown_started
                && !prev.maintenance_ticks.is_max() => {
                set phase <= ManagerPhase::Maintenance;
                set maintenance_ticks <= prev.maintenance_ticks.saturating_inc();
            }

            rule begin_shutdown when matches!(action, ManagerPlaneAction::BeginShutdown)
                && matches!(prev.phase, ManagerPhase::Listening | ManagerPhase::Maintenance) => {
                set phase <= ManagerPhase::ShutdownRequested;
                set accepts_control <= false;
                set shutdown_started <= true;
            }

            rule drain_services when matches!(action, ManagerPlaneAction::DrainServices)
                && matches!(prev.phase, ManagerPhase::ShutdownRequested)
                && !prev.services_drained => {
                set services_drained <= true;
            }

            rule stop_maintenance when matches!(action, ManagerPlaneAction::StopMaintenance)
                && matches!(prev.phase, ManagerPhase::ShutdownRequested)
                && !prev.maintenance_stopped => {
                set maintenance_stopped <= true;
            }

            rule finish_shutdown when matches!(action, ManagerPlaneAction::FinishShutdown)
                && matches!(prev.phase, ManagerPhase::ShutdownRequested)
                && prev.services_drained
                && prev.maintenance_stopped => {
                set phase <= ManagerPhase::Stopped;
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = ManagerPlaneSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_check::ModelChecker;

    fn case_by_label(
        spec: &ManagerPlaneSpec,
        label: &str,
    ) -> nirvash_lower::ModelInstance<ManagerPlaneState, ManagerPlaneAction> {
        spec.model_instances()
            .into_iter()
            .find(|case| case.label() == label)
            .expect("model case should exist")
    }

    fn bounded_parity_case(
        case: nirvash_lower::ModelInstance<ManagerPlaneState, ManagerPlaneAction>,
    ) -> nirvash_lower::ModelInstance<ManagerPlaneState, ManagerPlaneAction> {
        let mut config = case.effective_checker_config();
        config.max_states = Some(64);
        config.max_transitions = Some(256);
        case.with_checker_config(config)
    }

    #[test]
    fn explicit_and_symbolic_backends_agree() {
        let spec = ManagerPlaneSpec::new();
        let lowered = crate::lowered_spec(&spec);
        let explicit_case = bounded_parity_case(case_by_label(&spec, "explicit_boot_shutdown"));
        let symbolic_case = bounded_parity_case(case_by_label(&spec, "symbolic_boot_shutdown"));

        let explicit_snapshot = ModelChecker::for_case(&lowered, explicit_case.clone())
            .reachable_graph_snapshot()
            .expect("explicit manager snapshot");
        let symbolic_snapshot = ModelChecker::for_case(&lowered, symbolic_case.clone())
            .reachable_graph_snapshot()
            .expect("symbolic manager snapshot");
        assert_eq!(symbolic_snapshot, explicit_snapshot);
    }
}
