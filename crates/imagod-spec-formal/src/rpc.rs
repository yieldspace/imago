use nirvash_core::{
    ActionConstraint, Fairness, Ltl, ModelCase, RelSet, Relation2, Signature as _, StatePredicate,
    StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelationalState, action_constraint, fairness, invariant, property,
    subsystem_spec,
};

use crate::atoms::{
    BindingTargetAtom, RemoteAuthorityAtom, RpcCallAtom, RpcConnectionAtom, ServiceAtom,
    binding_target_for,
};

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
pub struct RpcState {
    bindings: Relation2<ServiceAtom, BindingTargetAtom>,
    local_call_owners: Relation2<RpcCallAtom, ServiceAtom>,
    local_call_targets: Relation2<RpcCallAtom, BindingTargetAtom>,
    denied_local_calls: Relation2<RpcCallAtom, ServiceAtom>,
    remote_connection_owners: Relation2<RpcConnectionAtom, ServiceAtom>,
    remote_connection_authorities: Relation2<RpcConnectionAtom, RemoteAuthorityAtom>,
    remote_call_owners: Relation2<RpcCallAtom, ServiceAtom>,
    remote_call_targets: Relation2<RpcCallAtom, BindingTargetAtom>,
    remote_inflight_calls: Relation2<RpcCallAtom, RpcConnectionAtom>,
    completed_remote_calls: RelSet<RpcCallAtom>,
    denied_remote_calls: Relation2<RpcCallAtom, ServiceAtom>,
}

impl RpcState {
    pub fn binding_allowed(&self, source: ServiceAtom) -> bool {
        self.bindings.contains(&source, &binding_target_for(source))
    }

    pub fn has_local_resolution_for(&self, source: ServiceAtom) -> bool {
        self.local_call_owners
            .pairs()
            .iter()
            .any(|(_, owner)| *owner == source)
    }

    pub fn has_denied_local_call_for(&self, source: ServiceAtom) -> bool {
        self.denied_local_calls
            .pairs()
            .iter()
            .any(|(_, owner)| *owner == source)
    }

    pub fn has_remote_connection_for(&self, source: ServiceAtom) -> bool {
        self.remote_connection_owners
            .pairs()
            .iter()
            .any(|(_, owner)| *owner == source)
    }

    pub fn has_completed_remote_call_for(&self, source: ServiceAtom) -> bool {
        self.remote_call_owners
            .pairs()
            .iter()
            .any(|(call, owner)| *owner == source && self.completed_remote_calls.contains(call))
    }

    pub fn has_denied_remote_call_for(&self, source: ServiceAtom) -> bool {
        self.denied_remote_calls
            .pairs()
            .iter()
            .any(|(_, owner)| *owner == source)
    }

    pub fn service_is_quiescent(&self, source: ServiceAtom) -> bool {
        !self.binding_allowed(source)
            && !self.has_local_resolution_for(source)
            && !self.has_denied_local_call_for(source)
            && !self.has_remote_connection_for(source)
            && !self.has_completed_remote_call_for(source)
            && !self.has_denied_remote_call_for(source)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, nirvash_macros::Signature, ActionVocabulary)]
