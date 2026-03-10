use std::{collections::BTreeSet, fmt};

#[derive(Clone, PartialEq, Eq)]
pub struct ConcurrentAction<A> {
    atoms: Vec<A>,
}

impl<A> ConcurrentAction<A> {
    pub fn new(atoms: Vec<A>) -> Option<Self>
    where
        A: Eq,
    {
        if atoms.is_empty() || has_duplicates(&atoms) {
            return None;
        }
        Some(Self { atoms })
    }

    pub fn from_atomic(atom: A) -> Self {
        Self { atoms: vec![atom] }
    }

    pub fn atoms(&self) -> &[A] {
        &self.atoms
    }

    pub fn arity(&self) -> usize {
        self.atoms.len()
    }
}

impl<A> fmt::Debug for ConcurrentAction<A>
where
    A: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.atoms.as_slice() {
            [atom] => atom.fmt(f),
            atoms => {
                write!(f, "parallel(")?;
                for (index, atom) in atoms.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{atom:?}")?;
                }
                write!(f, ")")
            }
        }
    }
}

pub trait ConcurrentTransitionSystem {
    type State: Clone + fmt::Debug + Eq + 'static;
    type AtomicAction: Clone + fmt::Debug + Eq + 'static;
    type ResourceKey: Clone + fmt::Debug + Eq + Ord + 'static;

    fn initial_states(&self) -> Vec<Self::State>;

    fn atomic_actions(&self) -> Vec<Self::AtomicAction>;

    fn atomic_transition(
        &self,
        state: &Self::State,
        action: &Self::AtomicAction,
    ) -> Option<Self::State>;

    fn footprint_reads(&self, action: &Self::AtomicAction) -> BTreeSet<Self::ResourceKey>;

    fn footprint_writes(&self, action: &Self::AtomicAction) -> BTreeSet<Self::ResourceKey>;

    fn allow_stutter(&self) -> bool {
        true
    }

    fn stutter_state(&self, state: &Self::State) -> Self::State {
        state.clone()
    }

    fn synthesized_actions(&self) -> Vec<ConcurrentAction<Self::AtomicAction>> {
        synthesize_concurrent_actions(
            dedupe_preserving_order(self.atomic_actions()),
            |left, right| self.actions_conflict(left, right),
        )
    }

    fn enabled_atomic_actions(&self, state: &Self::State) -> Vec<Self::AtomicAction> {
        dedupe_preserving_order(self.atomic_actions())
            .into_iter()
            .filter(|action| self.atomic_transition(state, action).is_some())
            .collect()
    }

    fn synthesized_transition(
        &self,
        state: &Self::State,
        action: &ConcurrentAction<Self::AtomicAction>,
    ) -> Option<Self::State> {
        if action.atoms().is_empty() || !self.actions_are_independent(action.atoms()) {
            return None;
        }

        if !action
            .atoms()
            .iter()
            .all(|atom| self.atomic_transition(state, atom).is_some())
        {
            return None;
        }

        let mut current = state.clone();
        for atom in action.atoms() {
            current = self.atomic_transition(&current, atom)?;
        }

        Some(current)
    }

    fn synthesized_successors(
        &self,
        state: &Self::State,
    ) -> Vec<(ConcurrentAction<Self::AtomicAction>, Self::State)> {
        synthesize_concurrent_actions(self.enabled_atomic_actions(state), |left, right| {
            self.actions_conflict(left, right)
        })
        .into_iter()
        .filter_map(|action| {
            self.synthesized_transition(state, &action)
                .map(|next| (action, next))
        })
        .collect()
    }

    fn synthesized_successors_filtered<F>(
        &self,
        state: &Self::State,
        action_allowed: F,
    ) -> Vec<(ConcurrentAction<Self::AtomicAction>, Self::State)>
    where
        F: Fn(&ConcurrentAction<Self::AtomicAction>, &Self::State) -> bool,
    {
        // Filter single-atom candidates first so case-level action families can
        // prune concurrent subset synthesis before the combinatorial blow-up.
        let enabled_atoms = self
            .enabled_atomic_actions(state)
            .into_iter()
            .filter(|atom| {
                let atomic = ConcurrentAction::from_atomic(atom.clone());
                self.synthesized_transition(state, &atomic)
                    .is_some_and(|next| action_allowed(&atomic, &next))
            })
            .collect();

        synthesize_concurrent_actions(enabled_atoms, |left, right| {
            self.actions_conflict(left, right)
        })
        .into_iter()
        .filter_map(|action| {
            self.synthesized_transition(state, &action)
                .filter(|next| action_allowed(&action, next))
                .map(|next| (action, next))
        })
        .collect()
    }

