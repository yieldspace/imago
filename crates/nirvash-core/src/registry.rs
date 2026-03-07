use std::{
    any::{Any, TypeId, type_name},
    collections::BTreeSet,
};

use crate::{
    ActionConstraint, Fairness, Ltl, StateConstraint, StatePredicate, StepPredicate,
    SymmetryReducer, TransitionSystem,
};

pub use inventory;

pub struct RegisteredInvariant {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredIllegal {
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
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredActionConstraint {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub build: fn() -> Box<dyn Any>,
}

pub struct RegisteredSymmetry {
    pub spec_type_id: fn() -> TypeId,
    pub name: &'static str,
    pub build: fn() -> Box<dyn Any>,
}

type ErasedBuilder = fn() -> Box<dyn Any>;
type NamedBuilder = (&'static str, ErasedBuilder);

inventory::collect!(RegisteredInvariant);
inventory::collect!(RegisteredIllegal);
inventory::collect!(RegisteredProperty);
inventory::collect!(RegisteredFairness);
inventory::collect!(RegisteredStateConstraint);
inventory::collect!(RegisteredActionConstraint);
inventory::collect!(RegisteredSymmetry);

fn sorted_builders<'a, T, I>(entries: I, kind: &'static str) -> Vec<NamedBuilder>
where
    T: TransitionSystem + 'static,
    I: IntoIterator<Item = (&'a dyn RegistryEntry, ErasedBuilder)>,
{
    let spec_type_id = TypeId::of::<T>();
    let spec_name = type_name::<T>();
    let mut matched = entries
        .into_iter()
        .filter(|(entry, _)| entry.spec_type_id() == spec_type_id)
        .map(|(entry, build)| (entry.name(), build))
        .collect::<Vec<_>>();
    matched.sort_by_key(|(name, _)| *name);

    let mut seen = BTreeSet::new();
    for (name, _) in &matched {
        if !seen.insert(*name) {
            panic!("duplicate {kind} registration `{name}` for spec `{spec_name}`");
        }
    }

    matched
}

fn downcast_registered<T>(value: Box<dyn Any>, spec_name: &'static str, kind: &str, name: &str) -> T
where
    T: 'static,
{
    *value.downcast::<T>().unwrap_or_else(|_| {
        panic!("registered {kind} `{name}` for spec `{spec_name}` has an unexpected type")
    })
}

trait RegistryEntry {
    fn spec_type_id(&self) -> TypeId;
    fn name(&self) -> &'static str;
}

macro_rules! impl_registry_entry {
    ($ty:ty) => {
        impl RegistryEntry for $ty {
            fn spec_type_id(&self) -> TypeId {
                (self.spec_type_id)()
            }

            fn name(&self) -> &'static str {
                self.name
            }
        }
    };
}

impl_registry_entry!(RegisteredInvariant);
impl_registry_entry!(RegisteredIllegal);
impl_registry_entry!(RegisteredProperty);
impl_registry_entry!(RegisteredFairness);
impl_registry_entry!(RegisteredStateConstraint);
impl_registry_entry!(RegisteredActionConstraint);
impl_registry_entry!(RegisteredSymmetry);

pub fn collect_invariants<T>() -> Vec<StatePredicate<T::State>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
{
    let spec_name = type_name::<T>();
    sorted_builders::<T, _>(
        inventory::iter::<RegisteredInvariant>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "invariant",
    )
    .into_iter()
    .map(|(name, build)| {
        downcast_registered::<StatePredicate<T::State>>(build(), spec_name, "invariant", name)
    })
    .collect()
}

pub fn collect_illegal<T>() -> Vec<StepPredicate<T::State, T::Action>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    let spec_name = type_name::<T>();
    sorted_builders::<T, _>(
        inventory::iter::<RegisteredIllegal>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "illegal predicate",
    )
    .into_iter()
    .map(|(name, build)| {
        downcast_registered::<StepPredicate<T::State, T::Action>>(
            build(),
            spec_name,
            "illegal predicate",
            name,
        )
    })
    .collect()
}

pub fn collect_properties<T>() -> Vec<Ltl<T::State, T::Action>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    let spec_name = type_name::<T>();
    sorted_builders::<T, _>(
        inventory::iter::<RegisteredProperty>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "property",
    )
    .into_iter()
    .map(|(name, build)| {
        downcast_registered::<Ltl<T::State, T::Action>>(build(), spec_name, "property", name)
    })
    .collect()
}

pub fn collect_fairness<T>() -> Vec<Fairness<T::State, T::Action>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    let spec_name = type_name::<T>();
    sorted_builders::<T, _>(
        inventory::iter::<RegisteredFairness>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "fairness",
    )
    .into_iter()
    .map(|(name, build)| {
        downcast_registered::<Fairness<T::State, T::Action>>(build(), spec_name, "fairness", name)
    })
    .collect()
}

pub fn collect_state_constraints<T>() -> Vec<StateConstraint<T::State>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
{
    let spec_name = type_name::<T>();
    sorted_builders::<T, _>(
        inventory::iter::<RegisteredStateConstraint>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "state constraint",
    )
    .into_iter()
    .map(|(name, build)| {
        downcast_registered::<StateConstraint<T::State>>(
            build(),
            spec_name,
            "state constraint",
            name,
        )
    })
    .collect()
}

