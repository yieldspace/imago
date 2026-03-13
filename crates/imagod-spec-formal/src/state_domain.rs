use nirvash::BoundedDomain;
use nirvash_lower::FrontendSpec;

type ActionAllowed<T> = dyn Fn(
    &<T as FrontendSpec>::State,
    &<T as FrontendSpec>::Action,
    &<T as FrontendSpec>::State,
) -> bool;

pub fn reachable_state_domain<T>(spec: &T) -> BoundedDomain<T::State>
where
    T: FrontendSpec,
    T::State: PartialEq,
{
    reachable_state_domain_with_action_filter(spec, &|_, _, _| true)
}

pub fn reachable_state_domain_with_action_filter<T>(
    spec: &T,
    action_allowed: &ActionAllowed<T>,
) -> BoundedDomain<T::State>
where
    T: FrontendSpec,
    T::State: PartialEq,
{
    let mut states = spec.initial_states();
    let mut cursor = 0;

    while let Some(state) = states.get(cursor).cloned() {
        for action in spec.actions() {
            for next in spec.transition_relation(&state, &action) {
                if action_allowed(&state, &action, &next) && !states.contains(&next) {
                    states.push(next);
                }
            }
        }
        cursor += 1;
    }

    BoundedDomain::new(states)
}
