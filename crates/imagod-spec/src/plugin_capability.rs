use imagod_ipc::PluginKind;
use nirvash_core::{
    BoundedDomain, Fairness, Ltl, Signature, StatePredicate, StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    Signature as FormalSignature, fairness, illegal, invariant, property, subsystem_spec,
};

use crate::bounds::SPEC_PLUGIN_KINDS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRoleClass {
    NativeHost,
    WasmComponent,
}

pub fn classify_plugin_kind(kind: &PluginKind) -> PluginRoleClass {
    match kind {
        PluginKind::Native => PluginRoleClass::NativeHost,
        PluginKind::Wasm => PluginRoleClass::WasmComponent,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum DependencyGraphClass {
    Empty,
    Acyclic,
    Cyclic,
    MissingDependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum ProviderResolutionClass {
    Unresolved,
    SelfComponent,
    Dependency,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum CapabilityDecision {
    Denied,
    Allowed,
    Privileged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum HttpOutboundClass {
    None,
    Host,
    HostPort,
    Cidr,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
pub struct PluginCapabilityState {
    pub plugin_kind: Option<PluginKind>,
    pub graph: DependencyGraphClass,
    pub provider: ProviderResolutionClass,
    pub capability: CapabilityDecision,
    pub http_outbound: HttpOutboundClass,
}

impl PluginCapabilityStateSignatureSpec for PluginCapabilityState {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            PluginCapabilitySpec::new().initial_state(),
            Self {
                plugin_kind: Some(PluginKind::Native),
                graph: DependencyGraphClass::Acyclic,
                provider: ProviderResolutionClass::SelfComponent,
                capability: CapabilityDecision::Allowed,
                http_outbound: HttpOutboundClass::Host,
            },
            Self {
                plugin_kind: Some(PluginKind::Wasm),
                graph: DependencyGraphClass::Acyclic,
                provider: ProviderResolutionClass::Dependency,
                capability: CapabilityDecision::Privileged,
                http_outbound: HttpOutboundClass::Cidr,
            },
            Self {
                plugin_kind: Some(PluginKind::Wasm),
                graph: DependencyGraphClass::MissingDependency,
                provider: ProviderResolutionClass::Missing,
                capability: CapabilityDecision::Denied,
                http_outbound: HttpOutboundClass::None,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        let resolved_provider_needs_acyclic_graph =
            !matches!(
                self.provider,
                ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
            ) || matches!(self.graph, DependencyGraphClass::Acyclic);
        let outbound_requires_resolution = matches!(self.http_outbound, HttpOutboundClass::None)
            || matches!(
                self.provider,
                ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
            );
        let privileged_is_explicit = !matches!(self.capability, CapabilityDecision::Privileged)
            || !matches!(self.provider, ProviderResolutionClass::Missing);

        resolved_provider_needs_acyclic_graph
            && outbound_requires_resolution
            && privileged_is_explicit
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCapabilityAction {
    RegisterPlugin(PluginKind),
    ClassifyGraphAcyclic,
    ClassifyGraphCyclic,
    ClassifyGraphMissingDependency,
    ResolveProviderSelf,
    ResolveProviderDependency,
    ResolveProviderMissing,
    AllowCapability,
    DenyCapability,
    GrantPrivilegedCapability,
    AllowHttpHost,
    AllowHttpHostPort,
    AllowHttpCidr,
    DenyHttpOutbound,
}

impl Signature for PluginCapabilityAction {
    fn bounded_domain() -> BoundedDomain<Self> {
        let mut values = vec![
            Self::ClassifyGraphAcyclic,
            Self::ClassifyGraphCyclic,
            Self::ClassifyGraphMissingDependency,
            Self::ResolveProviderSelf,
            Self::ResolveProviderDependency,
            Self::ResolveProviderMissing,
            Self::AllowCapability,
            Self::DenyCapability,
            Self::GrantPrivilegedCapability,
            Self::AllowHttpHost,
            Self::AllowHttpHostPort,
            Self::AllowHttpCidr,
            Self::DenyHttpOutbound,
        ];
        values.extend(SPEC_PLUGIN_KINDS.iter().cloned().map(Self::RegisterPlugin));
        BoundedDomain::new(values)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PluginCapabilitySpec;

impl PluginCapabilitySpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> PluginCapabilityState {
        PluginCapabilityState {
            plugin_kind: None,
            graph: DependencyGraphClass::Empty,
            provider: ProviderResolutionClass::Unresolved,
            capability: CapabilityDecision::Denied,
            http_outbound: HttpOutboundClass::None,
        }
    }
}

#[invariant(PluginCapabilitySpec)]
fn resolved_provider_requires_acyclic_graph() -> StatePredicate<PluginCapabilityState> {
    StatePredicate::new("resolved_provider_requires_acyclic_graph", |state| {
        !matches!(
            state.provider,
            ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
        ) || matches!(state.graph, DependencyGraphClass::Acyclic)
    })
}

#[invariant(PluginCapabilitySpec)]
fn http_outbound_requires_provider_resolution() -> StatePredicate<PluginCapabilityState> {
    StatePredicate::new("http_outbound_requires_provider_resolution", |state| {
        matches!(state.http_outbound, HttpOutboundClass::None)
            || matches!(
                state.provider,
                ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
            )
    })
}

#[invariant(PluginCapabilitySpec)]
fn privileged_mode_requires_resolved_provider() -> StatePredicate<PluginCapabilityState> {
    StatePredicate::new("privileged_mode_requires_resolved_provider", |state| {
        !matches!(state.capability, CapabilityDecision::Privileged)
            || !matches!(state.provider, ProviderResolutionClass::Missing)
    })
}

#[illegal(PluginCapabilitySpec)]
fn resolve_dependency_without_acyclic_graph()
-> StepPredicate<PluginCapabilityState, PluginCapabilityAction> {
    StepPredicate::new(
        "resolve_dependency_without_acyclic_graph",
        |prev, action, _| {
            matches!(action, PluginCapabilityAction::ResolveProviderDependency)
                && !matches!(prev.graph, DependencyGraphClass::Acyclic)
        },
    )
}

#[illegal(PluginCapabilitySpec)]
fn grant_privileged_without_provider()
-> StepPredicate<PluginCapabilityState, PluginCapabilityAction> {
    StepPredicate::new("grant_privileged_without_provider", |prev, action, _| {
        matches!(action, PluginCapabilityAction::GrantPrivilegedCapability)
            && matches!(
                prev.provider,
                ProviderResolutionClass::Unresolved | ProviderResolutionClass::Missing
            )
    })
}

#[illegal(PluginCapabilitySpec)]
fn allow_http_without_provider() -> StepPredicate<PluginCapabilityState, PluginCapabilityAction> {
    StepPredicate::new("allow_http_without_provider", |prev, action, _| {
        matches!(
            action,
            PluginCapabilityAction::AllowHttpHost
                | PluginCapabilityAction::AllowHttpHostPort
                | PluginCapabilityAction::AllowHttpCidr
        ) && matches!(
            prev.provider,
            ProviderResolutionClass::Unresolved | ProviderResolutionClass::Missing
        )
    })
}

#[property(PluginCapabilitySpec)]
fn plugin_registered_leads_to_graph_classified()
-> Ltl<PluginCapabilityState, PluginCapabilityAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("plugin_registered", |state| {
            state.plugin_kind.is_some()
        })),
        Ltl::pred(StatePredicate::new("graph_classified", |state| {
            !matches!(state.graph, DependencyGraphClass::Empty)
        })),
    )
}

#[property(PluginCapabilitySpec)]
fn graph_acyclic_leads_to_provider_resolved() -> Ltl<PluginCapabilityState, PluginCapabilityAction>
{
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("graph_acyclic", |state| {
            matches!(state.graph, DependencyGraphClass::Acyclic)
        })),
        Ltl::pred(StatePredicate::new("provider_resolved", |state| {
            matches!(
                state.provider,
                ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
            )
        })),
    )
}

#[property(PluginCapabilitySpec)]
fn provider_resolved_leads_to_capability_decided()
-> Ltl<PluginCapabilityState, PluginCapabilityAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("provider_resolved", |state| {
            matches!(
                state.provider,
                ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
            )
        })),
        Ltl::pred(StatePredicate::new("capability_decided", |state| {
            !matches!(state.capability, CapabilityDecision::Denied)
                || matches!(state.http_outbound, HttpOutboundClass::None)
        })),
    )
}

#[fairness(PluginCapabilitySpec)]
fn graph_classification_fairness() -> Fairness<PluginCapabilityState, PluginCapabilityAction> {
    Fairness::weak(StepPredicate::new(
        "classify_graph",
        |prev, action, next| {
            prev.plugin_kind.is_some()
                && matches!(
                    action,
                    PluginCapabilityAction::ClassifyGraphAcyclic
                        | PluginCapabilityAction::ClassifyGraphCyclic
                        | PluginCapabilityAction::ClassifyGraphMissingDependency
                )
                && !matches!(next.graph, DependencyGraphClass::Empty)
        },
    ))
}

#[fairness(PluginCapabilitySpec)]
fn provider_resolution_fairness() -> Fairness<PluginCapabilityState, PluginCapabilityAction> {
    Fairness::weak(StepPredicate::new("resolve_provider", |_, action, next| {
        matches!(
            action,
            PluginCapabilityAction::ResolveProviderSelf
                | PluginCapabilityAction::ResolveProviderDependency
                | PluginCapabilityAction::ResolveProviderMissing
        ) && !matches!(next.provider, ProviderResolutionClass::Unresolved)
    }))
}

#[fairness(PluginCapabilitySpec)]
fn capability_decision_fairness() -> Fairness<PluginCapabilityState, PluginCapabilityAction> {
    Fairness::weak(StepPredicate::new(
        "decide_capability",
        |prev, action, next| {
            matches!(
                action,
                PluginCapabilityAction::AllowCapability
                    | PluginCapabilityAction::DenyCapability
                    | PluginCapabilityAction::GrantPrivilegedCapability
            ) && prev.provider != next.provider
                || next.capability != prev.capability
        },
    ))
}

#[subsystem_spec]
impl TransitionSystem for PluginCapabilitySpec {
    type State = PluginCapabilityState;
    type Action = PluginCapabilityAction;

    fn name(&self) -> &'static str {
        "plugin_capability"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = prev.clone();
        match action {
            PluginCapabilityAction::RegisterPlugin(kind) if prev.plugin_kind.is_none() => {
                candidate.plugin_kind = Some(kind.clone());
            }
            PluginCapabilityAction::ClassifyGraphAcyclic if prev.plugin_kind.is_some() => {
                candidate.graph = DependencyGraphClass::Acyclic;
            }
            PluginCapabilityAction::ClassifyGraphCyclic if prev.plugin_kind.is_some() => {
                candidate.graph = DependencyGraphClass::Cyclic;
            }
            PluginCapabilityAction::ClassifyGraphMissingDependency
                if prev.plugin_kind.is_some() =>
            {
                candidate.graph = DependencyGraphClass::MissingDependency;
            }
            PluginCapabilityAction::ResolveProviderSelf
                if matches!(prev.graph, DependencyGraphClass::Acyclic) =>
            {
                candidate.provider = ProviderResolutionClass::SelfComponent;
            }
            PluginCapabilityAction::ResolveProviderDependency
                if matches!(prev.graph, DependencyGraphClass::Acyclic) =>
            {
                candidate.provider = ProviderResolutionClass::Dependency;
            }
            PluginCapabilityAction::ResolveProviderMissing
                if !matches!(prev.graph, DependencyGraphClass::Acyclic) =>
            {
                candidate.provider = ProviderResolutionClass::Missing;
            }
            PluginCapabilityAction::AllowCapability
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) =>
            {
                candidate.capability = CapabilityDecision::Allowed;
            }
            PluginCapabilityAction::DenyCapability => {
                candidate.capability = CapabilityDecision::Denied;
            }
            PluginCapabilityAction::GrantPrivilegedCapability
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) =>
            {
                candidate.capability = CapabilityDecision::Privileged;
            }
            PluginCapabilityAction::AllowHttpHost
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) =>
            {
                candidate.http_outbound = HttpOutboundClass::Host;
            }
            PluginCapabilityAction::AllowHttpHostPort
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) =>
            {
                candidate.http_outbound = HttpOutboundClass::HostPort;
            }
            PluginCapabilityAction::AllowHttpCidr
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) =>
            {
                candidate.http_outbound = HttpOutboundClass::Cidr;
            }
            PluginCapabilityAction::DenyHttpOutbound => {
                candidate.http_outbound = HttpOutboundClass::None;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[cfg(test)]
#[nirvash_macros::formal_tests(spec = PluginCapabilitySpec, init = initial_state)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_kind_classifier_covers_public_kinds() {
        for kind in SPEC_PLUGIN_KINDS.iter() {
            let _ = classify_plugin_kind(kind);
        }
    }
}
