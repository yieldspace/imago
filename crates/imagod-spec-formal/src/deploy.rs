use nirvash_core::{
    ActionConstraint, Fairness, Ltl, ModelCase, RelSet, StatePredicate, StepPredicate,
    TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelationalState, fairness, invariant, property, subsystem_spec,
};

use crate::atoms::ServiceAtom;

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
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
    pub fn release_promoted(&self, service: ServiceAtom) -> bool {
        self.promoted_releases.contains(&service)
    }

    pub fn rollback_pending(&self, service: ServiceAtom) -> bool {
        self.rollback_pending.contains(&service)
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

fn deploy_model_cases() -> Vec<ModelCase<DeployState, DeployAction>> {
    vec![
        ModelCase::default()
            .with_check_deadlocks(false)
            .with_action_constraint(ActionConstraint::new("service0_only", |_, action, _| {
                deploy_action_service(*action) == ServiceAtom::Service0
            })),
    ]
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

#[invariant(DeploySpec)]
fn release_phases_require_committed_upload() -> StatePredicate<DeployState> {
    StatePredicate::new("release_phases_require_committed_upload", |state| {
        state.prepared_releases.subset_of(&state.committed_uploads)
            && state.promoted_releases.subset_of(&state.committed_uploads)
            && state.rollback_pending.subset_of(&state.committed_uploads)
            && state.rolled_back.subset_of(&state.committed_uploads)
    })
}

#[invariant(DeploySpec)]
fn rollback_requires_auto_rollback() -> StatePredicate<DeployState> {
    StatePredicate::new("rollback_requires_auto_rollback", |state| {
        state
            .rollback_pending
            .subset_of(&state.auto_rollback_enabled)
    })
}

#[invariant(DeploySpec)]
fn release_phases_are_exclusive() -> StatePredicate<DeployState> {
    StatePredicate::new("release_phases_are_exclusive", |state| {
        pairwise_disjoint(&[
            &state.prepared_releases,
            &state.promoted_releases,
            &state.rollback_pending,
            &state.rolled_back,
        ])
    })
}

#[property(DeploySpec)]
fn prepared_release_leads_to_promoted_or_rolled_back() -> Ltl<DeployState, DeployAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("prepared_release_exists", |state| {
            state.prepared_releases.some()
        })),
        Ltl::pred(StatePredicate::new(
            "promoted_or_rolled_back_exists",
            |state| state.promoted_releases.some() || state.rolled_back.some(),
        )),
    )
}

#[property(DeploySpec)]
fn rollback_pending_leads_to_rolled_back() -> Ltl<DeployState, DeployAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("rollback_pending_exists", |state| {
            state.rollback_pending.some()
        })),
        Ltl::pred(StatePredicate::new("rolled_back_exists", |state| {
            state.rolled_back.some()
        })),
    )
}

#[fairness(DeploySpec)]
fn release_progress_fairness() -> Fairness<DeployState, DeployAction> {
    Fairness::weak(StepPredicate::new(
        "release_progress",
        |prev, action, next| {
            matches!(action, DeployAction::AdvanceRelease(_))
                && (next.prepared_releases.some() || next.promoted_releases.some())
                && (prev.prepared_releases != next.prepared_releases
                    || prev.promoted_releases != next.promoted_releases)
        },
    ))
}

#[fairness(DeploySpec)]
fn rollback_completion_fairness() -> Fairness<DeployState, DeployAction> {
    Fairness::weak(StepPredicate::new(
        "rollback_completion",
        |prev, action, next| {
            matches!(action, DeployAction::FinishRollback(_))
                && prev.rollback_pending != next.rollback_pending
                && next.rolled_back.some()
        },
    ))
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
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, prev: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_state(prev, action)
    }
}

#[nirvash_macros::formal_tests(spec = DeploySpec)]
const _: () = ();

fn transition_state(prev: &DeployState, action: &DeployAction) -> Option<DeployState> {
    let mut candidate = prev.clone();
    let allowed = match action {
        DeployAction::AdvanceUpload(service)
            if !prev.committed_uploads.contains(service)
                && !prev.complete_uploads.contains(service) =>
        {
            if prev.partial_uploads.contains(service) {
                candidate.partial_uploads.remove(service);
                candidate.complete_uploads.insert(*service);
            } else {
                candidate.partial_uploads.insert(*service);
            }
            true
        }
        DeployAction::CommitUpload(service)
            if prev.complete_uploads.contains(service)
                && !prev.committed_uploads.contains(service) =>
        {
            candidate.complete_uploads.remove(service);
            candidate.committed_uploads.insert(*service);
            true
        }
        DeployAction::AdvanceRelease(service)
            if prev.committed_uploads.contains(service)
                && !prev.promoted_releases.contains(service)
                && !prev.rollback_pending.contains(service) =>
        {
            if prev.prepared_releases.contains(service) {
                candidate.prepared_releases.remove(service);
                candidate.promoted_releases.insert(*service);
                candidate.rolled_back.remove(service);
            } else {
                candidate.prepared_releases.insert(*service);
            }
            true
        }
        DeployAction::SetRestartPolicy(service)
            if prev.committed_uploads.contains(service)
                && !prev.restart_policy_persisted.contains(service) =>
        {
            candidate.restart_policy_persisted.insert(*service);
            candidate.auto_rollback_enabled.insert(*service);
            true
        }
        DeployAction::TriggerRollback(service)
            if prev.promoted_releases.contains(service)
                && prev.auto_rollback_enabled.contains(service)
                && prev.restart_policy_persisted.contains(service)
                && !prev.rollback_pending.contains(service) =>
        {
            candidate.promoted_releases.remove(service);
            candidate.rollback_pending.insert(*service);
            true
        }
        DeployAction::FinishRollback(service) if prev.rollback_pending.contains(service) => {
            candidate.rollback_pending.remove(service);
            candidate.prepared_releases.remove(service);
            candidate.rolled_back.insert(*service);
            true
        }
        _ => false,
    };

    allowed.then_some(candidate).filter(deploy_valid)
}

fn deploy_valid(state: &DeployState) -> bool {
    release_phases_require_committed_upload().eval(state)
        && rollback_requires_auto_rollback().eval(state)
        && release_phases_are_exclusive().eval(state)
}

fn pairwise_disjoint<T>(sets: &[&RelSet<T>]) -> bool
where
    T: nirvash_core::RelAtom + Clone + Eq + std::fmt::Debug + 'static,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
