use nirvash::{BoolExpr, Fairness, Ltl, ModelCase, RelSet, StepExpr, TransitionSystem};
use nirvash_macros::{
    ActionVocabulary, RelationalState, Signature as FormalSignature, action_constraint, fairness,
    invariant, nirvash_expr, nirvash_step_expr, nirvash_transition_program, property,
    subsystem_spec,
};

use crate::atoms::ServiceAtom;

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature, RelationalState)]
#[signature(custom)]
pub struct DeployState {
    partial_uploads: RelSet<ServiceAtom>,
    complete_uploads: RelSet<ServiceAtom>,
    committed_uploads: RelSet<ServiceAtom>,
    prepared_releases: RelSet<ServiceAtom>,
    promoted_releases: RelSet<ServiceAtom>,
    rollback_pending: RelSet<ServiceAtom>,
    rolled_back: RelSet<ServiceAtom>,
    restart_policy_persisted: RelSet<ServiceAtom>,
    auto_rollback_enabled: RelSet<ServiceAtom>,
}

impl DeployState {
    pub fn from_logs_summary(summary: &imagod_spec::LogsStateSummary) -> Self {
        let mut state = DeploySpec::new().initial_state();
        if summary.service_running {
            install_committed_promoted_release(&mut state, ServiceAtom::Service0);
        }
        state
    }

    pub fn from_runtime_summary(summary: &imagod_spec::RuntimeStateSummary) -> Self {
        let mut state = DeploySpec::new().initial_state();
        if summary.service0_promoted || summary.service0_running {
            install_runtime_release(&mut state, ServiceAtom::Service0);
        }
        if summary.service1_promoted || summary.service1_running {
            install_runtime_release(&mut state, ServiceAtom::Service1);
        }
        if summary.service0_rolled_back {
            state.promoted_releases.remove(&ServiceAtom::Service0);
            state.rollback_pending.remove(&ServiceAtom::Service0);
            state.rolled_back.insert(ServiceAtom::Service0);
        }
        state
    }

    pub fn release_promoted(&self, service: ServiceAtom) -> bool {
        self.promoted_releases.contains(&service)
    }

    pub fn rollback_pending(&self, service: ServiceAtom) -> bool {
        self.rollback_pending.contains(&service)
    }

    pub fn rollback_observed(&self, service: ServiceAtom) -> bool {
        self.rolled_back.contains(&service)
    }

    pub fn service_is_quiescent(&self, service: ServiceAtom) -> bool {
        !self.partial_uploads.contains(&service)
            && !self.complete_uploads.contains(&service)
            && !self.committed_uploads.contains(&service)
            && !self.prepared_releases.contains(&service)
            && !self.promoted_releases.contains(&service)
            && !self.rollback_pending.contains(&service)
            && !self.rolled_back.contains(&service)
            && !self.restart_policy_persisted.contains(&service)
            && !self.auto_rollback_enabled.contains(&service)
    }
}

fn install_committed_promoted_release(state: &mut DeployState, service: ServiceAtom) {
    state.committed_uploads.insert(service);
    state.promoted_releases.insert(service);
}

fn install_runtime_release(state: &mut DeployState, service: ServiceAtom) {
    install_committed_promoted_release(state, service);
    state.restart_policy_persisted.insert(service);
    state.auto_rollback_enabled.insert(service);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, nirvash_macros::Signature, ActionVocabulary)]
