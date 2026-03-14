pub const MAX_LASSO_DEPTH: usize = 8;
pub const DOC_CAP_FOCUS: (usize, usize) = (64, 256);
pub const DOC_CAP_SURFACE: (usize, usize) = (128, 512);

pub fn doc_explicit_cap(max_states: usize, max_transitions: usize) -> nirvash::ModelCheckConfig {
    nirvash::ModelCheckConfig {
        backend: Some(nirvash::ModelBackend::Explicit),
        max_states: Some(max_states),
        max_transitions: Some(max_transitions),
        stop_on_first_violation: false,
        ..nirvash::ModelCheckConfig::reachable_graph()
    }
}

pub fn doc_cap_focus() -> nirvash::ModelCheckConfig {
    let (max_states, max_transitions) = DOC_CAP_FOCUS;
    doc_explicit_cap(max_states, max_transitions)
}

pub fn doc_cap_surface() -> nirvash::ModelCheckConfig {
    let (max_states, max_transitions) = DOC_CAP_SURFACE;
    doc_explicit_cap(max_states, max_transitions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_explicit_cap_sets_explicit_reachable_graph_limits() {
        let config = doc_cap_focus();

        assert_eq!(config.backend, Some(nirvash::ModelBackend::Explicit));
        assert_eq!(config.max_states, Some(DOC_CAP_FOCUS.0));
        assert_eq!(config.max_transitions, Some(DOC_CAP_FOCUS.1));
        assert!(!config.stop_on_first_violation);
        assert_eq!(config.exploration, nirvash::ExplorationMode::ReachableGraph);
    }
}