pub fn collect_action_constraints<T>() -> Vec<ActionConstraint<T::State, T::Action>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    let spec_name = type_name::<T>();
    sorted_builders::<T, _>(
        inventory::iter::<RegisteredActionConstraint>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "action constraint",
    )
    .into_iter()
    .map(|(name, build)| {
        downcast_registered::<ActionConstraint<T::State, T::Action>>(
            build(),
            spec_name,
            "action constraint",
            name,
        )
    })
    .collect()
}

pub fn collect_symmetry<T>() -> Option<SymmetryReducer<T::State>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
{
    let spec_name = type_name::<T>();
    let matched = sorted_builders::<T, _>(
        inventory::iter::<RegisteredSymmetry>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "symmetry",
    );
    assert!(
        matched.len() <= 1,
        "multiple symmetry registrations for spec `{spec_name}` are not supported"
    );
    matched.into_iter().next().map(|(name, build)| {
        downcast_registered::<SymmetryReducer<T::State>>(build(), spec_name, "symmetry", name)
    })
}

#[cfg(test)]
mod tests {
    use std::panic::{self, AssertUnwindSafe};

    use super::*;
    use crate::{BoundedDomain, Signature};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RegistryState {
        Idle,
    }

    impl Signature for RegistryState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RegistryAction {
        Tick,
    }

    impl Signature for RegistryAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Tick])
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct OrderedSpec;

    impl TransitionSystem for OrderedSpec {
        type State = RegistryState;
        type Action = RegistryAction;

        fn init(&self, state: &Self::State) -> bool {
            matches!(state, RegistryState::Idle)
        }

        fn next(&self, _: &Self::State, _: &Self::Action, _: &Self::State) -> bool {
            false
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct DuplicateSpec;

    impl TransitionSystem for DuplicateSpec {
        type State = RegistryState;
        type Action = RegistryAction;

        fn init(&self, state: &Self::State) -> bool {
            matches!(state, RegistryState::Idle)
        }

        fn next(&self, _: &Self::State, _: &Self::Action, _: &Self::State) -> bool {
            false
        }
    }

    fn ordered_spec_type_id() -> TypeId {
        TypeId::of::<OrderedSpec>()
    }

    fn duplicate_spec_type_id() -> TypeId {
        TypeId::of::<DuplicateSpec>()
    }

    fn alpha_invariant() -> StatePredicate<RegistryState> {
        StatePredicate::new("alpha_invariant", |_| true)
    }

    fn zeta_invariant() -> StatePredicate<RegistryState> {
        StatePredicate::new("zeta_invariant", |_| true)
    }

    fn duplicate_a() -> StatePredicate<RegistryState> {
        StatePredicate::new("duplicate_name", |_| true)
    }

    fn duplicate_b() -> StatePredicate<RegistryState> {
        StatePredicate::new("duplicate_name", |_| true)
    }

    fn build_alpha_invariant() -> Box<dyn Any> {
        Box::new(alpha_invariant())
    }

    fn build_zeta_invariant() -> Box<dyn Any> {
        Box::new(zeta_invariant())
    }

    fn build_duplicate_a() -> Box<dyn Any> {
        Box::new(duplicate_a())
    }

    fn build_duplicate_b() -> Box<dyn Any> {
        Box::new(duplicate_b())
    }

    crate::inventory::submit! {
        RegisteredInvariant {
            spec_type_id: ordered_spec_type_id,
            name: "zeta_invariant",
            build: build_zeta_invariant,
        }
    }

    crate::inventory::submit! {
        RegisteredInvariant {
            spec_type_id: ordered_spec_type_id,
            name: "alpha_invariant",
            build: build_alpha_invariant,
        }
    }

    crate::inventory::submit! {
        RegisteredInvariant {
            spec_type_id: duplicate_spec_type_id,
            name: "duplicate_name",
            build: build_duplicate_a,
        }
    }

    crate::inventory::submit! {
        RegisteredInvariant {
            spec_type_id: duplicate_spec_type_id,
            name: "duplicate_name",
            build: build_duplicate_b,
        }
    }

    fn panic_message(payload: Box<dyn Any + Send>) -> String {
        match payload.downcast::<String>() {
            Ok(message) => *message,
            Err(payload) => match payload.downcast::<&'static str>() {
                Ok(message) => (*message).to_owned(),
                Err(_) => "unknown panic payload".to_owned(),
            },
        }
    }

    #[test]
    fn registry_collects_entries_in_deterministic_name_order() {
        let names = collect_invariants::<OrderedSpec>()
            .into_iter()
            .map(|predicate| predicate.name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["alpha_invariant", "zeta_invariant"]);
    }

    #[test]
    fn registry_rejects_duplicate_names_for_same_spec() {
        let panic = panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = collect_invariants::<DuplicateSpec>();
        }))
        .expect_err("duplicate registrations must panic");

        let message = panic_message(panic);
        assert!(message.contains("duplicate invariant registration `duplicate_name`"));
        assert!(message.contains(type_name::<DuplicateSpec>()));
    }
}
