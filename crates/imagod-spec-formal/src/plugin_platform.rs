use nirvash_core::{
    BoolExpr, Fairness, Ltl, ModelCase, RelSet, Relation2, StepExpr, TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelAtom, RelationalState, Signature as FormalSignature, fairness, invariant,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program, property, subsystem_spec,
};

use crate::PluginKind;
#[cfg(test)]
use crate::bounds::SPEC_PLUGIN_KINDS;

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

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature, RelationalState)]
#[signature(custom)]
pub struct PluginPlatformState {
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

impl PluginPlatformState {
    pub fn plugin_registered(&self) -> bool {
        self.registered_plugins.some()
    }

    pub fn graph_classified(&self) -> bool {
        self.requires.some() || self.cycle_edges.some() || self.missing_dependencies.some()
    }

    pub fn graph_is_acyclic(&self) -> bool {
        self.plugin_registered()
            && self.requires.some()
            && self.cycle_edges.no()
            && self.missing_dependencies.no()
    }

    pub fn provider_decided(&self) -> bool {
        self.resolved_provider.some() || self.provider_is_missing()
    }

    pub fn provider_is_self(&self) -> bool {
        self.resolved_provider
            .contains(&PluginAtom::Root, &ProviderAtom::SelfProvider)
    }

    pub fn provider_is_dependency(&self) -> bool {
        self.resolved_provider
            .contains(&PluginAtom::Root, &ProviderAtom::DependencyProvider)
    }

    pub fn provider_is_missing(&self) -> bool {
        self.imports
            .contains(&PluginAtom::Root, &InterfaceAtom::CapabilityApi)
            && self.resolved_provider.no()
    }

    pub fn capability_decided(&self) -> bool {
        self.allowed_dep_calls.some()
            || self.allowed_wasi_calls.some()
            || self.privileged_plugins.some()
    }

    pub fn capability_is_privileged(&self) -> bool {
        self.privileged_plugins.contains(&PluginAtom::Root)
    }

