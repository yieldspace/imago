use imagod_ipc::{RunnerAppType, RunnerBootstrap};
use nirvash_core::{Fairness, Ltl, StatePredicate, StepPredicate, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, fairness, invariant, property, subsystem_spec};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerBootstrapState {
    pub size: BootstrapSizeClass,
    pub decoded: bool,
    pub app_type: Option<RunnerAppType>,
    pub endpoint: EndpointState,
    pub auth: AuthProofState,
    pub registered: bool,
    pub ready: bool,
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

    fn action_vocabulary(&self) -> Vec<RunnerBootstrapAction> {
        vec![
            RunnerBootstrapAction::ReadWithinBounds,
            RunnerBootstrapAction::ReadOversized,
            RunnerBootstrapAction::DecodeBootstrap(RunnerAppType::Cli),
            RunnerBootstrapAction::DecodeBootstrap(RunnerAppType::Rpc),
            RunnerBootstrapAction::DecodeBootstrap(RunnerAppType::Http),
            RunnerBootstrapAction::DecodeBootstrap(RunnerAppType::Socket),
            RunnerBootstrapAction::PrepareEndpoint,
            RunnerBootstrapAction::RegisterRunner,
            RunnerBootstrapAction::RejectAuthProof,
            RunnerBootstrapAction::MarkReady,
        ]
    }

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
fn ready_requires_verified_registration() -> StatePredicate<RunnerBootstrapState> {
    StatePredicate::new("ready_requires_verified_registration", |state| {
        !state.ready || (state.registered && matches!(state.auth, AuthProofState::Verified))
    })
}

#[invariant(RunnerBootstrapSpec)]
fn registration_requires_prepared_endpoint() -> StatePredicate<RunnerBootstrapState> {
    StatePredicate::new("registration_requires_prepared_endpoint", |state| {
        !state.registered || (state.decoded && matches!(state.endpoint, EndpointState::Prepared))
    })
}

#[invariant(RunnerBootstrapSpec)]
fn rejected_auth_cannot_be_ready() -> StatePredicate<RunnerBootstrapState> {
    StatePredicate::new("rejected_auth_cannot_be_ready", |state| {
        !matches!(state.auth, AuthProofState::Rejected) || !state.ready
    })
}

#[property(RunnerBootstrapSpec)]
fn decoded_leads_to_endpoint_prepared() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("decoded", |state| state.decoded)),
        Ltl::pred(StatePredicate::new("endpoint_prepared", |state| {
            matches!(state.endpoint, EndpointState::Prepared)
        })),
    )
}

#[property(RunnerBootstrapSpec)]
fn registered_leads_to_ready() -> Ltl<RunnerBootstrapState, RunnerBootstrapAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("registered", |state| state.registered)),
        Ltl::pred(StatePredicate::new("ready", |state| state.ready)),
    )
}

#[property(RunnerBootstrapSpec)]
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

#[fairness(RunnerBootstrapSpec)]
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

#[fairness(RunnerBootstrapSpec)]
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

#[fairness(RunnerBootstrapSpec)]
fn ready_fairness() -> Fairness<RunnerBootstrapState, RunnerBootstrapAction> {
    Fairness::weak(StepPredicate::new("mark_ready", |prev, action, next| {
        matches!(action, RunnerBootstrapAction::MarkReady)
            && prev.registered
            && matches!(prev.auth, AuthProofState::Verified)
            && next.ready
    }))
}

#[subsystem_spec]
impl TransitionSystem for RunnerBootstrapSpec {
    type State = RunnerBootstrapState;
    type Action = RunnerBootstrapAction;

    fn name(&self) -> &'static str {
        "runner_bootstrap"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        self.action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.transition_state(state, action)
    }
}

#[nirvash_macros::formal_tests(spec = RunnerBootstrapSpec)]
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