pub enum RpcAction {
    /// Grant the default binding for one source service.
    GrantBinding(ServiceAtom),
    /// Resolve one local RPC call.
    ResolveLocal(ServiceAtom),
    /// Reject one local RPC call.
    RejectLocal(ServiceAtom),
    /// Open one remote RPC connection.
    ConnectRemote(ServiceAtom),
    /// Invoke one remote RPC call.
    InvokeRemote(ServiceAtom),
    /// Reject one remote RPC call.
    RejectRemoteInvoke(ServiceAtom),
    /// Mark one remote RPC call as completed.
    CompleteRemoteCall(ServiceAtom),
    /// Disconnect one remote RPC connection.
    DisconnectRemote(ServiceAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RpcSpec;

impl RpcSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> RpcState {
        RpcState {
            bindings: Relation2::empty(),
            local_call_owners: Relation2::empty(),
            local_call_targets: Relation2::empty(),
            denied_local_calls: Relation2::empty(),
            remote_connection_owners: Relation2::empty(),
            remote_connection_authorities: Relation2::empty(),
            remote_call_owners: Relation2::empty(),
            remote_call_targets: Relation2::empty(),
            remote_inflight_calls: Relation2::empty(),
            completed_remote_calls: RelSet::empty(),
            denied_remote_calls: Relation2::empty(),
        }
    }
}

fn rpc_model_cases() -> Vec<ModelCase<RpcState, RpcAction>> {
    vec![local_rpc_model_case(), remote_rpc_model_case()]
}

fn local_rpc_model_case() -> ModelCase<RpcState, RpcAction> {
    ModelCase::new("local_service0_only").with_check_deadlocks(false)
}

fn remote_rpc_model_case() -> ModelCase<RpcState, RpcAction> {
    ModelCase::new("remote_service0_only").with_check_deadlocks(false)
}

#[action_constraint(RpcSpec, cases("local_service0_only"))]
fn local_service0_only() -> ActionConstraint<RpcState, RpcAction> {
    ActionConstraint::new("local_service0_only", |_, action, _| {
        matches!(
            action,
            RpcAction::GrantBinding(ServiceAtom::Service0)
                | RpcAction::ResolveLocal(ServiceAtom::Service0)
                | RpcAction::RejectLocal(ServiceAtom::Service0)
        )
    })
}

#[action_constraint(RpcSpec, cases("remote_service0_only"))]
fn remote_service0_only() -> ActionConstraint<RpcState, RpcAction> {
    ActionConstraint::new("remote_service0_only", |_, action, _| {
        matches!(
            action,
            RpcAction::GrantBinding(ServiceAtom::Service0)
                | RpcAction::ConnectRemote(ServiceAtom::Service0)
                | RpcAction::InvokeRemote(ServiceAtom::Service0)
                | RpcAction::RejectRemoteInvoke(ServiceAtom::Service0)
                | RpcAction::CompleteRemoteCall(ServiceAtom::Service0)
                | RpcAction::DisconnectRemote(ServiceAtom::Service0)
        )
    })
}

#[invariant(RpcSpec)]
fn remote_inflight_requires_owner_and_target() -> StatePredicate<RpcState> {
    StatePredicate::new("remote_inflight_requires_owner_and_target", |state| {
        state.remote_inflight_calls.pairs().iter().all(|(call, _)| {
            state.remote_call_owners.domain().contains(call)
                && state.remote_call_targets.domain().contains(call)
        })
    })
}

#[invariant(RpcSpec)]
fn denied_and_completed_remote_calls_are_disjoint() -> StatePredicate<RpcState> {
    StatePredicate::new("denied_and_completed_remote_calls_are_disjoint", |state| {
        state
            .denied_remote_calls
            .pairs()
            .iter()
            .all(|(call, _)| !state.completed_remote_calls.contains(call))
    })
}

#[invariant(RpcSpec)]
fn bindings_match_default_target_pairs() -> StatePredicate<RpcState> {
    StatePredicate::new("bindings_match_default_target_pairs", |state| {
        state
            .bindings
            .pairs()
            .iter()
            .all(|(source, target)| *target == binding_target_for(*source))
    })
}

#[property(RpcSpec)]
fn local_resolution_eventually_happens_or_is_rejected() -> Ltl<RpcState, RpcAction> {
    Ltl::always(Ltl::implies(
        Ltl::enabled(resolve_or_reject_local_step()),
        Ltl::eventually(Ltl::pred(StatePredicate::new(
            "local_outcome_observed",
            |state| state.local_call_owners.some() || state.denied_local_calls.some(),
        ))),
    ))
}

#[property(RpcSpec)]
fn remote_invoke_leads_to_completion_or_rejection() -> Ltl<RpcState, RpcAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("remote_connection_exists", |state| {
            state.remote_connection_owners.some()
        })),
        Ltl::pred(StatePredicate::new("remote_outcome_exists", |state| {
            state.completed_remote_calls.some() || state.denied_remote_calls.some()
        })),
    )
}

