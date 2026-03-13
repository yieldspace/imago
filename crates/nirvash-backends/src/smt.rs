use std::collections::BTreeSet;

use nirvash::{SymbolicStateSchema, TransitionRule};
use z3::{
    Model, SatResult, Solver,
    ast::{Bool, Int},
};

fn int_const(value: usize) -> Int {
    Int::from_u64(value as u64)
}

pub(crate) fn bool_and(parts: &[Bool]) -> Bool {
    if parts.is_empty() {
        return Bool::from_bool(true);
    }
    let refs = parts.iter().collect::<Vec<_>>();
    Bool::and(&refs)
}

pub(crate) fn bool_or(parts: &[Bool]) -> Bool {
    if parts.is_empty() {
        return Bool::from_bool(false);
    }
    let refs = parts.iter().collect::<Vec<_>>();
    Bool::or(&refs)
}

pub(crate) fn assert_in_domain(solver: &Solver, value: &Int, domain_size: usize) {
    solver.assert(value.ge(0));
    solver.assert(value.lt(int_const(domain_size)));
}

pub(crate) fn decode_int(model: &Model, value: &Int) -> Option<usize> {
    model
        .eval(value, true)
        .and_then(|ast| ast.as_u64())
        .map(|value| value as usize)
}

pub(crate) fn block_current_model(solver: &Solver, model: &Model, values: &[Int]) {
    let mut clauses = Vec::with_capacity(values.len());
    for value in values {
        let Some(index) = decode_int(model, value) else {
            continue;
        };
        clauses.push(value.eq(int_const(index)).not());
    }
    if !clauses.is_empty() {
        solver.assert(bool_or(&clauses));
    }
}

pub(crate) fn field_indices_for_paths<S>(
    schema: &SymbolicStateSchema<S>,
    paths: impl IntoIterator<Item = &'static str>,
) -> Vec<usize> {
    let wanted = paths
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    schema
        .fields()
        .iter()
        .enumerate()
        .filter_map(|(index, field)| wanted.contains(field.path()).then_some(index))
        .collect()
}

fn all_field_indices<S>(schema: &SymbolicStateSchema<S>) -> Vec<usize> {
    (0..schema.fields().len()).collect()
}

fn enumerate_assignments_impl(
    domains: &[usize],
    values: &mut Vec<usize>,
    callback: &mut dyn FnMut(&[usize]),
) {
    if values.len() == domains.len() {
        callback(values);
        return;
    }
    let domain_size = domains[values.len()];
    for candidate in 0..domain_size {
        values.push(candidate);
        enumerate_assignments_impl(domains, values, callback);
        values.pop();
    }
}

pub(crate) fn enumerate_assignments(domains: &[usize], mut callback: impl FnMut(&[usize])) {
    let mut values = Vec::with_capacity(domains.len());
    enumerate_assignments_impl(domains, &mut values, &mut callback);
}

pub(crate) fn build_state_from_indices<S: Clone>(
    schema: &SymbolicStateSchema<S>,
    indices: &[(usize, usize)],
) -> S {
    let mut state = schema.seed_state();
    for (field_index, value_index) in indices {
        schema.fields()[*field_index].write_index(&mut state, *value_index);
    }
    state
}

