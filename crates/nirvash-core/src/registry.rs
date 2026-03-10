use std::{
    any::{Any, TypeId, type_name},
    collections::BTreeSet,
};

use crate::{
    ActionConstraint, Fairness, Ltl, StateConstraint, StatePredicate, SymmetryReducer,
    TransitionSystem,
};

pub use inventory;

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

type ErasedBuilder = fn() -> Box<dyn Any>;
type NamedBuilder = (&'static str, ErasedBuilder);

#[derive(Debug, Clone, Copy)]
pub struct ScopedStateConstraint<S> {
    name: &'static str,
    case_labels: Option<&'static [&'static str]>,
    constraint: StateConstraint<S>,
}

impl<S> ScopedStateConstraint<S> {
    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn case_labels(&self) -> Option<&'static [&'static str]> {
        self.case_labels
    }

    pub const fn constraint(&self) -> StateConstraint<S> {
        self.constraint
    }

    pub fn applies_to(&self, case_label: &str) -> bool {
        self.case_labels
            .is_none_or(|labels| labels.contains(&case_label))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScopedActionConstraint<S, A> {
    name: &'static str,
    case_labels: Option<&'static [&'static str]>,
    constraint: ActionConstraint<S, A>,
}

impl<S, A> ScopedActionConstraint<S, A> {
    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn case_labels(&self) -> Option<&'static [&'static str]> {
        self.case_labels
    }

    pub const fn constraint(&self) -> ActionConstraint<S, A> {
        self.constraint
    }

    pub fn applies_to(&self, case_label: &str) -> bool {
        self.case_labels
            .is_none_or(|labels| labels.contains(&case_label))
    }
}

inventory::collect!(RegisteredInvariant);
inventory::collect!(RegisteredProperty);
inventory::collect!(RegisteredFairness);
inventory::collect!(RegisteredStateConstraint);
inventory::collect!(RegisteredActionConstraint);
inventory::collect!(RegisteredSymmetry);
inventory::collect!(RegisteredActionDocLabel);
inventory::collect!(RegisteredActionDocPresentation);

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
    collect_scoped_state_constraints::<T>()
        .into_iter()
        .map(|entry| entry.constraint())
        .collect()
}

pub fn collect_scoped_state_constraints<T>() -> Vec<ScopedStateConstraint<T::State>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
{
    let spec_name = type_name::<T>();
    let spec_type_id = TypeId::of::<T>();
    let mut matched = inventory::iter::<RegisteredStateConstraint>
        .into_iter()
        .filter(|entry| (entry.spec_type_id)() == spec_type_id)
        .collect::<Vec<_>>();
    matched.sort_by_key(|entry| entry.name);

    let mut seen = BTreeSet::new();
    for entry in &matched {
        if !seen.insert(entry.name) {
            panic!(
                "duplicate state constraint registration `{}` for spec `{spec_name}`",
                entry.name
            );
        }
    }

    matched
        .into_iter()
        .map(|entry| ScopedStateConstraint {
            name: entry.name,
            case_labels: entry.case_labels,
            constraint: downcast_registered::<StateConstraint<T::State>>(
                (entry.build)(),
                spec_name,
                "state constraint",
                entry.name,
            ),
        })
        .collect()
}

pub fn collect_action_constraints<T>() -> Vec<ActionConstraint<T::State, T::Action>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    collect_scoped_action_constraints::<T>()
        .into_iter()
        .map(|entry| entry.constraint())
        .collect()
}

pub fn collect_scoped_action_constraints<T>() -> Vec<ScopedActionConstraint<T::State, T::Action>>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    let spec_name = type_name::<T>();
    let spec_type_id = TypeId::of::<T>();
    let mut matched = inventory::iter::<RegisteredActionConstraint>
        .into_iter()
        .filter(|entry| (entry.spec_type_id)() == spec_type_id)
        .collect::<Vec<_>>();
    matched.sort_by_key(|entry| entry.name);

    let mut seen = BTreeSet::new();
    for entry in &matched {
        if !seen.insert(entry.name) {
            panic!(
                "duplicate action constraint registration `{}` for spec `{spec_name}`",
                entry.name
            );
        }
    }

    matched
        .into_iter()
        .map(|entry| ScopedActionConstraint {
            name: entry.name,
            case_labels: entry.case_labels,
            constraint: downcast_registered::<ActionConstraint<T::State, T::Action>>(
                (entry.build)(),
                spec_name,
                "action constraint",
                entry.name,
            ),
        })
        .collect()
}