#[fairness(RpcSpec)]
fn remote_completion_fairness() -> Fairness<RpcState, RpcAction> {
    Fairness::weak(StepPredicate::new(
        "remote_completion",
        |prev, action, next| {
            matches!(action, RpcAction::CompleteRemoteCall(_))
                && prev.completed_remote_calls != next.completed_remote_calls
        },
    ))
}

#[fairness(RpcSpec)]
fn local_resolution_fairness() -> Fairness<RpcState, RpcAction> {
    Fairness::weak(resolve_or_reject_local_step())
}

#[fairness(RpcSpec)]
fn remote_resolution_fairness() -> Fairness<RpcState, RpcAction> {
    Fairness::weak(resolve_or_reject_remote_step())
}

#[fairness(RpcSpec)]
fn remote_disconnect_fairness() -> Fairness<RpcState, RpcAction> {
    Fairness::weak(StepPredicate::new(
        "remote_disconnect",
        |prev, action, next| {
            matches!(action, RpcAction::DisconnectRemote(_))
                && prev.remote_connection_owners != next.remote_connection_owners
        },
    ))
}

#[subsystem_spec(model_cases(rpc_model_cases))]
impl TransitionSystem for RpcSpec {
    type State = RpcState;
    type Action = RpcAction;

    fn name(&self) -> &'static str {
        "rpc"
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

#[nirvash_macros::formal_tests(spec = RpcSpec)]
const _: () = ();

fn transition_state(prev: &RpcState, action: &RpcAction) -> Option<RpcState> {
    let mut candidate = prev.clone();
    let allowed = match action {
        RpcAction::GrantBinding(source) if !prev.binding_allowed(*source) => {
            candidate
                .bindings
                .insert(*source, binding_target_for(*source));
            true
        }
        RpcAction::ResolveLocal(source)
            if prev.binding_allowed(*source) && !prev.has_local_resolution_for(*source) =>
        {
            let call = next_free_call(prev)?;
            candidate.local_call_owners.insert(call, *source);
            candidate
                .local_call_targets
                .insert(call, binding_target_for(*source));
            true
        }
        RpcAction::RejectLocal(source) if !prev.has_local_resolution_for(*source) => {
            let call = next_free_call(prev)?;
            candidate.denied_local_calls.insert(call, *source);
            true
        }
        RpcAction::ConnectRemote(source) if !prev.has_remote_connection_for(*source) => {
            let connection = next_free_connection(prev)?;
            candidate
                .remote_connection_owners
                .insert(connection, *source);
            candidate
                .remote_connection_authorities
                .insert(connection, authority_for(*source));
            true
        }
        RpcAction::InvokeRemote(source)
            if prev.binding_allowed(*source) && prev.has_remote_connection_for(*source) =>
        {
            let call = next_free_call(prev)?;
            let connection = connection_for_source(prev, *source)?;
            candidate.remote_call_owners.insert(call, *source);
            candidate
                .remote_call_targets
                .insert(call, binding_target_for(*source));
            candidate.remote_inflight_calls.insert(call, connection);
            true
        }
        RpcAction::RejectRemoteInvoke(source) => {
            let call = next_free_call(prev)?;
            candidate.denied_remote_calls.insert(call, *source);
            true
        }
        RpcAction::CompleteRemoteCall(source) => {
            let call = inflight_call_for_source(prev, *source)?;
            let connection = inflight_connection(prev, call)?;
            candidate.remote_inflight_calls.remove(&call, &connection);
            candidate.completed_remote_calls.insert(call);
            true
        }
        RpcAction::DisconnectRemote(source)
            if prev.has_remote_connection_for(*source)
                && inflight_call_for_source(prev, *source).is_none()
                && (prev.has_completed_remote_call_for(*source)
                    || prev.has_denied_remote_call_for(*source)) =>
        {
            let connection = connection_for_source(prev, *source)?;
            candidate
                .remote_connection_owners
                .remove(&connection, source);
            candidate
                .remote_connection_authorities
                .remove(&connection, &authority_for(*source));
            true
        }
        _ => false,
    };

    allowed.then_some(candidate).filter(rpc_valid)
}

fn rpc_valid(state: &RpcState) -> bool {
    remote_inflight_requires_owner_and_target().eval(state)
        && denied_and_completed_remote_calls_are_disjoint().eval(state)
        && bindings_match_default_target_pairs().eval(state)
}

fn resolve_or_reject_local_step() -> StepPredicate<RpcState, RpcAction> {
    StepPredicate::new("resolve_or_reject_local", |prev, action, next| {
        matches!(
            action,
            RpcAction::ResolveLocal(_) | RpcAction::RejectLocal(_)
        ) && (prev.local_call_owners != next.local_call_owners
            || prev.denied_local_calls != next.denied_local_calls)
    })
}

fn resolve_or_reject_remote_step() -> StepPredicate<RpcState, RpcAction> {
    StepPredicate::new("resolve_or_reject_remote", |prev, action, next| {
        matches!(
            action,
            RpcAction::InvokeRemote(_) | RpcAction::RejectRemoteInvoke(_)
        ) && (prev.remote_inflight_calls != next.remote_inflight_calls
            || prev.denied_remote_calls != next.denied_remote_calls)
    })
}

fn next_free_call(state: &RpcState) -> Option<RpcCallAtom> {
    RpcCallAtom::bounded_domain()
        .into_vec()
        .into_iter()
        .find(|call| {
            !state.local_call_owners.domain().contains(call)
                && !state.denied_local_calls.domain().contains(call)
                && !state.remote_call_owners.domain().contains(call)
                && !state.denied_remote_calls.domain().contains(call)
                && !state.completed_remote_calls.contains(call)
        })
}

fn next_free_connection(state: &RpcState) -> Option<RpcConnectionAtom> {
    RpcConnectionAtom::bounded_domain()
        .into_vec()
        .into_iter()
        .find(|connection| !state.remote_connection_owners.domain().contains(connection))
}

fn connection_for_source(state: &RpcState, source: ServiceAtom) -> Option<RpcConnectionAtom> {
    state
        .remote_connection_owners
        .pairs()
        .into_iter()
        .find_map(|(connection, owner)| (owner == source).then_some(connection))
}

fn inflight_call_for_source(state: &RpcState, source: ServiceAtom) -> Option<RpcCallAtom> {
    state
        .remote_call_owners
        .pairs()
        .into_iter()
        .find_map(|(call, owner)| {
            (owner == source && state.remote_inflight_calls.domain().contains(&call))
                .then_some(call)
        })
}

fn inflight_connection(state: &RpcState, call: RpcCallAtom) -> Option<RpcConnectionAtom> {
    state
        .remote_inflight_calls
        .pairs()
        .into_iter()
        .find_map(|(candidate_call, connection)| (candidate_call == call).then_some(connection))
}

fn authority_for(source: ServiceAtom) -> RemoteAuthorityAtom {
    match source {
        ServiceAtom::Service0 => RemoteAuthorityAtom::Edge0,
        ServiceAtom::Service1 => RemoteAuthorityAtom::Edge1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_connection_lifecycle_tracks_owner() {
        let spec = RpcSpec::new();
        let bound = spec
            .transition(
                &spec.initial_state(),
                &RpcAction::GrantBinding(ServiceAtom::Service0),
            )
            .expect("binding");
        let connected = spec
            .transition(&bound, &RpcAction::ConnectRemote(ServiceAtom::Service0))
            .expect("connect");
        let invoked = spec
            .transition(&connected, &RpcAction::InvokeRemote(ServiceAtom::Service0))
            .expect("invoke");
        let completed = spec
            .transition(
                &invoked,
                &RpcAction::CompleteRemoteCall(ServiceAtom::Service0),
            )
            .expect("complete");
        let disconnected = spec
            .transition(
                &completed,
                &RpcAction::DisconnectRemote(ServiceAtom::Service0),
            )
            .expect("disconnect");

        assert!(completed.has_completed_remote_call_for(ServiceAtom::Service0));
        assert!(!disconnected.has_remote_connection_for(ServiceAtom::Service0));
    }
}
