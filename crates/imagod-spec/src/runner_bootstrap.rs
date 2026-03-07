use imago_formal_core::{
    BoundedDomain, Fairness, Ltl, Signature, StatePredicate, StepPredicate, TransitionSystem,
};
use imago_formal_macros::{
    Signature as FormalSignature, imago_fairness, imago_illegal, imago_invariant, imago_property,
    imago_subsystem_spec,
};
use imagod_ipc::{RunnerAppType, RunnerBootstrap};

use crate::bounds::SPEC_RUNNER_APP_TYPES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum BootstrapSizeClass {
    WithinBounds,
    Oversized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum EndpointState {
    Missing,
    Prepared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
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
    RunnerBootstrapContractClass {
        app_type: bootstrap.app_type,
        http_contract_valid: if matches!(bootstrap.app_type, RunnerAppType::Http) {
            bootstrap.http_port.is_some()
        } else {
            bootstrap.http_port.is_none()
        },
        socket_contract_valid: if matches!(bootstrap.app_type, RunnerAppType::Socket) {
            bootstrap.socket.is_some()
        } else {
            bootstrap.socket.is_none()
        },
        secrets_present: !bootstrap.manager_auth_secret.is_empty()
            && !bootstrap.invocation_secret.is_empty(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
pub struct RunnerBootstrapState {
    pub size: BootstrapSizeClass,
    pub decoded: bool,
    pub app_type: Option<RunnerAppType>,
    pub endpoint: EndpointState,
    pub auth: AuthProofState,
    pub registered: bool,
    pub ready: bool,
}

impl RunnerBootstrapStateSignatureSpec for RunnerBootstrapState {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            RunnerBootstrapSpec::new().initial_state(),
            Self {
                size: BootstrapSizeClass::WithinBounds,
                decoded: true,
                app_type: Some(RunnerAppType::Http),
                endpoint: EndpointState::Prepared,
                auth: AuthProofState::Pending,
                registered: false,
                ready: false,
            },
            Self {
                size: BootstrapSizeClass::WithinBounds,
                decoded: true,
                app_type: Some(RunnerAppType::Rpc),
                endpoint: EndpointState::Prepared,
                auth: AuthProofState::Verified,
                registered: true,
                ready: true,
            },
            Self {
                size: BootstrapSizeClass::Oversized,
                decoded: false,
                app_type: None,
                endpoint: EndpointState::Missing,
                auth: AuthProofState::Rejected,
                registered: false,
                ready: false,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        let ready_requires_registration =
            !self.ready || (self.registered && matches!(self.auth, AuthProofState::Verified));
        let registration_requires_endpoint =
            !self.registered || (self.decoded && matches!(self.endpoint, EndpointState::Prepared));
        let rejected_auth_is_not_ready =
            !matches!(self.auth, AuthProofState::Rejected) || !self.ready;

        ready_requires_registration && registration_requires_endpoint && rejected_auth_is_not_ready
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerBootstrapAction {
    ReadWithinBounds,
    ReadOversized,
    DecodeBootstrap(RunnerAppType),
    PrepareEndpoint,
    RegisterRunner,
    RejectAuthProof,
    MarkReady,
}

impl Signature for RunnerBootstrapAction {
    fn bounded_domain() -> BoundedDomain<Self> {
        let mut values = vec![
            Self::ReadWithinBounds,
            Self::ReadOversized,
            Self::PrepareEndpoint,
            Self::RegisterRunner,
            Self::RejectAuthProof,
            Self::MarkReady,
        ];
        values.extend(SPEC_RUNNER_APP_TYPES.into_iter().map(Self::DecodeBootstrap));
        BoundedDomain::new(values)
    }
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
}

#[imago_invariant]
fn ready_requires_verified_registration() -> StatePredicate<RunnerBootstrapState> {
    StatePredicate::new("ready_requires_verified_registration", |state| {
        !state.ready || (state.registered && matches!(state.auth, AuthProofState::Verified))
    })
}

#[imago_invariant]
fn registration_requires_prepared_endpoint() -> StatePredicate<RunnerBootstrapState> {
    StatePredicate::new("registration_requires_prepared_endpoint", |state| {
        !state.registered || (state.decoded && matches!(state.endpoint, EndpointState::Prepared))
    })
}

#[imago_invariant]
fn rejected_auth_cannot_be_ready() -> StatePredicate<RunnerBootstrapState> {
    StatePredicate::new("rejected_auth_cannot_be_ready", |state| {
        !matches!(state.auth, AuthProofState::Rejected) || !state.ready
    })
}

#[imago_illegal]
fn decode_oversized_payload() -> StepPredicate<RunnerBootstrapState, RunnerBootstrapAction> {
    StepPredicate::new("decode_oversized_payload", |prev, action, _| {
        matches!(action, RunnerBootstrapAction::DecodeBootstrap(_))
            && matches!(prev.size, BootstrapSizeClass::Oversized)
    })
}

#[imago_illegal]
fn register_without_endpoint() -> StepPredicate<RunnerBootstrapState, RunnerBootstrapAction> {
    StepPredicate::new("register_without_endpoint", |prev, action, _| {
        matches!(action, RunnerBootstrapAction::RegisterRunner)
            && !matches!(prev.endpoint, EndpointState::Prepared)
    })
}

#[imago_illegal]
fn ready_without_registration() -> StepPredicate<RunnerBootstrapState, RunnerBootstrapAction> {
    StepPredicate::new("ready_without_registration", |prev, action, _| {
        matches!(action, RunnerBootstrapAction::MarkReady) && !prev.registered
    })
}

#[imago_property]
fn decoded_leads_to_endpoint_prepared() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("decoded", |state| state.decoded)),
        Ltl::pred(StatePredicate::new("endpoint_prepared", |state| {
            matches!(state.endpoint, EndpointState::Prepared)
        })),
    )
}

#[imago_property]
fn registered_leads_to_ready() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("registered", |state| state.registered)),
        Ltl::pred(StatePredicate::new("ready", |state| state.ready)),
    )
}

