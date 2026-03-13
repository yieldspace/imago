use std::any::{Any, TypeId};

pub use inventory;
pub use nirvash_foundation::{RegisteredSymbolicStateSchema, lookup_symbolic_state_schema};

pub struct RegisteredInvariant {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredProperty {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredFairness {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredStateConstraint {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub case_labels: Option<&'static [&'static str]>,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredActionConstraint {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub case_labels: Option<&'static [&'static str]>,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredSymmetry {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredActionDocLabel {
    pub value_type_id: fn() -> TypeId,
    pub format: fn(&dyn Any) -> Option<String>,
}

pub struct RegisteredActionDocPresentation {
    pub value_type_id: fn() -> TypeId,
    pub format: fn(&dyn Any) -> Option<crate::DocGraphActionPresentation>,
}

pub struct RegisteredSymbolicPureHelper {
    pub key: &'static str,
}

pub struct RegisteredSymbolicEffect {
    pub key: &'static str,
}

inventory::collect!(RegisteredInvariant);
inventory::collect!(RegisteredProperty);
inventory::collect!(RegisteredFairness);
inventory::collect!(RegisteredStateConstraint);
inventory::collect!(RegisteredActionConstraint);
inventory::collect!(RegisteredSymmetry);
inventory::collect!(RegisteredActionDocLabel);
inventory::collect!(RegisteredActionDocPresentation);
inventory::collect!(RegisteredSymbolicPureHelper);
inventory::collect!(RegisteredSymbolicEffect);

pub fn lookup_action_doc_label(value: &dyn Any) -> Option<String> {
    let value_type_id = value.type_id();
    inventory::iter::<RegisteredActionDocLabel>
        .into_iter()
        .filter(|entry| (entry.value_type_id)() == value_type_id)
        .find_map(|entry| (entry.format)(value))
        .filter(|label| !label.trim().is_empty())
}

pub fn lookup_action_doc_presentation(
    value: &dyn Any,
) -> Option<crate::DocGraphActionPresentation> {
    let value_type_id = value.type_id();
    inventory::iter::<RegisteredActionDocPresentation>
        .into_iter()
        .filter(|entry| (entry.value_type_id)() == value_type_id)
        .find_map(|entry| (entry.format)(value))
        .filter(|presentation| !presentation.label.trim().is_empty())
}

pub fn has_registered_symbolic_pure_helper(key: &str) -> bool {
    inventory::iter::<RegisteredSymbolicPureHelper>
        .into_iter()
        .any(|entry| entry.key == key)
}

pub fn registered_symbolic_pure_helper_keys() -> Vec<&'static str> {
    let mut keys: Vec<_> = inventory::iter::<RegisteredSymbolicPureHelper>
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

pub fn has_registered_symbolic_effect(key: &str) -> bool {
    inventory::iter::<RegisteredSymbolicEffect>
        .into_iter()
        .any(|entry| entry.key == key)
}

pub fn registered_symbolic_effect_keys() -> Vec<&'static str> {
    let mut keys: Vec<_> = inventory::iter::<RegisteredSymbolicEffect>
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}
