use nirvash::{BoolExpr, Fairness, Ltl, RelAtom as _, RelSet, StepExpr, TransitionProgram};
use nirvash_lower::{FiniteModelDomain as _, FrontendSpec, ModelInstance};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain, RelAtom, RelationalState,
    SymbolicEncoding as FormalSymbolicEncoding, fairness, invariant, nirvash_expr,
    nirvash_step_expr, nirvash_transition_program, property, subsystem_spec,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding, RelAtom,
)]
enum SessionAtom {
    Session0,
    Session1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum SessionOutcome {
    None,
    Accepted,
    RejectedTooMany,
    Joined,
}

#[derive(
    Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding, RelationalState,
)]
#[finite_model_domain(custom)]
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

nirvash::finite_model_domain_spec!(
    SessionTransportStateFiniteModelDomainSpec for SessionTransportState,
    representatives = crate::state_domain::reachable_state_domain(&SessionTransportSpec::new())
);

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
fn shutdown_blocks_accept() -> BoolExpr<SessionTransportState> {
    nirvash_expr! { shutdown_blocks_accept(state) =>
        !state.shutdown_requested || !matches!(state.last_outcome, SessionOutcome::Accepted)
    }
}

#[invariant(SessionTransportSpec)]
fn too_many_means_full_or_stopping() -> BoolExpr<SessionTransportState> {
    nirvash_expr! { too_many_means_full_or_stopping(state) =>
        !matches!(state.last_outcome, SessionOutcome::RejectedTooMany)
            || state.shutdown_requested
            || state.at_capacity()
    }
}

#[invariant(SessionTransportSpec)]
fn joined_implies_capacity_reduced() -> BoolExpr<SessionTransportState> {
    nirvash_expr! { joined_implies_capacity_reduced(state) =>
        !matches!(state.last_outcome, SessionOutcome::Joined) || !state.at_capacity()
    }
}

#[property(SessionTransportSpec)]
fn shutdown_requested_leads_to_idle_sessions() -> Ltl<SessionTransportState, SessionTransportAction>
{
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { shutdown_requested(state) => state.shutdown_requested }),
        Ltl::pred(nirvash_expr! { idle_sessions(state) => state.sessions_idle() }),
    )
}

#[property(SessionTransportSpec)]
fn full_capacity_leads_to_resolution() -> Ltl<SessionTransportState, SessionTransportAction> {
    Ltl::always(Ltl::implies(
        Ltl::enabled(resolve_capacity_pressure()),
        Ltl::eventually(Ltl::pred(nirvash_expr! { join_or_reject(state) =>
            matches!(
                state.last_outcome,
                SessionOutcome::RejectedTooMany | SessionOutcome::Joined
            )
        })),
    ))
}

#[property(SessionTransportSpec)]
fn accepted_session_leads_to_join_or_shutdown_drain()
-> Ltl<SessionTransportState, SessionTransportAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { accepted_session(state) =>
            matches!(state.last_outcome, SessionOutcome::Accepted)
        }),
        Ltl::pred(nirvash_expr! { joined_or_resolved(state) =>
            matches!(state.last_outcome, SessionOutcome::Joined)
                || matches!(state.last_outcome, SessionOutcome::RejectedTooMany)
                || (state.shutdown_requested && state.sessions_idle())
        }),
    )
}

#[fairness(SessionTransportSpec)]
fn shutdown_drain_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(
        nirvash_step_expr! { shutdown_drain_progress(prev, action, next) =>
            prev.shutdown_requested
                && prev.has_active_sessions()
                && matches!(action, SessionTransportAction::JoinSession)
                && next.active_session_count() < prev.active_session_count()
        },
    )
}

#[fairness(SessionTransportSpec)]
fn capacity_resolution_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(resolve_capacity_pressure())
}

#[fairness(SessionTransportSpec)]
fn accepted_session_progress() -> Fairness<SessionTransportState, SessionTransportAction> {
    Fairness::weak(
        nirvash_step_expr! { accepted_session_progress(prev, action, next) =>
            !prev.shutdown_requested
                && prev.has_active_sessions()
                && matches!(prev.last_outcome, SessionOutcome::Accepted)
                && matches!(action, SessionTransportAction::JoinSession)
                && next.active_session_count() < prev.active_session_count()
        },
    )
}

#[subsystem_spec]
impl FrontendSpec for SessionTransportSpec {
    type State = SessionTransportState;
    type Action = SessionTransportAction;

    fn frontend_name(&self) -> &'static str {
        "session_transport"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule accept_session when matches!(action, SessionTransportAction::AcceptSession)
                && !prev.shutdown_requested
                && !prev.at_capacity()
                && next_free_session(prev).is_some() => {
                insert active_sessions <= next_free_session(prev)
                    .expect("accept_session guard ensures a free session");
                set last_outcome <= SessionOutcome::Accepted;
            }

            rule reject_too_many when matches!(action, SessionTransportAction::RejectTooMany)
                && (prev.shutdown_requested || prev.at_capacity()) => {
                set last_outcome <= SessionOutcome::RejectedTooMany;
            }

            rule join_session when matches!(action, SessionTransportAction::JoinSession)
                && prev.has_active_sessions()
                && first_active_session(prev).is_some() => {
                remove active_sessions <= first_active_session(prev)
                    .expect("join_session guard ensures an active session");
                set last_outcome <= SessionOutcome::Joined;
            }

            rule begin_shutdown when matches!(action, SessionTransportAction::BeginShutdown) => {
                set shutdown_requested <= true;
                set last_outcome <= SessionOutcome::None;
            }
        })
    }

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        vec![ModelInstance::default().with_check_deadlocks(false)]
    }
}

fn resolve_capacity_pressure() -> StepExpr<SessionTransportState, SessionTransportAction> {
    nirvash_step_expr! { resolve_capacity_pressure(prev, action, next) =>
        (prev.at_capacity() || prev.shutdown_requested)
            && matches!(
                action,
                SessionTransportAction::RejectTooMany | SessionTransportAction::JoinSession
            )
            && matches!(
                next.last_outcome,
                SessionOutcome::RejectedTooMany | SessionOutcome::Joined
            )
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash::{ModelBackend, ModelCheckConfig};
    use nirvash_check::ModelChecker;

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

    #[test]
    fn explicit_and_symbolic_backends_agree() {
        let spec = SessionTransportSpec::new();
        let lowered = crate::lowered_spec(&spec);
        let explicit_snapshot = ModelChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .full_reachable_graph_snapshot()
        .expect("explicit session_transport snapshot");
        let symbolic_snapshot = match ModelChecker::with_config(
            &lowered,
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
            Err(error) => panic!("symbolic session_transport snapshot: {error:?}"),
        };
        assert_eq!(symbolic_snapshot, explicit_snapshot);

        let explicit_result = ModelChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .check_all()
        .expect("explicit session_transport result");
        let symbolic_result = match ModelChecker::with_config(
            &lowered,
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
            Err(error) => panic!("symbolic session_transport result: {error:?}"),
        };
        assert_eq!(symbolic_result, explicit_result);
    }
}