pub enum DeployAction {
    /// Advance upload from missing->partial or partial->complete.
    AdvanceUpload(ServiceAtom),
    /// Commit one completed upload.
    CommitUpload(ServiceAtom),
    /// Advance release from committed->prepared or prepared->promoted.
    AdvanceRelease(ServiceAtom),
    /// Persist restart policy metadata for one service.
    SetRestartPolicy(ServiceAtom),
    /// Trigger rollback for one promoted release.
    TriggerRollback(ServiceAtom),
    /// Finish rollback for one service.
    FinishRollback(ServiceAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DeploySpec;

impl DeploySpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> DeployState {
        DeployState {
            partial_uploads: RelSet::empty(),
            complete_uploads: RelSet::empty(),
            committed_uploads: RelSet::empty(),
            prepared_releases: RelSet::empty(),
            promoted_releases: RelSet::empty(),
            rollback_pending: RelSet::empty(),
            rolled_back: RelSet::empty(),
            restart_policy_persisted: RelSet::empty(),
            auto_rollback_enabled: RelSet::empty(),
        }
    }
}

nirvash::signature_spec!(
    DeployStateSignatureSpec for DeployState,
    representatives = crate::state_domain::reachable_state_domain(&DeploySpec::new())
);

nirvash::symbolic_state_spec!(for DeployState {
    partial_uploads: RelSet<ServiceAtom>,
    complete_uploads: RelSet<ServiceAtom>,
    committed_uploads: RelSet<ServiceAtom>,
    prepared_releases: RelSet<ServiceAtom>,
    promoted_releases: RelSet<ServiceAtom>,
    rollback_pending: RelSet<ServiceAtom>,
    rolled_back: RelSet<ServiceAtom>,
    restart_policy_persisted: RelSet<ServiceAtom>,
    auto_rollback_enabled: RelSet<ServiceAtom>,
});

fn deploy_model_cases() -> Vec<ModelCase<DeployState, DeployAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

fn deploy_action_service(action: DeployAction) -> ServiceAtom {
    match action {
        DeployAction::AdvanceUpload(service)
        | DeployAction::CommitUpload(service)
        | DeployAction::AdvanceRelease(service)
        | DeployAction::SetRestartPolicy(service)
        | DeployAction::TriggerRollback(service)
        | DeployAction::FinishRollback(service) => service,
    }
}

#[action_constraint(DeploySpec, cases("default"))]
fn service0_only() -> StepExpr<DeployState, DeployAction> {
    nirvash_step_expr! { service0_only(_prev, action, _next) =>
        deploy_action_service(*action) == ServiceAtom::Service0
    }
}

#[invariant(DeploySpec)]
fn release_phases_require_committed_upload() -> BoolExpr<DeployState> {
    nirvash_expr! { release_phases_require_committed_upload(state) =>
        state.prepared_releases.subset_of(&state.committed_uploads)
            && state.promoted_releases.subset_of(&state.committed_uploads)
            && state.rollback_pending.subset_of(&state.committed_uploads)
            && state.rolled_back.subset_of(&state.committed_uploads)
    }
}

#[invariant(DeploySpec)]
fn rollback_requires_auto_rollback() -> BoolExpr<DeployState> {
    nirvash_expr! { rollback_requires_auto_rollback(state) =>
        state.rollback_pending.subset_of(&state.auto_rollback_enabled)
    }
}

#[invariant(DeploySpec)]
fn release_phases_are_exclusive() -> BoolExpr<DeployState> {
    nirvash_expr! { release_phases_are_exclusive(state) =>
        pairwise_disjoint(&[
            &state.prepared_releases,
            &state.promoted_releases,
            &state.rollback_pending,
            &state.rolled_back,
        ])
    }
}

#[property(DeploySpec)]
fn prepared_release_leads_to_promoted_or_rolled_back() -> Ltl<DeployState, DeployAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { prepared_release_exists(state) =>
            state.prepared_releases.some()
        }),
        Ltl::pred(nirvash_expr! { promoted_or_rolled_back_exists(state) =>
            state.promoted_releases.some() || state.rolled_back.some()
        }),
    )
}

#[property(DeploySpec)]
fn rollback_pending_leads_to_rolled_back() -> Ltl<DeployState, DeployAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { rollback_pending_exists(state) =>
            state.rollback_pending.some()
        }),
        Ltl::pred(nirvash_expr! { rolled_back_exists(state) =>
            state.rolled_back.some()
        }),
    )
}

