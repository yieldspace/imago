use std::fmt;

pub struct StatePredicate<S> {
    name: &'static str,
    test: fn(&S) -> bool,
}

impl<S> StatePredicate<S> {
    pub const fn new(name: &'static str, test: fn(&S) -> bool) -> Self {
        Self { name, test }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn eval(&self, state: &S) -> bool {
        (self.test)(state)
    }
}

impl<S> Clone for StatePredicate<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for StatePredicate<S> {}

impl<S> fmt::Debug for StatePredicate<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatePredicate")
            .field("name", &self.name)
            .finish()
    }
}

pub struct StateConstraint<S> {
    name: &'static str,
    test: fn(&S) -> bool,
}

impl<S> StateConstraint<S> {
    pub const fn new(name: &'static str, test: fn(&S) -> bool) -> Self {
        Self { name, test }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn eval(&self, state: &S) -> bool {
        (self.test)(state)
    }
}

impl<S> Clone for StateConstraint<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for StateConstraint<S> {}

impl<S> fmt::Debug for StateConstraint<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateConstraint")
            .field("name", &self.name)
            .finish()
    }
}

pub struct StepPredicate<S, A> {
    name: &'static str,
    test: fn(&S, &A, &S) -> bool,
}

impl<S, A> StepPredicate<S, A> {
    pub const fn new(name: &'static str, test: fn(&S, &A, &S) -> bool) -> Self {
        Self { name, test }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.test)(prev, action, next)
    }
}

impl<S, A> Clone for StepPredicate<S, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, A> Copy for StepPredicate<S, A> {}

impl<S, A> fmt::Debug for StepPredicate<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StepPredicate")
            .field("name", &self.name)
            .finish()
    }
}

pub struct ActionConstraint<S, A> {
    name: &'static str,
    test: fn(&S, &A, &S) -> bool,
}

impl<S, A> ActionConstraint<S, A> {
    pub const fn new(name: &'static str, test: fn(&S, &A, &S) -> bool) -> Self {
        Self { name, test }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.test)(prev, action, next)
    }
}

impl<S, A> Clone for ActionConstraint<S, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, A> Copy for ActionConstraint<S, A> {}

impl<S, A> fmt::Debug for ActionConstraint<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionConstraint")
            .field("name", &self.name)
            .finish()
    }
}