#[imago_property]
fn oversized_implies_next_auth_rejected() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::always(Ltl::implies(
        Ltl::pred(StatePredicate::new("oversized", |state| {
            matches!(state.size, BootstrapSizeClass::Oversized)
        })),
        Ltl::next(Ltl::pred(StatePredicate::new("auth_rejected", |state| {
            matches!(state.auth, AuthProofState::Rejected)
        }))),
    ))
}

#[imago_fairness]
fn endpoint_preparation_fairness() -> Fairness<RunnerBootstrapState, RunnerBootstrapAction> {
    Fairness::weak(StepPredicate::new(
        "prepare_endpoint",
        |prev, action, next| {
            matches!(action, RunnerBootstrapAction::PrepareEndpoint)
                && prev.decoded
                && matches!(next.endpoint, EndpointState::Prepared)
        },
    ))
}

#[imago_fairness]
fn registration_fairness() -> Fairness<RunnerBootstrapState, RunnerBootstrapAction> {
    Fairness::weak(StepPredicate::new(
        "register_runner",
        |prev, action, next| {
            matches!(action, RunnerBootstrapAction::RegisterRunner)
                && prev.decoded
                && matches!(prev.endpoint, EndpointState::Prepared)
                && next.registered
                && matches!(next.auth, AuthProofState::Verified)
        },
    ))
}

#[imago_fairness]
fn ready_fairness() -> Fairness<RunnerBootstrapState, RunnerBootstrapAction> {
    Fairness::weak(StepPredicate::new("mark_ready", |prev, action, next| {
        matches!(action, RunnerBootstrapAction::MarkReady)
            && prev.registered
            && matches!(prev.auth, AuthProofState::Verified)
            && next.ready
    }))
}

#[imago_subsystem_spec(
    invariants(
        ready_requires_verified_registration,
        registration_requires_prepared_endpoint,
        rejected_auth_cannot_be_ready
    ),
    illegal(
        decode_oversized_payload,
        register_without_endpoint,
        ready_without_registration
    ),
    properties(
        decoded_leads_to_endpoint_prepared,
        registered_leads_to_ready,
        oversized_implies_next_auth_rejected
    ),
    fairness(endpoint_preparation_fairness, registration_fairness, ready_fairness)
)]
impl TransitionSystem for RunnerBootstrapSpec {
    type State = RunnerBootstrapState;
    type Action = RunnerBootstrapAction;

    fn name(&self) -> &'static str {
        "runner_bootstrap"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            RunnerBootstrapAction::ReadWithinBounds if !prev.decoded => {
                candidate.size = BootstrapSizeClass::WithinBounds;
            }
            RunnerBootstrapAction::ReadOversized if !prev.decoded => {
                candidate.size = BootstrapSizeClass::Oversized;
                candidate.auth = AuthProofState::Rejected;
            }
            RunnerBootstrapAction::DecodeBootstrap(app_type)
                if matches!(prev.size, BootstrapSizeClass::WithinBounds) && !prev.decoded =>
            {
                candidate.decoded = true;
                candidate.app_type = Some(*app_type);
            }
            RunnerBootstrapAction::PrepareEndpoint if prev.decoded => {
                candidate.endpoint = EndpointState::Prepared;
            }
            RunnerBootstrapAction::RegisterRunner
                if prev.decoded
                    && matches!(prev.endpoint, EndpointState::Prepared)
                    && matches!(prev.auth, AuthProofState::Pending) =>
            {
                candidate.registered = true;
                candidate.auth = AuthProofState::Verified;
            }
            RunnerBootstrapAction::RejectAuthProof
                if prev.decoded
                    && matches!(prev.endpoint, EndpointState::Prepared)
                    && !prev.registered =>
            {
                candidate.auth = AuthProofState::Rejected;
            }
            RunnerBootstrapAction::MarkReady
                if prev.registered && matches!(prev.auth, AuthProofState::Verified) =>
            {
                candidate.ready = true;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[cfg(test)]
#[imago_formal_macros::imago_formal_tests(spec = RunnerBootstrapSpec, init = initial_state)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_ipc::{
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
