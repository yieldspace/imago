use imagod_ipc::PluginKind;
use nirvash_core::{Fairness, Ltl, ModelCase, StatePredicate, StepPredicate, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, fairness, invariant, property, subsystem_spec};

#[cfg(test)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCapabilityState {
    pub plugin_kind: Option<PluginKind>,
    pub graph: DependencyGraphClass,
    pub provider: ProviderResolutionClass,
    pub capability: CapabilityDecision,
    pub http_outbound: HttpOutboundClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCapabilityAction {
    RegisterPlugin(PluginKind),
    ClassifyGraphAcyclic,
    ClassifyGraphCyclic,
    ClassifyGraphMissingDependency,
    ResolveProviderSelf,
    ResolveProviderMissing,
    AllowCapability,
    GrantPrivilegedCapability,
    AllowHttpHost,
    DenyHttpOutbound,
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

    fn action_vocabulary(&self) -> Vec<PluginCapabilityAction> {
        vec![
            PluginCapabilityAction::RegisterPlugin(PluginKind::Native),
            PluginCapabilityAction::RegisterPlugin(PluginKind::Wasm),
            PluginCapabilityAction::ClassifyGraphAcyclic,
            PluginCapabilityAction::ClassifyGraphCyclic,
            PluginCapabilityAction::ClassifyGraphMissingDependency,
            PluginCapabilityAction::ResolveProviderSelf,
            PluginCapabilityAction::ResolveProviderMissing,
            PluginCapabilityAction::AllowCapability,
            PluginCapabilityAction::GrantPrivilegedCapability,
            PluginCapabilityAction::AllowHttpHost,
            PluginCapabilityAction::DenyHttpOutbound,
        ]
    }

    fn transition_state(
        &self,
        prev: &PluginCapabilityState,
        action: &PluginCapabilityAction,
    ) -> Option<PluginCapabilityState> {
        let mut candidate = prev.clone();
        let allowed = match action {
            PluginCapabilityAction::RegisterPlugin(kind) if prev.plugin_kind.is_none() => {
                candidate.plugin_kind = Some(kind.clone());
                true
            }
            PluginCapabilityAction::ClassifyGraphAcyclic
                if prev.plugin_kind.is_some()
                    && matches!(prev.graph, DependencyGraphClass::Empty) =>
            {
                candidate.graph = DependencyGraphClass::Acyclic;
                true
            }
            PluginCapabilityAction::ClassifyGraphCyclic
                if prev.plugin_kind.is_some()
                    && matches!(prev.graph, DependencyGraphClass::Empty) =>
            {
                candidate.graph = DependencyGraphClass::Cyclic;
                true
            }
            PluginCapabilityAction::ClassifyGraphMissingDependency
                if prev.plugin_kind.is_some()
                    && matches!(prev.graph, DependencyGraphClass::Empty) =>
            {
                candidate.graph = DependencyGraphClass::MissingDependency;
                true
            }
            PluginCapabilityAction::ResolveProviderSelf
                if matches!(prev.graph, DependencyGraphClass::Acyclic)
                    && matches!(prev.provider, ProviderResolutionClass::Unresolved) =>
            {
                candidate.provider = ProviderResolutionClass::SelfComponent;
                true
            }
            PluginCapabilityAction::ResolveProviderMissing
                if !matches!(
                    prev.graph,
                    DependencyGraphClass::Empty | DependencyGraphClass::Acyclic
                ) && matches!(prev.provider, ProviderResolutionClass::Unresolved) =>
            {
                candidate.provider = ProviderResolutionClass::Missing;
                true
            }
            PluginCapabilityAction::AllowCapability
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) && !matches!(prev.capability, CapabilityDecision::Allowed) =>
            {
                candidate.capability = CapabilityDecision::Allowed;
                true
            }
            PluginCapabilityAction::GrantPrivilegedCapability
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) && !matches!(prev.capability, CapabilityDecision::Privileged) =>
            {
                candidate.capability = CapabilityDecision::Privileged;
                true
            }
            PluginCapabilityAction::AllowHttpHost
                if matches!(
                    prev.provider,
                    ProviderResolutionClass::SelfComponent | ProviderResolutionClass::Dependency
                ) && !matches!(prev.capability, CapabilityDecision::Denied)
                    && matches!(prev.http_outbound, HttpOutboundClass::None) =>
            {
                candidate.http_outbound = HttpOutboundClass::Host;
                true
            }
            PluginCapabilityAction::DenyHttpOutbound
                if !matches!(prev.http_outbound, HttpOutboundClass::None) =>
            {
                candidate.http_outbound = HttpOutboundClass::None;
                true
            }
            _ => false,
        };
        allowed.then_some(candidate)
    }
}

fn plugin_capability_model_cases() -> Vec<ModelCase<PluginCapabilityState, PluginCapabilityAction>>
{
    vec![ModelCase::default().with_check_deadlocks(false)]
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
                    | PluginCapabilityAction::GrantPrivilegedCapability
            ) && prev.provider != next.provider
                || next.capability != prev.capability
        },
    ))
}

#[subsystem_spec(model_cases(plugin_capability_model_cases))]
impl TransitionSystem for PluginCapabilitySpec {
    type State = PluginCapabilityState;
    type Action = PluginCapabilityAction;

    fn name(&self) -> &'static str {
        "plugin_capability"
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

#[nirvash_macros::formal_tests(spec = PluginCapabilitySpec)]
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
