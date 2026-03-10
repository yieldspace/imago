use nirvash_core::{
    Fairness, Ltl, RelAtom as _, RelSet, Signature as _, StatePredicate, StepPredicate,
    TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelAtom, RelationalState, Signature, fairness, invariant, property,
    subsystem_spec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, RelAtom)]
enum SessionAtom {
    Session0,
    Session1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum SessionOutcome {
    None,
    Accepted,
    RejectedTooMany,
    Joined,
}

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
pub struct SessionTransportState {
    active_sessions: RelSet<SessionAtom>,
    pub shutdown_requested: bool,
    pub last_outcome: SessionOutcome,
}

impl SessionTransportState {
    pub fn active_session_count(&self) -> usize {
        self.active_sessions.cardinality()
    }

    pub fn sessions_idle(&self) -> bool {
        self.active_sessions.no()
    }

    pub fn has_active_sessions(&self) -> bool {
        self.active_sessions.some()
    }

    pub fn at_capacity(&self) -> bool {
        self.active_session_count() == SessionAtom::rel_domain_len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum SessionTransportAction {
    /// Accept session
    AcceptSession,
    /// Reject session
    RejectTooMany,
    /// Join session
    JoinSession,
    /// Begin shutdown
    BeginShutdown,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SessionTransportSpec;

impl SessionTransportSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> SessionTransportState {
        SessionTransportState {
            active_sessions: RelSet::empty(),
            shutdown_requested: false,
            last_outcome: SessionOutcome::None,
        }
    }
}

impl SessionTransportState {
    pub fn from_summary(active_session: bool, shutdown_requested: bool) -> Self {
        let mut state = SessionTransportSpec::new().initial_state();
        if active_session {
            state.active_sessions.insert(SessionAtom::Session0);
            state.last_outcome = SessionOutcome::Accepted;
        }
        state.shutdown_requested = shutdown_requested;
        state
    }
}

#[invariant(SessionTransportSpec)]
fn shutdown_blocks_accept() -> StatePredicate<SessionTransportState> {
    StatePredicate::new("shutdown_blocks_accept", |state| {
        !state.shutdown_requested || !matches!(state.last_outcome, SessionOutcome::Accepted)
    })
}

#[invariant(SessionTransportSpec)]
fn too_many_means_full_or_stopping() -> StatePredicate<SessionTransportState> {
    StatePredicate::new("too_many_means_full_or_stopping", |state| {
        !matches!(state.last_outcome, SessionOutcome::RejectedTooMany)
            || state.shutdown_requested
            || state.at_capacity()
    })
}

#[invariant(SessionTransportSpec)]
fn joined_implies_capacity_reduced() -> StatePredicate<SessionTransportState> {
    StatePredicate::new("joined_implies_capacity_reduced", |state| {
        !matches!(state.last_outcome, SessionOutcome::Joined) || !state.at_capacity()
    })
}

#[property(SessionTransportSpec)]
fn shutdown_requested_leads_to_idle_sessions() -> Ltl<SessionTransportState, SessionTransportAction>
{
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("shutdown_requested", |state| {
            state.shutdown_requested
        })),
        Ltl::pred(StatePredicate::new("idle_sessions", |state| {
            state.sessions_idle()
        })),
    )
}

#[property(SessionTransportSpec)]
fn full_capacity_leads_to_resolution() -> Ltl<SessionTransportState, SessionTransportAction> {
    Ltl::always(Ltl::implies(
        Ltl::enabled(resolve_capacity_pressure()),
        Ltl::eventually(Ltl::pred(StatePredicate::new("join_or_reject", |state| {
            matches!(
                state.last_outcome,
                SessionOutcome::RejectedTooMany | SessionOutcome::Joined
            )
        }))),
    ))
}

#[property(SessionTransportSpec)]
fn accepted_session_leads_to_join_or_shutdown_drain()
-> Ltl<SessionTransportState, SessionTransportAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("accepted_session", |state| {
            matches!(state.last_outcome, SessionOutcome::Accepted)
        })),
        Ltl::pred(StatePredicate::new("joined_or_resolved", |state| {
            matches!(state.last_outcome, SessionOutcome::Joined)
                || matches!(state.last_outcome, SessionOutcome::RejectedTooMany)
                || (state.shutdown_requested && state.sessions_idle())
        })),
    )
}

