use imago_formal_core::{
    BoundedDomain, Fairness, Ltl, Signature as FormalSignature, StatePredicate, StepPredicate,
    TransitionSystem,
};
use imago_formal_macros::{
    Signature, imago_fairness, imago_illegal, imago_invariant, imago_property, imago_subsystem_spec,
};

use crate::bounds::SessionSlots;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum SessionOutcome {
    None,
    Accepted,
    RejectedTooMany,
    Joined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
#[signature(custom)]
pub struct SessionTransportState {
    pub active_sessions: SessionSlots,
    pub shutdown_requested: bool,
    pub last_outcome: SessionOutcome,
}

impl SessionTransportStateSignatureSpec for SessionTransportState {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            SessionTransportSpec::new().initial_state(),
            Self {
                active_sessions: SessionSlots::new(1).expect("within bounds"),
                shutdown_requested: false,
                last_outcome: SessionOutcome::Accepted,
            },
            Self {
                active_sessions: SessionSlots::new(2).expect("within bounds"),
                shutdown_requested: false,
                last_outcome: SessionOutcome::RejectedTooMany,
            },
            Self {
                active_sessions: SessionSlots::new(1).expect("within bounds"),
                shutdown_requested: true,
                last_outcome: SessionOutcome::Joined,
            },
            Self {
                active_sessions: SessionSlots::new(0).expect("within bounds"),
                shutdown_requested: false,
                last_outcome: SessionOutcome::Joined,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        !self.shutdown_requested || !matches!(self.last_outcome, SessionOutcome::Accepted)
    }
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

#[imago_invariant]
fn shutdown_blocks_accept() -> StatePredicate<SessionTransportState> {
    StatePredicate::new("shutdown_blocks_accept", |state| {
        !state.shutdown_requested || !matches!(state.last_outcome, SessionOutcome::Accepted)
    })
}

#[imago_invariant]
fn too_many_means_full_or_stopping() -> StatePredicate<SessionTransportState> {
    StatePredicate::new("too_many_means_full_or_stopping", |state| {
        !matches!(state.last_outcome, SessionOutcome::RejectedTooMany)
            || state.shutdown_requested
            || state.active_sessions.is_max()
    })
}

#[imago_invariant]
fn joined_implies_non_negative_sessions() -> StatePredicate<SessionTransportState> {
    StatePredicate::new("joined_implies_non_negative_sessions", |state| {
        !matches!(state.last_outcome, SessionOutcome::Joined) || state.active_sessions.get() < 2
    })
}

#[imago_illegal]
fn accept_after_shutdown() -> StepPredicate<SessionTransportState, SessionTransportAction> {
    StepPredicate::new("accept_after_shutdown", |prev, action, _| {
        matches!(action, SessionTransportAction::AcceptSession) && prev.shutdown_requested
    })
}

#[imago_illegal]
fn accept_over_capacity() -> StepPredicate<SessionTransportState, SessionTransportAction> {
    StepPredicate::new("accept_over_capacity", |prev, action, _| {
        matches!(action, SessionTransportAction::AcceptSession) && prev.active_sessions.is_max()
    })
}

#[imago_illegal]
fn join_when_idle() -> StepPredicate<SessionTransportState, SessionTransportAction> {
    StepPredicate::new("join_when_idle", |prev, action, _| {
        matches!(action, SessionTransportAction::JoinSession) && prev.active_sessions.is_zero()
    })
}

#[imago_property]
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

#[imago_property]
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

#[imago_property]
fn accepted_session_leads_to_join_or_shutdown_drain()
-> Ltl<SessionTransportState, SessionTransportAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("accepted_session", |state| {
            matches!(state.last_outcome, SessionOutcome::Accepted)
        })),
        Ltl::pred(StatePredicate::new("joined_or_drained", |state| {
            matches!(state.last_outcome, SessionOutcome::Joined)
                || (state.shutdown_requested && state.active_sessions.is_zero())
        })),
    )
}

#[imago_fairness]
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

#[imago_fairness]
fn capacity_resolution_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(resolve_capacity_pressure())
}

#[imago_fairness]
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

#[imago_subsystem_spec(
    invariants(
        shutdown_blocks_accept,
        too_many_means_full_or_stopping,
        joined_implies_non_negative_sessions
    ),
    illegal(accept_after_shutdown, accept_over_capacity, join_when_idle),
    properties(
        shutdown_requested_leads_to_idle_sessions,
        full_capacity_leads_to_resolution,
        accepted_session_leads_to_join_or_shutdown_drain
    ),
    fairness(
        shutdown_drain_progress,
        capacity_resolution_progress,
        accepted_session_progress
    )
)]
impl TransitionSystem for SessionTransportSpec {
    type State = SessionTransportState;
    type Action = SessionTransportAction;

    fn name(&self) -> &'static str {
        "session_transport"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            SessionTransportAction::AcceptSession
                if !prev.shutdown_requested && !prev.active_sessions.is_max() =>
            {
                candidate.active_sessions = prev.active_sessions.saturating_inc();
                candidate.last_outcome = SessionOutcome::Accepted;
            }
            SessionTransportAction::RejectTooMany
                if prev.shutdown_requested || prev.active_sessions.is_max() =>
            {
                candidate.last_outcome = SessionOutcome::RejectedTooMany;
            }
            SessionTransportAction::JoinSession if !prev.active_sessions.is_zero() => {
                candidate.active_sessions = prev.active_sessions.saturating_dec();
                candidate.last_outcome = SessionOutcome::Joined;
            }
            SessionTransportAction::BeginShutdown => {
                candidate.shutdown_requested = true;
                candidate.last_outcome = SessionOutcome::None;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
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

#[cfg(test)]
#[imago_formal_macros::imago_formal_tests(spec = SessionTransportSpec, init = initial_state)]
const _: () = ();
