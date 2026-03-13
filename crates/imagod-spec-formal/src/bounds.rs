use nirvash_macros::FiniteModelDomain as FormalFiniteModelDomain;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, FormalFiniteModelDomain)]
#[finite_model_domain(range = "0..=MAX")]
pub struct BoundedU8<const MAX: u8>(u8);

impl<const MAX: u8> BoundedU8<MAX> {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn is_max(self) -> bool {
        self.0 == MAX
    }

    pub fn saturating_inc(self) -> Self {
        Self(self.0.saturating_add(1).min(MAX))
    }

    pub fn saturating_dec(self) -> Self {
        Self(self.0.saturating_sub(1))
    }
}

impl<const MAX: u8> nirvash_lower::SymbolicEncoding for BoundedU8<MAX> {
    fn symbolic_sort() -> nirvash::SymbolicSort {
        nirvash::SymbolicSort::finite::<Self>()
    }
}

pub const MAX_MAINTENANCE_TICKS: u8 = 2;
pub const MAX_LASSO_DEPTH: usize = 8;
pub const DOC_CAP_FOCUS: (usize, usize) = (64, 256);
pub const DOC_CAP_SURFACE: (usize, usize) = (128, 512);

pub type MaintenanceTicks = BoundedU8<MAX_MAINTENANCE_TICKS>;

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
    use nirvash_lower::FiniteModelDomain;

    #[test]
    fn bounded_u8_domain_matches_declared_max() {
        let values = MaintenanceTicks::bounded_domain().into_vec();
        assert_eq!(values.len(), usize::from(MAX_MAINTENANCE_TICKS) + 1);
        assert_eq!(
            values.last().map(|value| value.get()),
            Some(MAX_MAINTENANCE_TICKS)
        );
    }

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
