use std::fmt;

pub struct PartialOrderReducer<S, A> {
    name: &'static str,
    allow_action: fn(&S, &A) -> bool,
}

impl<S, A> PartialOrderReducer<S, A> {
    pub const fn new(name: &'static str, allow_action: fn(&S, &A) -> bool) -> Self {
        Self { name, allow_action }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn allow_action(&self, state: &S, action: &A) -> bool {
        (self.allow_action)(state, action)
    }
}

impl<S, A> Clone for PartialOrderReducer<S, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, A> Copy for PartialOrderReducer<S, A> {}

impl<S, A> fmt::Debug for PartialOrderReducer<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartialOrderReducer")
            .field("name", &self.name)
            .finish()
    }
}
