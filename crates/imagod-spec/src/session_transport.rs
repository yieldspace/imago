use nirvash_core::{Fairness, Ltl, StatePredicate, StepPredicate, TransitionSystem};
use nirvash_macros::{Signature, fairness, invariant, property, subsystem_spec};

use crate::bounds::SessionSlots;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum SessionOutcome {
    None,
    Accepted,
    RejectedTooMany,
    Joined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTransportState {
    pub active_sessions: SessionSlots,
    pub shutdown_requested: bool,
    pub last_outcome: SessionOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum SessionTransportAction {
    AcceptSession,
    RejectTooMany,
    JoinSession,
    BeginShutdown,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SessionTransportSpec;

impl SessionTransportSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> SessionTransportState {
        SessionTransportState {
            active_sessions: SessionSlots::new(0).expect("within bounds"),
            shutdown_requested: false,
            last_outcome: SessionOutcome::None,
        }
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
            || state.active_sessions.is_max()
    })
}

#[invariant(SessionTransportSpec)]
fn joined_implies_non_negative_sessions() -> StatePredicate<SessionTransportState> {
    StatePredicate::new("joined_implies_non_negative_sessions", |state| {
        !matches!(state.last_outcome, SessionOutcome::Joined) || state.active_sessions.get() < 2
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
            state.active_sessions.is_zero()
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
                || (state.shutdown_requested && state.active_sessions.is_zero())
        })),
    )
}

#[fairness(SessionTransportSpec)]
fn shutdown_drain_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(StepPredicate::new(
        "shutdown_drain_progress",
        |prev, action, next| {
            prev.shutdown_requested
                && !prev.active_sessions.is_zero()
                && matches!(action, SessionTransportAction::JoinSession)
                && next.active_sessions.get() < prev.active_sessions.get()
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
                && !prev.active_sessions.is_zero()
                && matches!(prev.last_outcome, SessionOutcome::Accepted)
                && matches!(action, SessionTransportAction::JoinSession)
                && next.active_sessions.get() < prev.active_sessions.get()
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
        action_vocabulary()
    }

    fn transition(&self, prev: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_state(prev, action)
    }
}

fn resolve_capacity_pressure() -> StepPredicate<SessionTransportState, SessionTransportAction> {
    StepPredicate::new("resolve_capacity_pressure", |prev, action, next| {
        (prev.active_sessions.is_max() || prev.shutdown_requested)
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

fn action_vocabulary() -> Vec<SessionTransportAction> {
    vec![
        SessionTransportAction::AcceptSession,
        SessionTransportAction::RejectTooMany,
        SessionTransportAction::JoinSession,
        SessionTransportAction::BeginShutdown,
    ]
}

fn transition_state(
    prev: &SessionTransportState,
    action: &SessionTransportAction,
) -> Option<SessionTransportState> {
    let mut candidate = *prev;
    match action {
        SessionTransportAction::AcceptSession
            if !prev.shutdown_requested && !prev.active_sessions.is_max() =>
        {
            candidate.active_sessions = prev.active_sessions.saturating_inc();
            candidate.last_outcome = SessionOutcome::Accepted;
            Some(candidate)
        }
        SessionTransportAction::RejectTooMany
            if prev.shutdown_requested || prev.active_sessions.is_max() =>
        {
            candidate.last_outcome = SessionOutcome::RejectedTooMany;
            Some(candidate)
        }
        SessionTransportAction::JoinSession if !prev.active_sessions.is_zero() => {
            candidate.active_sessions = prev.active_sessions.saturating_dec();
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
