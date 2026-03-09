use imagod_ipc::PluginKind;
use nirvash_core::{
    Fairness, Ltl, ModelCase, RelSet, Relation2, StatePredicate, StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    RelAtom, RelationalState, Signature as FormalSignature, fairness, invariant, property,
    subsystem_spec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum PluginAtom {
    Root,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum PluginKindAtom {
    Native,
    Wasm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum ProviderAtom {
    SelfProvider,
    DependencyProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum InterfaceAtom {
    CapabilityApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum WasiCapabilityAtom {
    HttpOutgoing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum HttpTargetAtom {
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
pub struct PluginCapabilityRelationalState {
    registered_plugins: RelSet<PluginAtom>,
    plugin_kinds: Relation2<PluginAtom, PluginKindAtom>,
    requires: Relation2<PluginAtom, ProviderAtom>,
    provides: Relation2<ProviderAtom, InterfaceAtom>,
    imports: Relation2<PluginAtom, InterfaceAtom>,
    resolved_provider: Relation2<PluginAtom, ProviderAtom>,
    missing_dependencies: Relation2<PluginAtom, ProviderAtom>,
    cycle_edges: Relation2<ProviderAtom, ProviderAtom>,
    allowed_dep_calls: Relation2<PluginAtom, ProviderAtom>,
    allowed_wasi_calls: Relation2<PluginAtom, WasiCapabilityAtom>,
    privileged_plugins: RelSet<PluginAtom>,
    http_outbound: Relation2<PluginAtom, HttpTargetAtom>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCapabilityRelationalAction {
    RegisterPlugin(PluginKind),
    ClassifyGraphAcyclic,
    ClassifyGraphCyclic,
    ClassifyGraphMissingDependency,
    ResolveProviderSelf,
    ResolveProviderDependency,
    ResolveProviderMissing,
    AllowCapability,
    GrantPrivilegedCapability,
    AllowHttpHost,
    DenyHttpOutbound,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PluginCapabilityRelationalSpec;

impl PluginCapabilityRelationalSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> PluginCapabilityRelationalState {
        PluginCapabilityRelationalState {
            registered_plugins: RelSet::empty(),
            plugin_kinds: Relation2::empty(),
            requires: Relation2::empty(),
            provides: Relation2::empty(),
            imports: Relation2::empty(),
            resolved_provider: Relation2::empty(),
            missing_dependencies: Relation2::empty(),
            cycle_edges: Relation2::empty(),
            allowed_dep_calls: Relation2::empty(),
            allowed_wasi_calls: Relation2::empty(),
            privileged_plugins: RelSet::empty(),
            http_outbound: Relation2::empty(),
        }
    }

    fn action_vocabulary(&self) -> Vec<PluginCapabilityRelationalAction> {
        vec![
            PluginCapabilityRelationalAction::RegisterPlugin(PluginKind::Native),
            PluginCapabilityRelationalAction::RegisterPlugin(PluginKind::Wasm),
            PluginCapabilityRelationalAction::ClassifyGraphAcyclic,
            PluginCapabilityRelationalAction::ClassifyGraphCyclic,
            PluginCapabilityRelationalAction::ClassifyGraphMissingDependency,
            PluginCapabilityRelationalAction::ResolveProviderSelf,
            PluginCapabilityRelationalAction::ResolveProviderDependency,
            PluginCapabilityRelationalAction::ResolveProviderMissing,
            PluginCapabilityRelationalAction::AllowCapability,
            PluginCapabilityRelationalAction::GrantPrivilegedCapability,
            PluginCapabilityRelationalAction::AllowHttpHost,
            PluginCapabilityRelationalAction::DenyHttpOutbound,
        ]
    }

    fn transition_state(
        &self,
        prev: &PluginCapabilityRelationalState,
        action: &PluginCapabilityRelationalAction,
    ) -> Option<PluginCapabilityRelationalState> {
        let mut candidate = prev.clone();
        let allowed = match action {
            PluginCapabilityRelationalAction::RegisterPlugin(kind)
                if prev.registered_plugins.no() =>
            {
                candidate.registered_plugins.insert(PluginAtom::Root);
                candidate
                    .plugin_kinds
                    .insert(PluginAtom::Root, plugin_kind_atom(kind.clone()));
                true
            }
            PluginCapabilityRelationalAction::ClassifyGraphAcyclic
                if prev.registered_plugins.some() && !graph_classified(prev) =>
            {
                candidate
                    .requires
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                true
            }
            PluginCapabilityRelationalAction::ClassifyGraphCyclic
                if prev.registered_plugins.some() && !graph_classified(prev) =>
            {
                candidate
                    .cycle_edges
                    .insert(ProviderAtom::SelfProvider, ProviderAtom::DependencyProvider);
                candidate
                    .cycle_edges
                    .insert(ProviderAtom::DependencyProvider, ProviderAtom::SelfProvider);
                true
            }
            PluginCapabilityRelationalAction::ClassifyGraphMissingDependency
                if prev.registered_plugins.some() && !graph_classified(prev) =>
            {
                candidate
                    .missing_dependencies
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                true
            }
            PluginCapabilityRelationalAction::ResolveProviderSelf
                if graph_is_acyclic(prev) && prev.resolved_provider.no() =>
            {
                candidate
                    .imports
                    .insert(PluginAtom::Root, InterfaceAtom::CapabilityApi);
                candidate
                    .provides
                    .insert(ProviderAtom::SelfProvider, InterfaceAtom::CapabilityApi);
                candidate
                    .resolved_provider
                    .insert(PluginAtom::Root, ProviderAtom::SelfProvider);
                true
            }
            PluginCapabilityRelationalAction::ResolveProviderDependency
                if graph_is_acyclic(prev) && prev.resolved_provider.no() =>
            {
                candidate
                    .imports
                    .insert(PluginAtom::Root, InterfaceAtom::CapabilityApi);
                candidate.provides.insert(
                    ProviderAtom::DependencyProvider,
                    InterfaceAtom::CapabilityApi,
                );
                candidate
                    .resolved_provider
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                true
            }
            PluginCapabilityRelationalAction::ResolveProviderMissing
                if !graph_is_acyclic(prev) && prev.resolved_provider.no() =>
            {
                candidate
                    .imports
                    .insert(PluginAtom::Root, InterfaceAtom::CapabilityApi);
                true
            }
            PluginCapabilityRelationalAction::AllowCapability
                if prev.resolved_provider.some() && !capability_decided(prev) =>
            {
                candidate
                    .allowed_dep_calls
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                candidate
                    .allowed_wasi_calls
                    .insert(PluginAtom::Root, WasiCapabilityAtom::HttpOutgoing);
                true
            }
            PluginCapabilityRelationalAction::GrantPrivilegedCapability
                if prev.resolved_provider.some() && prev.privileged_plugins.no() =>
            {
                candidate.privileged_plugins.insert(PluginAtom::Root);
                candidate
                    .allowed_dep_calls
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                candidate
                    .allowed_wasi_calls
                    .insert(PluginAtom::Root, WasiCapabilityAtom::HttpOutgoing);
                true
            }
            PluginCapabilityRelationalAction::AllowHttpHost
                if prev.resolved_provider.some()
                    && capability_decided(prev)
                    && prev.http_outbound.no() =>
            {
                candidate
                    .http_outbound
                    .insert(PluginAtom::Root, HttpTargetAtom::Host);
                true
            }
            PluginCapabilityRelationalAction::DenyHttpOutbound if prev.http_outbound.some() => {
                candidate.http_outbound = Relation2::empty();
                true
            }
            _ => false,
        };
        allowed.then_some(candidate)
    }
}

fn plugin_capability_relational_model_cases()
-> Vec<ModelCase<PluginCapabilityRelationalState, PluginCapabilityRelationalAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

fn plugin_kind_atom(kind: PluginKind) -> PluginKindAtom {
    match kind {
        PluginKind::Native => PluginKindAtom::Native,
        PluginKind::Wasm => PluginKindAtom::Wasm,
    }
}

fn graph_classified(state: &PluginCapabilityRelationalState) -> bool {
    state.requires.some() || state.cycle_edges.some() || state.missing_dependencies.some()
}

fn graph_is_acyclic(state: &PluginCapabilityRelationalState) -> bool {
    state.registered_plugins.some()
        && state.requires.some()
        && state.cycle_edges.no()
        && state.missing_dependencies.no()
}

fn capability_decided(state: &PluginCapabilityRelationalState) -> bool {
    state.allowed_dep_calls.some()
        || state.allowed_wasi_calls.some()
        || state.privileged_plugins.some()
}

#[invariant(PluginCapabilityRelationalSpec)]
fn resolved_provider_requires_acyclic_graph() -> StatePredicate<PluginCapabilityRelationalState> {
    StatePredicate::new("resolved_provider_requires_acyclic_graph", |state| {
        state.resolved_provider.no() || graph_is_acyclic(state)
    })
}

#[invariant(PluginCapabilityRelationalSpec)]
fn http_outbound_requires_provider_resolution() -> StatePredicate<PluginCapabilityRelationalState> {
    StatePredicate::new("http_outbound_requires_provider_resolution", |state| {
        state.http_outbound.no() || state.resolved_provider.some()
    })
}

#[invariant(PluginCapabilityRelationalSpec)]
fn privileged_mode_requires_resolved_provider() -> StatePredicate<PluginCapabilityRelationalState> {
    StatePredicate::new("privileged_mode_requires_resolved_provider", |state| {
        state.privileged_plugins.no() || state.resolved_provider.some()
    })
}

#[property(PluginCapabilityRelationalSpec)]
fn plugin_registered_leads_to_graph_classified()
-> Ltl<PluginCapabilityRelationalState, PluginCapabilityRelationalAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("plugin_registered", |state| {
            state.registered_plugins.some()
        })),
        Ltl::pred(StatePredicate::new("graph_classified", graph_classified)),
    )
}

#[property(PluginCapabilityRelationalSpec)]
fn graph_acyclic_leads_to_provider_resolved()
-> Ltl<PluginCapabilityRelationalState, PluginCapabilityRelationalAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("graph_acyclic", graph_is_acyclic)),
        Ltl::pred(StatePredicate::new("provider_resolved", |state| {
            state.resolved_provider.some()
        })),
    )
}

#[property(PluginCapabilityRelationalSpec)]
fn provider_resolved_leads_to_capability_decided()
-> Ltl<PluginCapabilityRelationalState, PluginCapabilityRelationalAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("provider_resolved", |state| {
            state.resolved_provider.some()
        })),
        Ltl::pred(StatePredicate::new(
            "capability_decided",
            capability_decided,
        )),
    )
}