    fn actions_conflict(&self, left: &Self::AtomicAction, right: &Self::AtomicAction) -> bool {
        let left_reads = self.footprint_reads(left);
        let left_writes = self.footprint_writes(left);
        let right_reads = self.footprint_reads(right);
        let right_writes = self.footprint_writes(right);

        intersects(&left_writes, &right_writes)
            || intersects(&left_writes, &right_reads)
            || intersects(&right_writes, &left_reads)
    }

    fn actions_are_independent(&self, atoms: &[Self::AtomicAction]) -> bool {
        for (index, left) in atoms.iter().enumerate() {
            for right in atoms.iter().skip(index + 1) {
                if self.actions_conflict(left, right) {
                    return false;
                }
            }
        }
        true
    }
}

fn synthesize_concurrent_actions<A, F>(atoms: Vec<A>, conflicts: F) -> Vec<ConcurrentAction<A>>
where
    A: Clone + Eq,
    F: Fn(&A, &A) -> bool,
{
    let atoms = dedupe_preserving_order(atoms);
    let mut synthesized = Vec::new();

    for subset in non_empty_subsets(&atoms) {
        if subset_is_independent(&subset, &conflicts) {
            synthesized.push(
                ConcurrentAction::new(subset)
                    .expect("generated concurrent actions are non-empty and unique"),
            );
        }
    }

    synthesized
}

fn non_empty_subsets<A>(atoms: &[A]) -> Vec<Vec<A>>
where
    A: Clone,
{
    let mut subsets = Vec::new();
    let mut current = Vec::new();
    for size in 1..=atoms.len() {
        combinations(atoms, size, 0, &mut current, &mut subsets);
    }
    subsets
}

fn combinations<A>(
    atoms: &[A],
    target_size: usize,
    start: usize,
    current: &mut Vec<A>,
    subsets: &mut Vec<Vec<A>>,
) where
    A: Clone,
{
    if current.len() == target_size {
        subsets.push(current.clone());
        return;
    }

    for index in start..atoms.len() {
        current.push(atoms[index].clone());
        combinations(atoms, target_size, index + 1, current, subsets);
        current.pop();
    }
}

fn subset_is_independent<A, F>(atoms: &[A], conflicts: &F) -> bool
where
    F: Fn(&A, &A) -> bool,
{
    for (index, left) in atoms.iter().enumerate() {
        for right in atoms.iter().skip(index + 1) {
            if conflicts(left, right) {
                return false;
            }
        }
    }
    true
}

fn has_duplicates<A>(atoms: &[A]) -> bool
where
    A: Eq,
{
    for (index, atom) in atoms.iter().enumerate() {
        if atoms
            .iter()
            .skip(index + 1)
            .any(|candidate| candidate == atom)
        {
            return true;
        }
    }
    false
}

fn dedupe_preserving_order<A>(atoms: Vec<A>) -> Vec<A>
where
    A: Eq,
{
    let mut deduped = Vec::new();
    for atom in atoms {
        if !deduped.iter().any(|existing| existing == &atom) {
            deduped.push(atom);
        }
    }
    deduped
}

fn intersects<T>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> bool
where
    T: Ord,
{
    left.iter().any(|value| right.contains(value))
}

#[cfg(test)]
mod tests {
    use std::any::{Any, TypeId};

