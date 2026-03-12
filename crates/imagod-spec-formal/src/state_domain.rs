use nirvash::{BoundedDomain, TransitionSystem};

pub fn reachable_state_domain<T>(spec: &T) -> BoundedDomain<T::State>
where
    T: TransitionSystem,
    T::State: PartialEq,
{
    reachable_state_domain_with_action_filter(spec, &|_, _, _| true)
}

pub fn reachable_state_domain_with_action_filter<T>(
    spec: &T,
    action_allowed: &dyn Fn(&T::State, &T::Action, &T::State) -> bool,
) -> BoundedDomain<T::State>
where
    T: TransitionSystem,
    T::State: PartialEq,
{
    let mut states = spec.initial_states();
    let mut cursor = 0;

    while let Some(state) = states.get(cursor).cloned() {
        for (_action, next) in spec
            .successors_constrained(&state, &|action, next| action_allowed(&state, action, next))
        {
            if !states.contains(&next) {
                states.push(next);
            }
        }

        if spec.allow_stutter() {
            let stutter = spec.stutter_state(&state);
            if !states.contains(&stutter) {
                states.push(stutter);
            }
        }

        cursor += 1;
    }

    BoundedDomain::new(states)
}