#[fairness(PluginCapabilityRelationalSpec)]
fn graph_classification_fairness()
-> Fairness<PluginCapabilityRelationalState, PluginCapabilityRelationalAction> {
    Fairness::weak(StepPredicate::new(
        "classify_graph",
        |prev, action, next| {
            prev.registered_plugins.some()
                && matches!(
                    action,
                    PluginCapabilityRelationalAction::ClassifyGraphAcyclic
                        | PluginCapabilityRelationalAction::ClassifyGraphCyclic
                        | PluginCapabilityRelationalAction::ClassifyGraphMissingDependency
                )
                && graph_classified(next)
        },
    ))
}

#[fairness(PluginCapabilityRelationalSpec)]
fn provider_resolution_fairness()
-> Fairness<PluginCapabilityRelationalState, PluginCapabilityRelationalAction> {
    Fairness::weak(StepPredicate::new("resolve_provider", |_, action, next| {
        matches!(
            action,
            PluginCapabilityRelationalAction::ResolveProviderSelf
                | PluginCapabilityRelationalAction::ResolveProviderDependency
                | PluginCapabilityRelationalAction::ResolveProviderMissing
        ) && (next.resolved_provider.some() || next.imports.some())
    }))
}

#[fairness(PluginCapabilityRelationalSpec)]
fn capability_decision_fairness()
-> Fairness<PluginCapabilityRelationalState, PluginCapabilityRelationalAction> {
    Fairness::weak(StepPredicate::new(
        "decide_capability",
        |prev, action, next| {
            matches!(
                action,
                PluginCapabilityRelationalAction::AllowCapability
                    | PluginCapabilityRelationalAction::GrantPrivilegedCapability
            ) && capability_decided(next)
                && !capability_decided(prev)
        },
    ))
}