fn validate_case_labels(
    spec_name: &'static str,
    kind: &'static str,
    name: &'static str,
    case_labels: Option<&'static [&'static str]>,
    available_labels: &BTreeSet<&'static str>,
) {
    if let Some(labels) = case_labels {
        for label in labels {
            assert!(
                available_labels.contains(label),
                "registered {kind} `{name}` references unknown model case `{label}` for spec `{spec_name}`"
            );
        }
    }
}

pub fn apply_registered_model_case_metadata<T>(
    model_cases: &mut Vec<crate::ModelCase<T::State, T::Action>>,
) where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    let state_constraints = collect_scoped_state_constraints::<T>();
    let action_constraints = collect_scoped_action_constraints::<T>();
    let symmetry = collect_symmetry::<T>();
    let spec_name = type_name::<T>();
    let available_labels = model_cases
        .iter()
        .map(|model_case| model_case.label())
        .collect::<BTreeSet<_>>();

    for entry in &state_constraints {
        validate_case_labels(
            spec_name,
            "state constraint",
            entry.name(),
            entry.case_labels(),
            &available_labels,
        );
    }
    for entry in &action_constraints {
        validate_case_labels(
            spec_name,
            "action constraint",
            entry.name(),
            entry.case_labels(),
            &available_labels,
        );
    }

    for model_case in model_cases {
        let mut next_model_case = ::core::mem::take(model_case);
        for constraint in &state_constraints {
            if constraint.applies_to(next_model_case.label()) {
                next_model_case = next_model_case.with_state_constraint(constraint.constraint());
            }
        }
        for constraint in &action_constraints {
            if constraint.applies_to(next_model_case.label()) {
                next_model_case = next_model_case.with_action_constraint(constraint.constraint());
            }
        }
        if next_model_case.symmetry().is_none()
            && let Some(symmetry) = symmetry
        {
            next_model_case = next_model_case.with_symmetry(symmetry);
        }
        *model_case = next_model_case;
    }
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

pub fn collect_symmetry_name<T>() -> Option<String>
where
    T: TransitionSystem + 'static,
    T::State: 'static,
{
    let matched = sorted_builders::<T, _>(
        inventory::iter::<RegisteredSymmetry>
            .into_iter()
            .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
        "symmetry",
    );
    matched.into_iter().next().map(|(name, _)| name.to_owned())
}