#[fairness(DeploySpec)]
fn release_progress_fairness() -> Fairness<DeployState, DeployAction> {
    Fairness::weak(nirvash_step_expr! { release_progress(prev, action, next) =>
        matches!(action, DeployAction::AdvanceRelease(_))
            && (next.prepared_releases.some() || next.promoted_releases.some())
            && (rel_set_changed(&prev.prepared_releases, &next.prepared_releases)
                || rel_set_changed(&prev.promoted_releases, &next.promoted_releases))
    })
}

#[fairness(DeploySpec)]
fn rollback_completion_fairness() -> Fairness<DeployState, DeployAction> {
    Fairness::weak(
        nirvash_step_expr! { rollback_completion(prev, action, next) =>
            matches!(action, DeployAction::FinishRollback(_))
                && rel_set_changed(&prev.rollback_pending, &next.rollback_pending)
                && next.rolled_back.some()
        },
    )
}

#[subsystem_spec(model_cases(deploy_model_cases))]
impl TransitionSystem for DeploySpec {
    type State = DeployState;
    type Action = DeployAction;

    fn name(&self) -> &'static str {
        "deploy"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start_partial_upload when advance_upload_target(action).is_some()
                && !prev.committed_uploads.contains(
                    &advance_upload_target(action)
                        .expect("start_partial_upload guard ensures target"),
                )
                && !prev.complete_uploads.contains(
                    &advance_upload_target(action)
                        .expect("start_partial_upload guard ensures target"),
                )
                && !prev.partial_uploads.contains(
                    &advance_upload_target(action)
                        .expect("start_partial_upload guard ensures target"),
                ) => {
                insert partial_uploads <= advance_upload_target(action)
                    .expect("start_partial_upload guard ensures target");
            }

            rule finish_upload when advance_upload_target(action).is_some()
                && !prev.committed_uploads.contains(
                    &advance_upload_target(action)
                        .expect("finish_upload guard ensures target"),
                )
                && !prev.complete_uploads.contains(
                    &advance_upload_target(action)
                        .expect("finish_upload guard ensures target"),
                )
                && prev.partial_uploads.contains(
                    &advance_upload_target(action)
                        .expect("finish_upload guard ensures target"),
                ) => {
                remove partial_uploads <= advance_upload_target(action)
                    .expect("finish_upload guard ensures target");
                insert complete_uploads <= advance_upload_target(action)
                    .expect("finish_upload guard ensures target");
            }

            rule commit_upload when commit_upload_target(action).is_some()
                && prev.complete_uploads.contains(
                    &commit_upload_target(action)
                        .expect("commit_upload guard ensures target"),
                )
                && !prev.committed_uploads.contains(
                    &commit_upload_target(action)
                        .expect("commit_upload guard ensures target"),
                ) => {
                remove complete_uploads <= commit_upload_target(action)
                    .expect("commit_upload guard ensures target");
                insert committed_uploads <= commit_upload_target(action)
                    .expect("commit_upload guard ensures target");
            }

            rule prepare_release when advance_release_target(action).is_some()
                && prev.committed_uploads.contains(
                    &advance_release_target(action)
                        .expect("prepare_release guard ensures target"),
                )
                && !prev.promoted_releases.contains(
                    &advance_release_target(action)
                        .expect("prepare_release guard ensures target"),
                )
                && !prev.rollback_pending.contains(
                    &advance_release_target(action)
                        .expect("prepare_release guard ensures target"),
                )
                && !prev.rolled_back.contains(
                    &advance_release_target(action)
                        .expect("prepare_release guard ensures target"),
                )
                && !prev.prepared_releases.contains(
                    &advance_release_target(action)
                        .expect("prepare_release guard ensures target"),
                ) => {
                insert prepared_releases <= advance_release_target(action)
                    .expect("prepare_release guard ensures target");
            }

