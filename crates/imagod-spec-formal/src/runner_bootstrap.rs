use imagod_spec::RunnerBootstrap;
use nirvash::{BoolExpr, Fairness, Ltl, ModelBackend, ModelCheckConfig};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    SymbolicEncoding as FormalSymbolicEncoding, fairness, invariant, nirvash_expr,
    nirvash_step_expr, nirvash_transition_program, property, subsystem_spec,
};

use crate::RunnerAppType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum BootstrapSizeClass {
    WithinBounds,
    Oversized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum EndpointState {
    Missing,
    Prepared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum AuthProofState {
    Pending,
    Verified,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerBootstrapContractClass {
    pub app_type: RunnerAppType,
    pub http_contract_valid: bool,
    pub socket_contract_valid: bool,
    pub secrets_present: bool,
}

pub fn classify_bootstrap(bootstrap: &RunnerBootstrap) -> RunnerBootstrapContractClass {
    let app_type = bootstrap.app_type;
    RunnerBootstrapContractClass {
        app_type,
        http_contract_valid: if matches!(app_type, RunnerAppType::Http) {
            bootstrap.http_port.is_some()
        } else {
            bootstrap.http_port.is_none()
        },
        socket_contract_valid: if matches!(app_type, RunnerAppType::Socket) {
            bootstrap.socket.is_some()
        } else {
            bootstrap.socket.is_none()
        },
        secrets_present: !bootstrap.manager_auth_secret.is_empty()
            && !bootstrap.invocation_secret.is_empty(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub struct RunnerBootstrapState {
    pub size: BootstrapSizeClass,
    pub decoded: bool,
    pub app_type: Option<RunnerAppType>,
    pub endpoint: EndpointState,
    pub auth: AuthProofState,
    pub registered: bool,
    pub ready: bool,
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
pub enum RunnerBootstrapAction {
    /// Read bootstrap
    ReadWithinBounds,
    /// Read oversized bootstrap
    ReadOversized,
    /// Decode bootstrap
    DecodeBootstrap(RunnerAppType),
    /// Prepare endpoint
    PrepareEndpoint,
    /// Register runner
    RegisterRunner,
    /// Reject auth proof
    RejectAuthProof,
    /// Mark runner ready
    MarkReady,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RunnerBootstrapSpec;

impl RunnerBootstrapSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> RunnerBootstrapState {
        RunnerBootstrapState {
            size: BootstrapSizeClass::WithinBounds,
            decoded: false,
            app_type: None,
            endpoint: EndpointState::Missing,
            auth: AuthProofState::Pending,
            registered: false,
            ready: false,
        }
    }

    #[allow(dead_code)]
    fn transition_state(
        &self,
        prev: &RunnerBootstrapState,
        action: &RunnerBootstrapAction,
    ) -> Option<RunnerBootstrapState> {
        let mut candidate = *prev;
        let allowed = match action {
            RunnerBootstrapAction::ReadWithinBounds if !prev.decoded => {
                candidate.size = BootstrapSizeClass::WithinBounds;
                true
            }
            RunnerBootstrapAction::ReadOversized if !prev.decoded => {
                candidate.size = BootstrapSizeClass::Oversized;
                candidate.auth = AuthProofState::Rejected;
                true
            }
            RunnerBootstrapAction::DecodeBootstrap(app_type)
                if matches!(prev.size, BootstrapSizeClass::WithinBounds) && !prev.decoded =>
            {
                candidate.decoded = true;
                candidate.app_type = Some(*app_type);
                true
            }
            RunnerBootstrapAction::PrepareEndpoint if prev.decoded => {
                candidate.endpoint = EndpointState::Prepared;
                true
            }
            RunnerBootstrapAction::RegisterRunner
                if prev.decoded
                    && matches!(prev.endpoint, EndpointState::Prepared)
                    && matches!(prev.auth, AuthProofState::Pending) =>
            {
                candidate.registered = true;
                candidate.auth = AuthProofState::Verified;
                true
            }
            RunnerBootstrapAction::RejectAuthProof
                if prev.decoded
                    && matches!(prev.endpoint, EndpointState::Prepared)
                    && !prev.registered =>
            {
                candidate.auth = AuthProofState::Rejected;
                true
            }
            RunnerBootstrapAction::MarkReady
                if prev.registered && matches!(prev.auth, AuthProofState::Verified) =>
            {
                candidate.ready = true;
                true
            }
            _ => false,
        };
        (allowed && runner_bootstrap_state_valid(&candidate)).then_some(candidate)
    }
}

fn runner_bootstrap_model_cases() -> Vec<ModelInstance<RunnerBootstrapState, RunnerBootstrapAction>>
{
    vec![
        ModelInstance::default().with_checker_config(ModelCheckConfig {
            backend: Some(ModelBackend::Explicit),
            ..ModelCheckConfig::reachable_graph()
        }),
    ]
}

#[allow(dead_code)]
fn runner_bootstrap_state_valid(state: &RunnerBootstrapState) -> bool {
    let ready_requires_registration =
        !state.ready || (state.registered && matches!(state.auth, AuthProofState::Verified));
    let registration_requires_endpoint =
        !state.registered || (state.decoded && matches!(state.endpoint, EndpointState::Prepared));
    let rejected_auth_is_not_ready =
        !matches!(state.auth, AuthProofState::Rejected) || !state.ready;

    ready_requires_registration && registration_requires_endpoint && rejected_auth_is_not_ready
}

#[invariant(RunnerBootstrapSpec)]
fn ready_requires_verified_registration() -> BoolExpr<RunnerBootstrapState> {
    nirvash_expr! { ready_requires_verified_registration(state) =>
        !state.ready || (state.registered && matches!(state.auth, AuthProofState::Verified))
    }
}

#[invariant(RunnerBootstrapSpec)]
fn registration_requires_prepared_endpoint() -> BoolExpr<RunnerBootstrapState> {
    nirvash_expr! { registration_requires_prepared_endpoint(state) =>
        !state.registered || (state.decoded && matches!(state.endpoint, EndpointState::Prepared))
    }
}

#[invariant(RunnerBootstrapSpec)]
fn rejected_auth_cannot_be_ready() -> BoolExpr<RunnerBootstrapState> {
    nirvash_expr! { rejected_auth_cannot_be_ready(state) =>
        !matches!(state.auth, AuthProofState::Rejected) || !state.ready
    }
}

#[property(RunnerBootstrapSpec)]
fn decoded_leads_to_endpoint_prepared() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { decoded(state) => state.decoded }),
        Ltl::pred(nirvash_expr! { endpoint_prepared(state) =>
            matches!(state.endpoint, EndpointState::Prepared)
        }),
    )
}

#[property(RunnerBootstrapSpec)]
fn registered_leads_to_ready() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { registered(state) => state.registered }),
        Ltl::pred(nirvash_expr! { ready(state) => state.ready }),
    )
}

#[property(RunnerBootstrapSpec)]
fn oversized_implies_next_auth_rejected() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::always(Ltl::implies(
        Ltl::pred(nirvash_expr! { oversized(state) =>
            matches!(state.size, BootstrapSizeClass::Oversized)
        }),
        Ltl::next(Ltl::pred(nirvash_expr! { auth_rejected(state) =>
            matches!(state.auth, AuthProofState::Rejected)
        })),
    ))
}