#[fairness(SessionTransportSpec)]
fn shutdown_drain_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(StepPredicate::new(
        "shutdown_drain_progress",
        |prev, action, next| {
            prev.shutdown_requested
                && prev.has_active_sessions()
                && matches!(action, SessionTransportAction::JoinSession)
                && next.active_session_count() < prev.active_session_count()
        },
    ))
}

#[fairness(SessionTransportSpec)]
fn capacity_resolution_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(resolve_capacity_pressure())
}

#[fairness(SessionTransportSpec)]
fn accepted_session_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(StepPredicate::new(
        "accepted_session_progress",
        |prev, action, next| {
            !prev.shutdown_requested
                && prev.has_active_sessions()
                && matches!(prev.last_outcome, SessionOutcome::Accepted)
                && matches!(action, SessionTransportAction::JoinSession)
                && next.active_session_count() < prev.active_session_count()
        },
    ))
}

#[subsystem_spec]
impl TransitionSystem for SessionTransportSpec {
    type State = SessionTransportState;
    type Action = SessionTransportAction;

    fn name(&self) -> &'static str {
        "session_transport"
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

fn resolve_capacity_pressure() -> StepPredicate<SessionTransportState, SessionTransportAction> {
    StepPredicate::new("resolve_capacity_pressure", |prev, action, next| {
        (prev.at_capacity() || prev.shutdown_requested)
            && matches!(
                action,
                SessionTransportAction::RejectTooMany | SessionTransportAction::JoinSession
            )
            && matches!(
                next.last_outcome,
                SessionOutcome::RejectedTooMany | SessionOutcome::Joined
            )
    })
}

#[nirvash_macros::formal_tests(spec = SessionTransportSpec)]
const _: () = ();

fn next_free_session(state: &SessionTransportState) -> Option<SessionAtom> {
    SessionAtom::bounded_domain()
        .into_vec()
        .into_iter()
        .find(|session| !state.active_sessions.contains(session))
}

fn first_active_session(state: &SessionTransportState) -> Option<SessionAtom> {
    state.active_sessions.items().into_iter().next()
}

fn transition_state(
    prev: &SessionTransportState,
    action: &SessionTransportAction,
) -> Option<SessionTransportState> {
    let mut candidate = prev.clone();
    match action {
        SessionTransportAction::AcceptSession
            if !prev.shutdown_requested && !prev.at_capacity() =>
        {
            let next_session = next_free_session(prev)?;
            candidate.active_sessions.insert(next_session);
            candidate.last_outcome = SessionOutcome::Accepted;
            Some(candidate)
        }
        SessionTransportAction::RejectTooMany if prev.shutdown_requested || prev.at_capacity() => {
            candidate.last_outcome = SessionOutcome::RejectedTooMany;
            Some(candidate)
        }
        SessionTransportAction::JoinSession if prev.has_active_sessions() => {
            let session = first_active_session(prev)?;
            candidate.active_sessions.remove(&session);
            candidate.last_outcome = SessionOutcome::Joined;
            Some(candidate)
        }
        SessionTransportAction::BeginShutdown => {
            candidate.shutdown_requested = true;
            candidate.last_outcome = SessionOutcome::None;
            Some(candidate)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_and_join_use_deterministic_session_atoms() {
        let spec = SessionTransportSpec::new();
        let accepted_once = spec
            .transition(
                &spec.initial_state(),
                &SessionTransportAction::AcceptSession,
            )
            .expect("first accept");
        let accepted_twice = spec
            .transition(&accepted_once, &SessionTransportAction::AcceptSession)
            .expect("second accept");
        let joined = spec
            .transition(&accepted_twice, &SessionTransportAction::JoinSession)
            .expect("join should release one session");

        assert_eq!(
            accepted_once.active_sessions.items(),
            vec![SessionAtom::Session0]
        );
        assert_eq!(
            accepted_twice.active_sessions.items(),
            vec![SessionAtom::Session0, SessionAtom::Session1]
        );
        assert_eq!(joined.active_sessions.items(), vec![SessionAtom::Session1]);
    }
}