    pub fn http_outbound_enabled(&self) -> bool {
        self.http_outbound.some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature, ActionVocabulary)]
pub enum PluginPlatformAction {
    /// Register plugin
    RegisterPlugin(PluginKind),
    /// Mark graph acyclic
    ClassifyGraphAcyclic,
    /// Mark graph cyclic
    ClassifyGraphCyclic,
    /// Mark dependency missing
    ClassifyGraphMissingDependency,
    /// Resolve self provider
    ResolveProviderSelf,
    /// Resolve dependency provider
    ResolveProviderDependency,
    /// Mark provider missing
    ResolveProviderMissing,
    /// Allow capability
    AllowCapability,
    /// Grant privileged capability
    GrantPrivilegedCapability,
    /// Allow HTTP host
    AllowHttpHost,
    /// Deny HTTP outbound
    DenyHttpOutbound,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PluginPlatformSpec;

impl PluginPlatformSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> PluginPlatformState {
        PluginPlatformState {
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

    fn transition_state(
        &self,
        prev: &PluginPlatformState,
        action: &PluginPlatformAction,
    ) -> Option<PluginPlatformState> {
        let mut candidate = prev.clone();
        let allowed = match action {
            PluginPlatformAction::RegisterPlugin(kind) if prev.registered_plugins.no() => {
                candidate.registered_plugins.insert(PluginAtom::Root);
                candidate
                    .plugin_kinds
                    .insert(PluginAtom::Root, plugin_kind_atom(kind.clone()));
                true
            }
            PluginPlatformAction::ClassifyGraphAcyclic
                if prev.plugin_registered() && !prev.graph_classified() =>
            {
                candidate
                    .requires
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                true
            }
            PluginPlatformAction::ClassifyGraphCyclic
                if prev.plugin_registered() && !prev.graph_classified() =>
            {
                candidate
                    .cycle_edges
                    .insert(ProviderAtom::SelfProvider, ProviderAtom::DependencyProvider);
                candidate
                    .cycle_edges
                    .insert(ProviderAtom::DependencyProvider, ProviderAtom::SelfProvider);
                true
            }
            PluginPlatformAction::ClassifyGraphMissingDependency
                if prev.plugin_registered() && !prev.graph_classified() =>
            {
                candidate
                    .missing_dependencies
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                true
            }
            PluginPlatformAction::ResolveProviderSelf
                if prev.graph_is_acyclic() && prev.resolved_provider.no() =>
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
            PluginPlatformAction::ResolveProviderDependency
                if prev.graph_is_acyclic() && prev.resolved_provider.no() =>
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
            PluginPlatformAction::ResolveProviderMissing
                if !prev.graph_is_acyclic() && prev.resolved_provider.no() && prev.imports.no() =>
            {
                candidate
                    .imports
                    .insert(PluginAtom::Root, InterfaceAtom::CapabilityApi);
                true
            }
            PluginPlatformAction::AllowCapability
                if prev.resolved_provider.some() && !prev.capability_decided() =>
            {
                candidate
                    .allowed_dep_calls
                    .insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
                candidate
                    .allowed_wasi_calls
                    .insert(PluginAtom::Root, WasiCapabilityAtom::HttpOutgoing);
                true
            }
            PluginPlatformAction::GrantPrivilegedCapability
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
            PluginPlatformAction::AllowHttpHost
                if prev.resolved_provider.some()
                    && prev.capability_decided()
                    && prev.http_outbound.no() =>
            {
                candidate
                    .http_outbound
                    .insert(PluginAtom::Root, HttpTargetAtom::Host);
                true
            }
            PluginPlatformAction::DenyHttpOutbound if prev.http_outbound.some() => {
                candidate.http_outbound = Relation2::empty();
                true
            }
            _ => false,
        };
        allowed.then_some(candidate)
    }
}

nirvash_core::signature_spec!(
    PluginPlatformStateSignatureSpec for PluginPlatformState,
    representatives = crate::state_domain::reachable_state_domain(&PluginPlatformSpec::new())
);

fn plugin_capability_model_cases() -> Vec<ModelCase<PluginPlatformState, PluginPlatformAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

fn plugin_kind_atom(kind: PluginKind) -> PluginKindAtom {
    match kind {
        PluginKind::Native => PluginKindAtom::Native,
        PluginKind::Wasm => PluginKindAtom::Wasm,
    }
}

#[invariant(PluginPlatformSpec)]
fn resolved_provider_requires_acyclic_graph() -> BoolExpr<PluginPlatformState> {
    nirvash_expr! { resolved_provider_requires_acyclic_graph(state) =>
        state.resolved_provider.no() || state.graph_is_acyclic()
    }
}

#[invariant(PluginPlatformSpec)]
fn http_outbound_requires_provider_resolution() -> BoolExpr<PluginPlatformState> {
    nirvash_expr! { http_outbound_requires_provider_resolution(state) =>
        !state.http_outbound_enabled() || state.resolved_provider.some()
    }
}

#[invariant(PluginPlatformSpec)]
fn privileged_mode_requires_resolved_provider() -> BoolExpr<PluginPlatformState> {
    nirvash_expr! { privileged_mode_requires_resolved_provider(state) =>
        !state.capability_is_privileged() || state.resolved_provider.some()
    }
}

#[property(PluginPlatformSpec)]
fn plugin_registered_leads_to_graph_classified() -> Ltl<PluginPlatformState, PluginPlatformAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { plugin_registered(state) => state.plugin_registered() }),
        Ltl::pred(nirvash_expr! { graph_classified(state) => state.graph_classified() }),
    )
}

#[property(PluginPlatformSpec)]
fn graph_acyclic_leads_to_provider_resolved() -> Ltl<PluginPlatformState, PluginPlatformAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { graph_acyclic(state) => state.graph_is_acyclic() }),
        Ltl::pred(nirvash_expr! { provider_resolved(state) => state.resolved_provider.some() }),
    )
}

#[property(PluginPlatformSpec)]
fn provider_resolved_leads_to_capability_decided() -> Ltl<PluginPlatformState, PluginPlatformAction>
{
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { provider_resolved(state) => state.resolved_provider.some() }),
        Ltl::pred(nirvash_expr! { capability_decided(state) => state.capability_decided() }),
    )
}