pub fn collect_spec_viz_registrations<T>() -> crate::SpecVizRegistrationSet
where
    T: TransitionSystem + 'static,
    T::State: 'static,
    T::Action: 'static,
{
    crate::SpecVizRegistrationSet {
        invariants: collect_invariants::<T>()
            .into_iter()
            .map(|predicate| predicate.name().to_owned())
            .collect(),
        properties: collect_properties::<T>()
            .into_iter()
            .map(|property| property.describe().to_owned())
            .collect(),
        fairness: collect_fairness::<T>()
            .into_iter()
            .map(|fairness| fairness.name().to_owned())
            .collect(),
        state_constraints: collect_scoped_state_constraints::<T>()
            .into_iter()
            .map(|constraint| constraint.name().to_owned())
            .collect(),
        action_constraints: collect_scoped_action_constraints::<T>()
            .into_iter()
            .map(|constraint| constraint.name().to_owned())
            .collect(),
        symmetries: collect_symmetry_name::<T>().into_iter().collect(),
    }
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

        fn initial_states(&self) -> Vec<Self::State> {
            vec![RegistryState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![RegistryAction::Tick]
        }

        fn transition(&self, _: &Self::State, _: &Self::Action) -> Option<Self::State> {
            None
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct DuplicateSpec;

    impl TransitionSystem for DuplicateSpec {
        type State = RegistryState;
        type Action = RegistryAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![RegistryState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![RegistryAction::Tick]
        }

        fn transition(&self, _: &Self::State, _: &Self::Action) -> Option<Self::State> {
            None
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

    #[derive(Debug, Clone, Copy, Default)]
    struct ScopedCaseSpec;

    impl TransitionSystem for ScopedCaseSpec {
        type State = RegistryState;
        type Action = RegistryAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![RegistryState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![RegistryAction::Tick]
        }

        fn transition(&self, _: &Self::State, _: &Self::Action) -> Option<Self::State> {
            None
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct UnknownCaseSpec;

    impl TransitionSystem for UnknownCaseSpec {
        type State = RegistryState;
        type Action = RegistryAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![RegistryState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![RegistryAction::Tick]
        }

        fn transition(&self, _: &Self::State, _: &Self::Action) -> Option<Self::State> {
            None
        }
    }

    fn scoped_case_spec_type_id() -> TypeId {
        TypeId::of::<ScopedCaseSpec>()
    }

    fn unknown_case_spec_type_id() -> TypeId {
        TypeId::of::<UnknownCaseSpec>()
    }

    fn global_state_constraint() -> StateConstraint<RegistryState> {
        StateConstraint::new("global_state_constraint", |_| true)
    }

    fn only_case_a_state_constraint() -> StateConstraint<RegistryState> {
        StateConstraint::new("only_case_a_state_constraint", |_| true)
    }

    fn only_case_b_action_constraint() -> ActionConstraint<RegistryState, RegistryAction> {
        ActionConstraint::new("only_case_b_action_constraint", |_, _, _| true)
    }

    fn unknown_case_state_constraint() -> StateConstraint<RegistryState> {
        StateConstraint::new("unknown_case_state_constraint", |_| true)
    }

    fn build_global_state_constraint() -> Box<dyn Any> {
        Box::new(global_state_constraint())
    }

    fn build_only_case_a_state_constraint() -> Box<dyn Any> {
        Box::new(only_case_a_state_constraint())
    }

    fn build_only_case_b_action_constraint() -> Box<dyn Any> {
        Box::new(only_case_b_action_constraint())
    }

    fn build_unknown_case_state_constraint() -> Box<dyn Any> {
        Box::new(unknown_case_state_constraint())
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

    crate::inventory::submit! {
        RegisteredStateConstraint {
            spec_type_id: scoped_case_spec_type_id,
            name: "global_state_constraint",
            case_labels: None,
            build: build_global_state_constraint,
        }
    }

    crate::inventory::submit! {
        RegisteredStateConstraint {
            spec_type_id: scoped_case_spec_type_id,
            name: "only_case_a_state_constraint",
            case_labels: Some(&["case_a"]),
            build: build_only_case_a_state_constraint,
        }
    }

    crate::inventory::submit! {
        RegisteredActionConstraint {
            spec_type_id: scoped_case_spec_type_id,
            name: "only_case_b_action_constraint",
            case_labels: Some(&["case_b"]),
            build: build_only_case_b_action_constraint,
        }
    }

    crate::inventory::submit! {
        RegisteredStateConstraint {
            spec_type_id: unknown_case_spec_type_id,
            name: "unknown_case_state_constraint",
            case_labels: Some(&["missing_case"]),
            build: build_unknown_case_state_constraint,
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

    #[test]
    fn apply_registered_model_case_metadata_scopes_constraints_by_case_label() {
        let mut model_cases = vec![
            crate::ModelCase::<RegistryState, RegistryAction>::new("case_a"),
            crate::ModelCase::<RegistryState, RegistryAction>::new("case_b"),
        ];

        apply_registered_model_case_metadata::<ScopedCaseSpec>(&mut model_cases);

        assert_eq!(
            model_cases[0]
                .state_constraints()
                .iter()
                .map(|constraint| constraint.name())
                .collect::<Vec<_>>(),
            vec!["global_state_constraint", "only_case_a_state_constraint"]
        );
        assert!(model_cases[0].action_constraints().is_empty());

        assert_eq!(
            model_cases[1]
                .state_constraints()
                .iter()
                .map(|constraint| constraint.name())
                .collect::<Vec<_>>(),
            vec!["global_state_constraint"]
        );
        assert_eq!(
            model_cases[1]
                .action_constraints()
                .iter()
                .map(|constraint| constraint.name())
                .collect::<Vec<_>>(),
            vec!["only_case_b_action_constraint"]
        );
    }

    #[test]
    fn apply_registered_model_case_metadata_rejects_unknown_case_labels() {
        let panic = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut model_cases = vec![crate::ModelCase::<RegistryState, RegistryAction>::new(
                "case_a",
            )];
            apply_registered_model_case_metadata::<UnknownCaseSpec>(&mut model_cases);
        }))
        .expect_err("unknown case labels must panic");

        let message = panic_message(panic);
        assert!(message.contains("unknown model case `missing_case`"));
        assert!(message.contains("unknown_case_state_constraint"));
        assert!(message.contains(type_name::<UnknownCaseSpec>()));
    }
}