#[fairness(RunnerBootstrapSpec)]
fn endpoint_preparation_fairness() -> Fairness<RunnerBootstrapState, RunnerBootstrapAction> {
    Fairness::weak(nirvash_step_expr! { prepare_endpoint(prev, action, next) =>
        matches!(action, RunnerBootstrapAction::PrepareEndpoint)
            && prev.decoded
            && matches!(next.endpoint, EndpointState::Prepared)
    })
}

#[fairness(RunnerBootstrapSpec)]
fn registration_fairness() -> Fairness<RunnerBootstrapState, RunnerBootstrapAction> {
    Fairness::weak(nirvash_step_expr! { register_runner(prev, action, next) =>
        matches!(action, RunnerBootstrapAction::RegisterRunner)
            && prev.decoded
            && matches!(prev.endpoint, EndpointState::Prepared)
            && next.registered
            && matches!(next.auth, AuthProofState::Verified)
    })
}

#[fairness(RunnerBootstrapSpec)]
fn ready_fairness() -> Fairness<RunnerBootstrapState, RunnerBootstrapAction> {
    Fairness::weak(nirvash_step_expr! { mark_ready(prev, action, next) =>
        matches!(action, RunnerBootstrapAction::MarkReady)
            && prev.registered
            && matches!(prev.auth, AuthProofState::Verified)
            && next.ready
    })
}

