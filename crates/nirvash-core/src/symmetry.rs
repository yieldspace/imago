use std::fmt;

pub struct SymmetryReducer<S> {
    name: &'static str,
    canonicalize: fn(&S) -> S,
}

impl<S> SymmetryReducer<S> {
    pub const fn new(name: &'static str, canonicalize: fn(&S) -> S) -> Self {
        Self { name, canonicalize }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn canonicalize(&self, state: &S) -> S {
        (self.canonicalize)(state)
    }
}

impl<S> Clone for SymmetryReducer<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for SymmetryReducer<S> {}

impl<S> fmt::Debug for SymmetryReducer<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SymmetryReducer")
            .field("name", &self.name)
            .finish()
    }
}
