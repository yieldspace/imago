use crate::StepExpr;

#[derive(Debug, Clone)]
pub enum Fairness<S, A> {
    Weak(StepExpr<S, A>),
    Strong(StepExpr<S, A>),
}

impl<S: 'static, A: 'static> Fairness<S, A> {
    pub const fn weak(predicate: StepExpr<S, A>) -> Self {
        Self::Weak(predicate)
    }

    pub const fn strong(predicate: StepExpr<S, A>) -> Self {
        Self::Strong(predicate)
    }

    pub fn predicate(&self) -> &StepExpr<S, A> {
        match self {
            Self::Weak(predicate) | Self::Strong(predicate) => predicate,
        }
    }

    pub fn name(&self) -> &'static str {
        self.predicate().name()
    }

    pub fn is_ast_native(&self) -> bool {
        self.predicate().is_ast_native()
    }

    pub fn first_unencodable_symbolic_node(&self) -> Option<&'static str> {
        self.predicate().first_unencodable_symbolic_node()
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        self.predicate().symbolic_state_paths()
    }
}