            rule promote_release when advance_release_target(action).is_some()
                && prev.committed_uploads.contains(
                    &advance_release_target(action)
                        .expect("promote_release guard ensures target"),
                )
                && !prev.promoted_releases.contains(
                    &advance_release_target(action)
                        .expect("promote_release guard ensures target"),
                )
                && !prev.rollback_pending.contains(
                    &advance_release_target(action)
                        .expect("promote_release guard ensures target"),
                )
                && prev.prepared_releases.contains(
                    &advance_release_target(action)
                        .expect("promote_release guard ensures target"),
                ) => {
                remove prepared_releases <= advance_release_target(action)
                    .expect("promote_release guard ensures target");
                insert promoted_releases <= advance_release_target(action)
                    .expect("promote_release guard ensures target");
                remove rolled_back <= advance_release_target(action)
                    .expect("promote_release guard ensures target");
            }

            rule persist_restart_policy when set_restart_policy_target(action).is_some()
                && prev.committed_uploads.contains(
                    &set_restart_policy_target(action)
                        .expect("persist_restart_policy guard ensures target"),
                )
                && !prev.restart_policy_persisted.contains(
                    &set_restart_policy_target(action)
                        .expect("persist_restart_policy guard ensures target"),
                ) => {
                insert restart_policy_persisted <= set_restart_policy_target(action)
                    .expect("persist_restart_policy guard ensures target");
                insert auto_rollback_enabled <= set_restart_policy_target(action)
                    .expect("persist_restart_policy guard ensures target");
            }

            rule trigger_rollback when trigger_rollback_target(action).is_some()
                && prev.promoted_releases.contains(
                    &trigger_rollback_target(action)
                        .expect("trigger_rollback guard ensures target"),
                )
                && prev.auto_rollback_enabled.contains(
                    &trigger_rollback_target(action)
                        .expect("trigger_rollback guard ensures target"),
                )
                && prev.restart_policy_persisted.contains(
                    &trigger_rollback_target(action)
                        .expect("trigger_rollback guard ensures target"),
                )
                && !prev.rollback_pending.contains(
                    &trigger_rollback_target(action)
                        .expect("trigger_rollback guard ensures target"),
                ) => {
                remove promoted_releases <= trigger_rollback_target(action)
                    .expect("trigger_rollback guard ensures target");
                insert rollback_pending <= trigger_rollback_target(action)
                    .expect("trigger_rollback guard ensures target");
            }

            rule finish_rollback when finish_rollback_target(action).is_some()
                && prev.rollback_pending.contains(
                    &finish_rollback_target(action)
                        .expect("finish_rollback guard ensures target"),
                ) => {
                remove rollback_pending <= finish_rollback_target(action)
                    .expect("finish_rollback guard ensures target");
                remove prepared_releases <= finish_rollback_target(action)
                    .expect("finish_rollback guard ensures target");
                insert rolled_back <= finish_rollback_target(action)
                    .expect("finish_rollback guard ensures target");
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = DeploySpec)]
const _: () = ();

fn advance_upload_target(action: &DeployAction) -> Option<ServiceAtom> {
    match action {
        DeployAction::AdvanceUpload(service) => Some(*service),
        _ => None,
    }
}

fn commit_upload_target(action: &DeployAction) -> Option<ServiceAtom> {
    match action {
        DeployAction::CommitUpload(service) => Some(*service),
        _ => None,
    }
}

fn advance_release_target(action: &DeployAction) -> Option<ServiceAtom> {
    match action {
        DeployAction::AdvanceRelease(service) => Some(*service),
        _ => None,
    }
}

fn set_restart_policy_target(action: &DeployAction) -> Option<ServiceAtom> {
    match action {
        DeployAction::SetRestartPolicy(service) => Some(*service),
        _ => None,
    }
}

