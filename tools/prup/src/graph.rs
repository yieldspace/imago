use crate::config::PrupConfig;
use std::collections::{BTreeMap, BTreeSet};

pub fn find_line_cycle(config: &PrupConfig) -> Option<Vec<String>> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum State {
        Visiting,
        Visited,
    }

    fn dfs(
        current: &str,
        edges: &BTreeMap<&str, Vec<&str>>,
        states: &mut BTreeMap<String, State>,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        states.insert(current.to_string(), State::Visiting);
        stack.push(current.to_string());

        if let Some(next_nodes) = edges.get(current) {
            for next in next_nodes {
                match states.get(*next).copied() {
                    Some(State::Visiting) => {
                        let start = stack.iter().position(|node| node == next).unwrap_or(0);
                        return Some(stack[start..].to_vec());
                    }
                    Some(State::Visited) => continue,
                    None => {
                        if let Some(cycle) = dfs(next, edges, states, stack) {
                            return Some(cycle);
                        }
                    }
                }
            }
        }

        stack.pop();
        states.insert(current.to_string(), State::Visited);
        None
    }

    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut line_ids = BTreeSet::new();
    for line in &config.lines {
        line_ids.insert(line.id.as_str());
        edges.insert(
            line.id.as_str(),
            line.propagate_to.iter().map(String::as_str).collect(),
        );
    }

    let mut states = BTreeMap::new();
    let mut stack = Vec::new();

    for line_id in line_ids {
        if states.contains_key(line_id) {
            continue;
        }
        if let Some(cycle) = dfs(line_id, &edges, &mut states, &mut stack) {
            return Some(cycle);
        }
    }

    None
}