#[fairness(PluginPlatformSpec)]
fn graph_classification_fairness() -> Fairness<PluginPlatformState, PluginPlatformAction> {
    Fairness::weak(nirvash_step_expr! { classify_graph(prev, action, next) =>
        prev.plugin_registered()
            && matches!(
                action,
                PluginPlatformAction::ClassifyGraphAcyclic
                    | PluginPlatformAction::ClassifyGraphCyclic
                    | PluginPlatformAction::ClassifyGraphMissingDependency
            )
            && next.graph_classified()
    })
}

#[fairness(PluginPlatformSpec)]
fn provider_resolution_fairness() -> Fairness<PluginPlatformState, PluginPlatformAction> {
    Fairness::weak(
        nirvash_step_expr! { resolve_provider(_prev, action, next) =>
            matches!(
                action,
                PluginPlatformAction::ResolveProviderSelf
                    | PluginPlatformAction::ResolveProviderDependency
                    | PluginPlatformAction::ResolveProviderMissing
            ) && next.provider_decided()
        },
    )
}

#[fairness(PluginPlatformSpec)]
fn capability_decision_fairness() -> Fairness<PluginPlatformState, PluginPlatformAction> {
    Fairness::weak(
        nirvash_step_expr! { decide_capability(prev, action, next) =>
            matches!(
                action,
                PluginPlatformAction::AllowCapability | PluginPlatformAction::GrantPrivilegedCapability
            ) && next.capability_decided()
                && !prev.capability_decided()
        },
    )
}

#[subsystem_spec(model_cases(plugin_capability_model_cases))]
impl TransitionSystem for PluginPlatformSpec {
    type State = PluginPlatformState;
    type Action = PluginPlatformAction;

    fn name(&self) -> &'static str {
        "plugin_platform"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash_core::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule register_plugin when register_plugin_kind(action).is_some()
                && prev.registered_plugins.no() => {
                insert registered_plugins <= PluginAtom::Root;
                set plugin_kinds <= plugin_kinds_after_registration(prev, action);
            }

            rule classify_graph_acyclic when matches!(action, PluginPlatformAction::ClassifyGraphAcyclic)
                && prev.plugin_registered()
                && !prev.graph_classified() => {
                set requires <= requires_after_acyclic_classification(prev);
            }

            rule classify_graph_cyclic when matches!(action, PluginPlatformAction::ClassifyGraphCyclic)
                && prev.plugin_registered()
                && !prev.graph_classified() => {
                set cycle_edges <= cycle_edges_after_cyclic_classification(prev);
            }

            rule classify_graph_missing_dependency when matches!(action, PluginPlatformAction::ClassifyGraphMissingDependency)
                && prev.plugin_registered()
                && !prev.graph_classified() => {
                set missing_dependencies <= missing_dependencies_after_classification(prev);
            }

            rule resolve_provider_self when matches!(action, PluginPlatformAction::ResolveProviderSelf)
                && prev.graph_is_acyclic()
                && prev.resolved_provider.no() => {
                set imports <= capability_api_imports(prev);
                set provides <= self_provider_capabilities(prev);
                set resolved_provider <= self_provider_resolution(prev);
            }

            rule resolve_provider_dependency when matches!(action, PluginPlatformAction::ResolveProviderDependency)
                && prev.graph_is_acyclic()
                && prev.resolved_provider.no() => {
                set imports <= capability_api_imports(prev);
                set provides <= dependency_provider_capabilities(prev);
                set resolved_provider <= dependency_provider_resolution(prev);
            }

            rule resolve_provider_missing when matches!(action, PluginPlatformAction::ResolveProviderMissing)
                && !prev.graph_is_acyclic()
                && prev.resolved_provider.no()
                && prev.imports.no() => {
                set imports <= capability_api_imports(prev);
            }

            rule allow_capability when matches!(action, PluginPlatformAction::AllowCapability)
                && prev.resolved_provider.some()
                && !prev.capability_decided() => {
                set allowed_dep_calls <= allowed_dependency_calls(prev);
                set allowed_wasi_calls <= allowed_wasi_calls(prev);
            }