pub(crate) fn encode_state_bool<S: Clone>(
    schema: &SymbolicStateSchema<S>,
    state_vars: &[Int],
    paths: &[&'static str],
    whole_state: bool,
    eval: impl Fn(&S) -> bool,
) -> Bool {
    let field_indices = if whole_state {
        all_field_indices(schema)
    } else {
        field_indices_for_paths(schema, paths.iter().copied())
    };
    if field_indices.is_empty() {
        return Bool::from_bool(eval(&schema.seed_state()));
    }

    let domains = field_indices
        .iter()
        .map(|index| schema.fields()[*index].domain_size())
        .collect::<Vec<_>>();
    let mut clauses = Vec::new();
    enumerate_assignments(&domains, |assignment| {
        let state = build_state_from_indices(
            schema,
            &field_indices
                .iter()
                .copied()
                .zip(assignment.iter().copied())
                .collect::<Vec<_>>(),
        );
        if !eval(&state) {
            return;
        }
        let matchers = field_indices
            .iter()
            .copied()
            .zip(assignment.iter().copied())
            .map(|(field_index, value_index)| state_vars[field_index].eq(int_const(value_index)))
            .collect::<Vec<_>>();
        clauses.push(bool_and(&matchers));
    });
    bool_or(&clauses)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_step_bool<S: Clone, A: Clone>(
    schema: &SymbolicStateSchema<S>,
    prev_vars: &[Int],
    step_var: &Int,
    next_vars: &[Int],
    actions: &[A],
    paths: &[&'static str],
    whole_state: bool,
    eval: impl Fn(&S, &A, &S) -> bool,
) -> Bool {
    let field_indices = if whole_state {
        all_field_indices(schema)
    } else {
        field_indices_for_paths(schema, paths.iter().copied())
    };
    let mut domains = field_indices
        .iter()
        .map(|index| schema.fields()[*index].domain_size())
        .collect::<Vec<_>>();
    let prev_len = domains.len();
    domains.extend(
        field_indices
            .iter()
            .map(|index| schema.fields()[*index].domain_size()),
    );
    domains.push(actions.len());
    let mut clauses = Vec::new();
    enumerate_assignments(&domains, |assignment| {
        let prev_state = build_state_from_indices(
            schema,
            &field_indices
                .iter()
                .copied()
                .zip(assignment[..prev_len].iter().copied())
                .collect::<Vec<_>>(),
        );
        let next_state = build_state_from_indices(
            schema,
            &field_indices
                .iter()
                .copied()
                .zip(assignment[prev_len..(prev_len * 2)].iter().copied())
                .collect::<Vec<_>>(),
        );
        let action_index = assignment[prev_len * 2];
        if !eval(&prev_state, &actions[action_index], &next_state) {
            return;
        }
        let mut matchers = field_indices
            .iter()
            .copied()
            .zip(assignment[..prev_len].iter().copied())
            .map(|(field_index, value_index)| prev_vars[field_index].eq(int_const(value_index)))
            .collect::<Vec<_>>();
        matchers.extend(
            field_indices
                .iter()
                .copied()
                .zip(assignment[prev_len..(prev_len * 2)].iter().copied())
                .map(|(field_index, value_index)| {
                    next_vars[field_index].eq(int_const(value_index))
                }),
        );
        matchers.push(step_var.eq(int_const(action_index + 1)));
        clauses.push(bool_and(&matchers));
    });
    bool_or(&clauses)
}

pub(crate) fn encode_rule_transition<S: Clone + 'static, A: Clone + 'static>(
    schema: &SymbolicStateSchema<S>,
    prev_vars: &[Int],
    step_var: &Int,
    next_vars: &[Int],
    actions: &[A],
    rule: &TransitionRule<S, A>,
) -> Bool {
    let field_indices = all_field_indices(schema);
    let mut domains = field_indices
        .iter()
        .map(|index| schema.fields()[*index].domain_size())
        .collect::<Vec<_>>();
    domains.push(actions.len());
    let mut clauses = Vec::new();
    enumerate_assignments(&domains, |assignment| {
        let prev_state = build_state_from_indices(
            schema,
            &field_indices
                .iter()
                .copied()
                .zip(assignment[..field_indices.len()].iter().copied())
                .collect::<Vec<_>>(),
        );
        let action_index = assignment[field_indices.len()];
        let action = &actions[action_index];
        if !rule.matches(&prev_state, action) {
            return;
        }
        for next_state in rule
            .successors(&prev_state, action)
            .into_iter()
            .map(|successor| successor.into_next())
        {
            let mut matchers = field_indices
                .iter()
                .copied()
                .zip(assignment[..field_indices.len()].iter().copied())
                .map(|(field_index, value_index)| prev_vars[field_index].eq(int_const(value_index)))
                .collect::<Vec<_>>();
            matchers.push(step_var.eq(int_const(action_index + 1)));
            matchers.extend(
                schema
                    .fields()
                    .iter()
                    .enumerate()
                    .map(|(field_index, field)| {
                        next_vars[field_index].eq(int_const(field.read_index(&next_state)))
                    }),
            );
            clauses.push(bool_and(&matchers));
        }
    });
    bool_or(&clauses)
}

pub(crate) fn solver_is_sat(solver: &Solver) -> bool {
    matches!(solver.check(), SatResult::Sat)
}