    use super::*;
    use crate::{
        DocGraphActionPresentation, DocGraphProcessKind, DocGraphProcessStep,
        RegisteredActionDocPresentation, TransitionSystem, describe_doc_graph_action,
        format_doc_graph_action,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum ResourceKey {
        Left,
        Right,
        Gate,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct CounterState {
        left: bool,
        right: bool,
        gate: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AtomicAction {
        IncLeft,
        IncRight,
        ToggleGate,
        ResetLeft,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct ConcurrentSpec;

    impl ConcurrentTransitionSystem for ConcurrentSpec {
        type State = CounterState;
        type AtomicAction = AtomicAction;
        type ResourceKey = ResourceKey;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![CounterState {
                left: false,
                right: false,
                gate: false,
            }]
        }

        fn atomic_actions(&self) -> Vec<Self::AtomicAction> {
            vec![
                AtomicAction::IncLeft,
                AtomicAction::IncRight,
                AtomicAction::ToggleGate,
                AtomicAction::ResetLeft,
            ]
        }

        fn atomic_transition(
            &self,
            state: &Self::State,
            action: &Self::AtomicAction,
        ) -> Option<Self::State> {
            let mut next = *state;
            match action {
                AtomicAction::IncLeft if !state.left => {
                    next.left = true;
                    Some(next)
                }
                AtomicAction::IncRight if !state.right => {
                    next.right = true;
                    Some(next)
                }
                AtomicAction::ToggleGate if !state.gate => {
                    next.gate = true;
                    Some(next)
                }
                AtomicAction::ResetLeft if state.left => {
                    next.left = false;
                    Some(next)
                }
                _ => None,
            }
        }

        fn footprint_reads(&self, action: &Self::AtomicAction) -> BTreeSet<Self::ResourceKey> {
            match action {
                AtomicAction::IncLeft | AtomicAction::ResetLeft => {
                    BTreeSet::from([ResourceKey::Left])
                }
                AtomicAction::IncRight => BTreeSet::from([ResourceKey::Right]),
                AtomicAction::ToggleGate => BTreeSet::from([ResourceKey::Gate]),
            }
        }

        fn footprint_writes(&self, action: &Self::AtomicAction) -> BTreeSet<Self::ResourceKey> {
            self.footprint_reads(action)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LegacyState {
        Idle,
        Busy,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LegacyAction {
        Start,
        Stop,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct LegacySpec;

    fn atomic_action_type_id() -> TypeId {
        TypeId::of::<AtomicAction>()
    }

    fn atomic_action_presentation(value: &dyn Any) -> Option<DocGraphActionPresentation> {
        let value = value
            .downcast_ref::<AtomicAction>()
            .expect("registered action doc downcast");
        let label = match value {
            AtomicAction::IncLeft => "Increment left",
            AtomicAction::IncRight => "Increment right",
            AtomicAction::ToggleGate => "Toggle gate",
            AtomicAction::ResetLeft => "Reset left",
        };
        Some(DocGraphActionPresentation::new(label))
    }

    inventory::submit! {
        RegisteredActionDocPresentation {
            value_type_id: atomic_action_type_id,
            format: atomic_action_presentation,
        }
    }

    fn concurrent_action_type_id() -> TypeId {
        TypeId::of::<ConcurrentAction<AtomicAction>>()
    }

    fn concurrent_action_presentation(value: &dyn Any) -> Option<DocGraphActionPresentation> {
        let action = value
            .downcast_ref::<ConcurrentAction<AtomicAction>>()
            .expect("registered action doc downcast");
        let steps = action
            .atoms()
            .iter()
            .map(describe_doc_graph_action)
            .collect::<Vec<_>>();
        let label = if steps.len() == 1 {
            steps[0].label.clone()
        } else {
            format!(
                "parallel({})",
                steps
                    .iter()
                    .map(|step| step.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Some(DocGraphActionPresentation::with_steps(
            label,
            Vec::new(),
            steps
                .into_iter()
                .flat_map(|step| step.process_steps)
                .map(|inner| match inner.actor {
                    Some(actor) => DocGraphProcessStep::for_actor(actor, inner.kind, inner.label),
                    None => DocGraphProcessStep::for_actor("Concurrent", inner.kind, inner.label),
                })
                .collect(),
        ))
    }

    inventory::submit! {
        RegisteredActionDocPresentation {
            value_type_id: concurrent_action_type_id,
            format: concurrent_action_presentation,
        }
    }

    impl TransitionSystem for LegacySpec {
        type State = LegacyState;
        type Action = LegacyAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![LegacyState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![LegacyAction::Start, LegacyAction::Stop]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (LegacyState::Idle, LegacyAction::Start) => Some(LegacyState::Busy),
                (LegacyState::Busy, LegacyAction::Stop) => Some(LegacyState::Idle),
                _ => None,
            }
        }
    }

    #[test]
    fn concurrent_action_rejects_empty_and_duplicate_atoms() {
        assert!(ConcurrentAction::<AtomicAction>::new(Vec::new()).is_none());
        assert!(
            ConcurrentAction::new(vec![AtomicAction::IncLeft, AtomicAction::IncLeft]).is_none()
        );
    }

    #[test]
    fn synthesized_actions_preserve_declared_order_and_include_n_ary_composites() {
        let actions = ConcurrentSpec
            .synthesized_actions()
            .into_iter()
            .map(|action| format!("{action:?}"))
            .collect::<Vec<_>>();

        assert_eq!(
            actions,
            vec![
                "IncLeft",
                "IncRight",
                "ToggleGate",
                "ResetLeft",
                "parallel(IncLeft, IncRight)",
                "parallel(IncLeft, ToggleGate)",
                "parallel(IncRight, ToggleGate)",
                "parallel(IncRight, ResetLeft)",
                "parallel(ToggleGate, ResetLeft)",
                "parallel(IncLeft, IncRight, ToggleGate)",
                "parallel(IncRight, ToggleGate, ResetLeft)",
            ]
        );
    }

    #[test]
    fn conflicting_actions_are_not_synthesized_together() {
        let actions = ConcurrentSpec
            .synthesized_actions()
            .into_iter()
            .map(|action| format!("{action:?}"))
            .collect::<Vec<_>>();

        assert!(!actions.contains(&"parallel(IncLeft, ResetLeft)".to_string()));
        assert!(!actions.contains(&"parallel(IncLeft, IncRight, ResetLeft)".to_string()));
    }

    #[test]
    fn synthesized_transition_rejects_composites_with_disabled_atoms() {
        let action = ConcurrentAction::new(vec![AtomicAction::IncLeft, AtomicAction::IncRight])
            .expect("non-empty");

        assert_eq!(
            ConcurrentSpec.synthesized_transition(
                &CounterState {
                    left: false,
                    right: false,
                    gate: false,
                },
                &action
            ),
            Some(CounterState {
                left: true,
                right: true,
                gate: false,
            })
        );
        assert_eq!(
            ConcurrentSpec.synthesized_transition(
                &CounterState {
                    left: true,
                    right: false,
                    gate: false,
                },
                &action
            ),
            None
        );
    }

    #[test]
    fn synthesized_successors_include_parallel_steps() {
        let successors = ConcurrentSpec
            .synthesized_successors(&CounterState {
                left: false,
                right: false,
                gate: false,
            })
            .into_iter()
            .map(|(action, state)| (format!("{action:?}"), state))
            .collect::<Vec<_>>();

        assert!(successors.contains(&(
            "parallel(IncLeft, IncRight)".to_string(),
            CounterState {
                left: true,
                right: true,
                gate: false,
            }
        )));
        assert!(successors.contains(&(
            "parallel(IncLeft, IncRight, ToggleGate)".to_string(),
            CounterState {
                left: true,
                right: true,
                gate: true,
            }
        )));
    }

    #[test]
    fn doc_graph_formats_parallel_actions_with_parallel_prefix() {
        let action = ConcurrentAction::new(vec![AtomicAction::IncLeft, AtomicAction::IncRight])
            .expect("non-empty");

        assert_eq!(
            format_doc_graph_action(&action),
            "parallel(Increment left, Increment right)"
        );
        assert_eq!(
            format_doc_graph_action(&ConcurrentAction::from_atomic(AtomicAction::IncLeft)),
            "Increment left"
        );
        assert_eq!(
            describe_doc_graph_action(&action).process_steps,
            vec![
                DocGraphProcessStep::for_actor(
                    "Concurrent",
                    DocGraphProcessKind::Do,
                    "Increment left",
                ),
                DocGraphProcessStep::for_actor(
                    "Concurrent",
                    DocGraphProcessKind::Do,
                    "Increment right",
                ),
            ]
        );
    }

    #[test]
    fn legacy_transition_system_behavior_is_unchanged() {
        let spec = LegacySpec;

        assert_eq!(
            spec.actions(),
            vec![LegacyAction::Start, LegacyAction::Stop]
        );
        assert_eq!(
            spec.successors(&LegacyState::Idle),
            vec![(LegacyAction::Start, LegacyState::Busy)]
        );
    }
}