fn trigger_rollback_target(action: &DeployAction) -> Option<ServiceAtom> {
    match action {
        DeployAction::TriggerRollback(service) => Some(*service),
        _ => None,
    }
}

fn finish_rollback_target(action: &DeployAction) -> Option<ServiceAtom> {
    match action {
        DeployAction::FinishRollback(service) => Some(*service),
        _ => None,
    }
}

fn pairwise_disjoint<T>(sets: &[&RelSet<T>]) -> bool
where
    T: nirvash::RelAtom + Clone + Eq + std::fmt::Debug + 'static,
{
    for (index, left) in sets.iter().enumerate() {
        for right in sets.iter().skip(index + 1) {
            if left.items().iter().any(|item| right.contains(item)) {
                return false;
            }
        }
    }
    true
}

fn rel_set_changed<T>(left: &RelSet<T>, right: &RelSet<T>) -> bool
where
    T: nirvash::RelAtom + Clone + Eq + std::fmt::Debug + 'static,
{
    left.items() != right.items()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash::{ModelBackend, ModelCheckConfig};
    use nirvash_check::ModelChecker;

    #[test]
    fn rollback_only_touches_selected_service() {
        let spec = DeploySpec::new();
        let mut state = spec.initial_state();
        state.committed_uploads =
            RelSet::from_items([ServiceAtom::Service0, ServiceAtom::Service1]);
        state.promoted_releases =
            RelSet::from_items([ServiceAtom::Service0, ServiceAtom::Service1]);
        state.restart_policy_persisted = RelSet::from_items([ServiceAtom::Service0]);
        state.auto_rollback_enabled = RelSet::from_items([ServiceAtom::Service0]);

        let next = spec
            .transition(
                &state,
                &DeployAction::TriggerRollback(ServiceAtom::Service0),
            )
            .expect("rollback should start");

        assert!(next.rollback_pending(ServiceAtom::Service0));
        assert!(next.release_promoted(ServiceAtom::Service1));
    }

    #[test]
    fn advance_upload_completes_in_two_steps() {
        let spec = DeploySpec::new();
        let initial = spec.initial_state();
        let partial = spec
            .transition(
                &initial,
                &DeployAction::AdvanceUpload(ServiceAtom::Service0),
            )
            .expect("advance upload enters partial");
        assert!(partial.partial_uploads.contains(&ServiceAtom::Service0));
        assert!(!partial.complete_uploads.contains(&ServiceAtom::Service0));

        let complete = spec
            .transition(
                &partial,
                &DeployAction::AdvanceUpload(ServiceAtom::Service0),
            )
            .expect("second advance upload completes");
        assert!(!complete.partial_uploads.contains(&ServiceAtom::Service0));
        assert!(complete.complete_uploads.contains(&ServiceAtom::Service0));
    }

    #[test]
    fn explicit_and_symbolic_backends_agree() {
        let spec = DeploySpec::new();
        let explicit_snapshot = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .full_reachable_graph_snapshot()
        .expect("explicit deploy snapshot");
        let symbolic_snapshot = match ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .full_reachable_graph_snapshot()
        {
            Ok(snapshot) => snapshot,
            Err(nirvash::ModelCheckError::UnsupportedConfiguration(message))
                if message.contains("symbolic backend requires") =>
            {
                return;
            }
            Err(error) => panic!("symbolic deploy snapshot: {error:?}"),
        };
        assert_eq!(symbolic_snapshot, explicit_snapshot);

        let explicit_result = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .check_all()
        .expect("explicit deploy result");
        let symbolic_result = match ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .check_all()
        {
            Ok(result) => result,
            Err(nirvash::ModelCheckError::UnsupportedConfiguration(message))
                if message.contains("symbolic backend requires") =>
            {
                return;
            }
            Err(error) => panic!("symbolic deploy result: {error:?}"),
        };
        assert_eq!(symbolic_result, explicit_result);
    }
}