#[subsystem_spec(model_cases(runner_bootstrap_model_cases))]
impl FrontendSpec for RunnerBootstrapSpec {
    type State = RunnerBootstrapState;
    type Action = RunnerBootstrapAction;

    fn frontend_name(&self) -> &'static str {
        "runner_bootstrap"
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
            rule read_within_bounds when matches!(action, RunnerBootstrapAction::ReadWithinBounds)
                && !prev.decoded => {
                set size <= BootstrapSizeClass::WithinBounds;
            }

            rule read_oversized when matches!(action, RunnerBootstrapAction::ReadOversized)
                && !prev.decoded => {
                set size <= BootstrapSizeClass::Oversized;
                set auth <= AuthProofState::Rejected;
            }

            rule decode_bootstrap when decode_bootstrap_app_type(action).is_some()
                && matches!(prev.size, BootstrapSizeClass::WithinBounds)
                && !prev.decoded => {
                set decoded <= true;
                set app_type <= decode_bootstrap_app_type(action);
            }

            rule prepare_endpoint when matches!(action, RunnerBootstrapAction::PrepareEndpoint)
                && prev.decoded => {
                set endpoint <= EndpointState::Prepared;
            }

            rule register_runner when matches!(action, RunnerBootstrapAction::RegisterRunner)
                && prev.decoded
                && matches!(prev.endpoint, EndpointState::Prepared)
                && matches!(prev.auth, AuthProofState::Pending) => {
                set registered <= true;
                set auth <= AuthProofState::Verified;
            }

            rule reject_auth_proof when matches!(action, RunnerBootstrapAction::RejectAuthProof)
                && prev.decoded
                && matches!(prev.endpoint, EndpointState::Prepared)
                && !prev.registered => {
                set auth <= AuthProofState::Rejected;
            }

            rule mark_ready when matches!(action, RunnerBootstrapAction::MarkReady)
                && prev.registered
                && matches!(prev.auth, AuthProofState::Verified) => {
                set ready <= true;
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = RunnerBootstrapSpec)]
const _: () = ();

fn decode_bootstrap_app_type(action: &RunnerBootstrapAction) -> Option<RunnerAppType> {
    match action {
        RunnerBootstrapAction::DecodeBootstrap(app_type) => Some(*app_type),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_spec::{
        CapabilityPolicy, RunnerBootstrap, RunnerSocketConfig, RunnerSocketDirection,
        RunnerSocketProtocol,
    };

    #[test]
    fn bootstrap_classifier_reuses_public_contract() {
        let bootstrap = RunnerBootstrap {
            runner_id: "runner-1".to_string(),
            service_name: "svc".to_string(),
            release_hash: "release".to_string(),
            app_type: RunnerAppType::Socket,
            http_port: None,
            http_max_body_bytes: None,
            http_worker_count: 1,
            http_worker_queue_capacity: 1,
            socket: Some(RunnerSocketConfig {
                protocol: RunnerSocketProtocol::Tcp,
                direction: RunnerSocketDirection::Inbound,
                listen_addr: "127.0.0.1".to_string(),
                listen_port: 8080,
            }),
            component_path: std::path::PathBuf::from("/tmp/component.wasm"),
            args: vec![],
            envs: std::collections::BTreeMap::new(),
            wasi_mounts: vec![],
            wasi_http_outbound: vec![],
            resources: std::collections::BTreeMap::new(),
            bindings: vec![],
            plugin_dependencies: vec![],
            capabilities: CapabilityPolicy::default(),
            manager_control_endpoint: std::path::PathBuf::from("/tmp/manager.sock"),
            runner_endpoint: std::path::PathBuf::from("/tmp/runner.sock"),
            manager_auth_secret: "secret".to_string(),
            invocation_secret: "invoke".to_string(),
            epoch_tick_interval_ms: 1,
            wasm_memory_reservation_bytes: 1,
            wasm_memory_reservation_for_growth_bytes: 1,
            wasm_memory_guard_size_bytes: 1,
            wasm_guard_before_linear_memory: true,
            wasm_parallel_compilation: false,
        };

        let class = classify_bootstrap(&bootstrap);
        assert_eq!(class.app_type, RunnerAppType::Socket);
        assert!(class.http_contract_valid);
        assert!(class.socket_contract_valid);
        assert!(class.secrets_present);
    }
}