            rule grant_privileged_capability when matches!(action, PluginPlatformAction::GrantPrivilegedCapability)
                && prev.resolved_provider.some()
                && prev.privileged_plugins.no() => {
                insert privileged_plugins <= PluginAtom::Root;
                set allowed_dep_calls <= allowed_dependency_calls(prev);
                set allowed_wasi_calls <= allowed_wasi_calls(prev);
            }

            rule allow_http_host when matches!(action, PluginPlatformAction::AllowHttpHost)
                && prev.resolved_provider.some()
                && prev.capability_decided()
                && prev.http_outbound.no() => {
                set http_outbound <= http_outbound_host(prev);
            }

            rule deny_http_outbound when matches!(action, PluginPlatformAction::DenyHttpOutbound)
                && prev.http_outbound.some() => {
                set http_outbound <= Relation2::empty();
            }
        })
    }
}

fn register_plugin_kind(action: &PluginPlatformAction) -> Option<PluginKind> {
    match action {
        PluginPlatformAction::RegisterPlugin(kind) => Some(kind.clone()),
        _ => None,
    }
}

fn plugin_kinds_after_registration(
    prev: &PluginPlatformState,
    action: &PluginPlatformAction,
) -> Relation2<PluginAtom, PluginKindAtom> {
    let kind = register_plugin_kind(action)
        .expect("plugin_kinds_after_registration requires RegisterPlugin action");
    let mut kinds = prev.plugin_kinds.clone();
    kinds.insert(PluginAtom::Root, plugin_kind_atom(kind));
    kinds
}

fn requires_after_acyclic_classification(
    prev: &PluginPlatformState,
) -> Relation2<PluginAtom, ProviderAtom> {
    let mut requires = prev.requires.clone();
    requires.insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
    requires
}

fn cycle_edges_after_cyclic_classification(
    prev: &PluginPlatformState,
) -> Relation2<ProviderAtom, ProviderAtom> {
    let mut cycle_edges = prev.cycle_edges.clone();
    cycle_edges.insert(ProviderAtom::SelfProvider, ProviderAtom::DependencyProvider);
    cycle_edges.insert(ProviderAtom::DependencyProvider, ProviderAtom::SelfProvider);
    cycle_edges
}

fn missing_dependencies_after_classification(
    prev: &PluginPlatformState,
) -> Relation2<PluginAtom, ProviderAtom> {
    let mut missing = prev.missing_dependencies.clone();
    missing.insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
    missing
}

fn capability_api_imports(prev: &PluginPlatformState) -> Relation2<PluginAtom, InterfaceAtom> {
    let mut imports = prev.imports.clone();
    imports.insert(PluginAtom::Root, InterfaceAtom::CapabilityApi);
    imports
}

fn self_provider_capabilities(
    prev: &PluginPlatformState,
) -> Relation2<ProviderAtom, InterfaceAtom> {
    let mut provides = prev.provides.clone();
    provides.insert(ProviderAtom::SelfProvider, InterfaceAtom::CapabilityApi);
    provides
}

fn dependency_provider_capabilities(
    prev: &PluginPlatformState,
) -> Relation2<ProviderAtom, InterfaceAtom> {
    let mut provides = prev.provides.clone();
    provides.insert(
        ProviderAtom::DependencyProvider,
        InterfaceAtom::CapabilityApi,
    );
    provides
}

fn self_provider_resolution(prev: &PluginPlatformState) -> Relation2<PluginAtom, ProviderAtom> {
    let mut resolved = prev.resolved_provider.clone();
    resolved.insert(PluginAtom::Root, ProviderAtom::SelfProvider);
    resolved
}

fn dependency_provider_resolution(
    prev: &PluginPlatformState,
) -> Relation2<PluginAtom, ProviderAtom> {
    let mut resolved = prev.resolved_provider.clone();
    resolved.insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
    resolved
}

fn allowed_dependency_calls(prev: &PluginPlatformState) -> Relation2<PluginAtom, ProviderAtom> {
    let mut allowed = prev.allowed_dep_calls.clone();
    allowed.insert(PluginAtom::Root, ProviderAtom::DependencyProvider);
    allowed
}

