use std::fmt;

pub struct ViewProjector<S> {
    name: &'static str,
    project: fn(&S) -> String,
}

impl<S> ViewProjector<S> {
    pub const fn new(name: &'static str, project: fn(&S) -> String) -> Self {
        Self { name, project }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn project(&self, state: &S) -> String {
        (self.project)(state)
    }
}

impl<S> Clone for ViewProjector<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for ViewProjector<S> {}

impl<S> fmt::Debug for ViewProjector<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewProjector")
            .field("name", &self.name)
            .finish()
    }
}