#[subsystem_spec(model_cases(plugin_capability_relational_model_cases))]
impl TransitionSystem for PluginCapabilityRelationalSpec {
    type State = PluginCapabilityRelationalState;
    type Action = PluginCapabilityRelationalAction;

    fn name(&self) -> &'static str {
        "plugin_capability_relational"
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

#[nirvash_macros::formal_tests(spec = PluginCapabilityRelationalSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_capability::PluginCapabilityState;
    use crate::plugin_capability::{PluginCapabilitySpec, ProviderResolutionClass};
    use nirvash_core::ModelChecker;

    fn has_dependency_provider_path(state: &PluginCapabilityState) -> bool {
        matches!(state.provider, ProviderResolutionClass::Dependency)
    }

    fn has_relational_dependency_provider_path(state: &PluginCapabilityRelationalState) -> bool {
        state
            .resolved_provider
            .contains(&PluginAtom::Root, &ProviderAtom::DependencyProvider)
    }

    #[test]
    fn reachable_graph_contains_dependency_provider_path() {
        let spec = PluginCapabilityRelationalSpec::new();
        let snapshot = ModelChecker::new(&spec)
            .reachable_graph_snapshot()
            .expect("reachable graph snapshot");

        assert!(
            snapshot
                .states
                .iter()
                .any(has_relational_dependency_provider_path)
        );
    }

    #[test]
    fn relational_model_preserves_key_reachability_intents() {
        let classic_snapshot = ModelChecker::new(&PluginCapabilitySpec::new())
            .reachable_graph_snapshot()
            .expect("classic snapshot");
        let relational_snapshot = ModelChecker::new(&PluginCapabilityRelationalSpec::new())
            .reachable_graph_snapshot()
            .expect("relational snapshot");

        assert!(
            classic_snapshot
                .states
                .iter()
                .any(has_dependency_provider_path)
        );
        assert!(
            relational_snapshot
                .states
                .iter()
                .any(has_relational_dependency_provider_path)
        );
        assert!(
            relational_snapshot
                .states
                .iter()
                .any(|state| state.http_outbound.some())
        );
        assert!(
            relational_snapshot
                .states
                .iter()
                .any(|state| state.privileged_plugins.some())
        );
    }
}