fn allowed_wasi_calls(prev: &PluginPlatformState) -> Relation2<PluginAtom, WasiCapabilityAtom> {
    let mut allowed = prev.allowed_wasi_calls.clone();
    allowed.insert(PluginAtom::Root, WasiCapabilityAtom::HttpOutgoing);
    allowed
}

fn http_outbound_host(prev: &PluginPlatformState) -> Relation2<PluginAtom, HttpTargetAtom> {
    let mut outbound = prev.http_outbound.clone();
    outbound.insert(PluginAtom::Root, HttpTargetAtom::Host);
    outbound
}

#[nirvash_macros::formal_tests(spec = PluginPlatformSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_core::ModelChecker;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LegacyDependencyGraphClass {
        Empty,
        Acyclic,
        Cyclic,
        MissingDependency,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LegacyProviderResolutionClass {
        Unresolved,
        SelfComponent,
        Dependency,
        Missing,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LegacyCapabilityDecision {
        Denied,
        Allowed,
        Privileged,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LegacyHttpOutboundClass {
        None,
        Host,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LegacyPluginPlatformState {
        plugin_kind: Option<PluginKind>,
        graph: LegacyDependencyGraphClass,
        provider: LegacyProviderResolutionClass,
        capability: LegacyCapabilityDecision,
        http_outbound: LegacyHttpOutboundClass,
    }

    fn legacy_initial_state() -> LegacyPluginPlatformState {
        LegacyPluginPlatformState {
            plugin_kind: None,
            graph: LegacyDependencyGraphClass::Empty,
            provider: LegacyProviderResolutionClass::Unresolved,
            capability: LegacyCapabilityDecision::Denied,
            http_outbound: LegacyHttpOutboundClass::None,
        }
    }

    fn legacy_transition(
        prev: &LegacyPluginPlatformState,
        action: &PluginPlatformAction,
    ) -> Option<LegacyPluginPlatformState> {
        let mut candidate = prev.clone();
        let allowed = match action {
            PluginPlatformAction::RegisterPlugin(kind) if prev.plugin_kind.is_none() => {
                candidate.plugin_kind = Some(kind.clone());
                true
            }
            PluginPlatformAction::ClassifyGraphAcyclic
                if prev.plugin_kind.is_some()
                    && matches!(prev.graph, LegacyDependencyGraphClass::Empty) =>
            {
                candidate.graph = LegacyDependencyGraphClass::Acyclic;
                true
            }
            PluginPlatformAction::ClassifyGraphCyclic
                if prev.plugin_kind.is_some()
                    && matches!(prev.graph, LegacyDependencyGraphClass::Empty) =>
            {
                candidate.graph = LegacyDependencyGraphClass::Cyclic;
                true
            }
            PluginPlatformAction::ClassifyGraphMissingDependency
                if prev.plugin_kind.is_some()
                    && matches!(prev.graph, LegacyDependencyGraphClass::Empty) =>
            {
                candidate.graph = LegacyDependencyGraphClass::MissingDependency;
                true
            }
            PluginPlatformAction::ResolveProviderSelf
                if matches!(prev.graph, LegacyDependencyGraphClass::Acyclic)
                    && matches!(prev.provider, LegacyProviderResolutionClass::Unresolved) =>
            {
                candidate.provider = LegacyProviderResolutionClass::SelfComponent;
                true
            }
            PluginPlatformAction::ResolveProviderDependency
                if matches!(prev.graph, LegacyDependencyGraphClass::Acyclic)
                    && matches!(prev.provider, LegacyProviderResolutionClass::Unresolved) =>
            {
                candidate.provider = LegacyProviderResolutionClass::Dependency;
                true
            }
            PluginPlatformAction::ResolveProviderMissing
                if !matches!(
                    prev.graph,
                    LegacyDependencyGraphClass::Empty | LegacyDependencyGraphClass::Acyclic
                ) && matches!(prev.provider, LegacyProviderResolutionClass::Unresolved) =>
            {
                candidate.provider = LegacyProviderResolutionClass::Missing;
                true
            }
            PluginPlatformAction::AllowCapability
                if matches!(
                    prev.provider,
                    LegacyProviderResolutionClass::SelfComponent
                        | LegacyProviderResolutionClass::Dependency
                ) && !matches!(prev.capability, LegacyCapabilityDecision::Allowed) =>
            {
                candidate.capability = LegacyCapabilityDecision::Allowed;
                true
            }
            PluginPlatformAction::GrantPrivilegedCapability
                if matches!(
                    prev.provider,
                    LegacyProviderResolutionClass::SelfComponent
                        | LegacyProviderResolutionClass::Dependency
                ) && !matches!(prev.capability, LegacyCapabilityDecision::Privileged) =>
            {
                candidate.capability = LegacyCapabilityDecision::Privileged;
                true
            }
            PluginPlatformAction::AllowHttpHost
                if matches!(
                    prev.provider,
                    LegacyProviderResolutionClass::SelfComponent
                        | LegacyProviderResolutionClass::Dependency
                ) && !matches!(prev.capability, LegacyCapabilityDecision::Denied)
                    && matches!(prev.http_outbound, LegacyHttpOutboundClass::None) =>
            {
                candidate.http_outbound = LegacyHttpOutboundClass::Host;
                true
            }
            PluginPlatformAction::DenyHttpOutbound
                if !matches!(prev.http_outbound, LegacyHttpOutboundClass::None) =>
            {
                candidate.http_outbound = LegacyHttpOutboundClass::None;
                true
            }
            _ => false,
        };
        allowed.then_some(candidate)
    }

    fn legacy_reachable_states() -> Vec<LegacyPluginPlatformState> {
        let mut states = vec![legacy_initial_state()];
        let mut cursor = 0;
        let actions = PluginPlatformSpec::new().actions();

        while cursor < states.len() {
            let current = states[cursor].clone();
            for action in &actions {
                if let Some(next) = legacy_transition(&current, action)
                    && !states.contains(&next)
                {
                    states.push(next);
                }
            }
            cursor += 1;
        }

        states
    }

    #[test]
    fn plugin_kind_atom_covers_public_kinds() {
        for kind in SPEC_PLUGIN_KINDS.iter() {
            let _ = plugin_kind_atom(kind.clone());
        }
    }

    #[test]
    fn reachable_graph_contains_dependency_provider_path() {
        let spec = PluginPlatformSpec::new();
        let snapshot = ModelChecker::new(&spec)
            .reachable_graph_snapshot()
            .expect("reachable graph snapshot");

        assert!(
            snapshot
                .states
                .iter()
                .any(|state| { state.provider_is_dependency() && state.graph_is_acyclic() })
        );
    }

    #[test]
    fn relation_model_preserves_legacy_reachability_intents() {
        let legacy_states = legacy_reachable_states();
        let relational_states = ModelChecker::new(&PluginPlatformSpec::new())
            .reachable_graph_snapshot()
            .expect("reachable graph snapshot")
            .states;

        assert!(
            legacy_states.iter().any(|state| matches!(
                state.provider,
                LegacyProviderResolutionClass::SelfComponent
            ))
        );
        assert!(
            relational_states
                .iter()
                .any(PluginPlatformState::provider_is_self)
        );

        assert!(
            legacy_states
                .iter()
                .any(|state| matches!(state.provider, LegacyProviderResolutionClass::Dependency))
        );
        assert!(
            relational_states
                .iter()
                .any(PluginPlatformState::provider_is_dependency)
        );

        assert!(
            legacy_states
                .iter()
                .any(|state| matches!(state.provider, LegacyProviderResolutionClass::Missing))
        );
        assert!(
            relational_states
                .iter()
                .any(PluginPlatformState::provider_is_missing)
        );

        assert!(
            legacy_states
                .iter()
                .any(|state| matches!(state.capability, LegacyCapabilityDecision::Privileged))
        );
        assert!(
            relational_states
                .iter()
                .any(PluginPlatformState::capability_is_privileged)
        );

        assert!(
            legacy_states
                .iter()
                .any(|state| matches!(state.http_outbound, LegacyHttpOutboundClass::Host))
        );
        assert!(
            relational_states
                .iter()
                .any(PluginPlatformState::http_outbound_enabled)
        );
    }
}
