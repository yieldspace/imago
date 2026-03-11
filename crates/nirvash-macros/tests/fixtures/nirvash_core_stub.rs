pub struct BoolExpr<S> {
    eval: Box<dyn Fn(&S) -> bool>,
}

impl<S: 'static> BoolExpr<S> {
    pub fn literal(_name: &'static str, value: bool) -> Self {
        Self {
            eval: Box::new(move |_| value),
        }
    }

    pub fn field(_name: &'static str, _path: &'static str, read: fn(&S) -> bool) -> Self {
        Self {
            eval: Box::new(move |state| read(state)),
        }
    }

    pub fn pure_call(_name: &'static str, eval: fn(&S) -> bool) -> Self {
        Self {
            eval: Box::new(move |state| eval(state)),
        }
    }

    pub fn eq<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            eval: Box::new(move |state| lhs_eval(state) == rhs_eval(state)),
        }
    }

    pub fn ne<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            eval: Box::new(move |state| lhs_eval(state) != rhs_eval(state)),
        }
    }

    pub fn lt<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        T: PartialOrd + 'static,
    {
        Self {
            eval: Box::new(move |state| lhs_eval(state) < rhs_eval(state)),
        }
    }

    pub fn matches_variant(
        _name: &'static str,
        _value: &'static str,
        _pattern: &'static str,
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            eval: Box::new(move |state| eval(state)),
        }
    }

    pub fn not(_name: &'static str, inner: Self) -> Self {
        Self {
            eval: Box::new(move |state| !inner.eval(state)),
        }
    }

    pub fn and(_name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            eval: Box::new(move |state| parts.iter().all(|part| part.eval(state))),
        }
    }

    pub fn or(_name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            eval: Box::new(move |state| parts.iter().any(|part| part.eval(state))),
        }
    }

    pub fn eval(&self, state: &S) -> bool {
        (self.eval)(state)
    }

    pub fn is_ast_native(&self) -> bool {
        true
    }
}

pub struct StepExpr<S, A> {
    eval: Box<dyn Fn(&S, &A, &S) -> bool>,
}

impl<S: 'static, A: 'static> StepExpr<S, A> {
    pub fn literal(_name: &'static str, value: bool) -> Self {
        Self {
            eval: Box::new(move |_, _, _| value),
        }
    }

    pub fn field(
        _name: &'static str,
        _path: &'static str,
        read: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            eval: Box::new(move |prev, action, next| read(prev, action, next)),
        }
    }

    pub fn pure_call(_name: &'static str, eval: fn(&S, &A, &S) -> bool) -> Self {
        Self {
            eval: Box::new(move |prev, action, next| eval(prev, action, next)),
        }
    }

    pub fn eq<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            eval: Box::new(move |prev, action, next| {
                lhs_eval(prev, action, next) == rhs_eval(prev, action, next)
            }),
        }
    }

    pub fn ne<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            eval: Box::new(move |prev, action, next| {
                lhs_eval(prev, action, next) != rhs_eval(prev, action, next)
            }),
        }
    }

    pub fn lt<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        T: PartialOrd + 'static,
    {
        Self {
            eval: Box::new(move |prev, action, next| {
                lhs_eval(prev, action, next) < rhs_eval(prev, action, next)
            }),
        }
    }

    pub fn matches_variant(
        _name: &'static str,
        _value: &'static str,
        _pattern: &'static str,
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            eval: Box::new(move |prev, action, next| eval(prev, action, next)),
        }
    }

    pub fn not(_name: &'static str, inner: Self) -> Self {
        Self {
            eval: Box::new(move |prev, action, next| !inner.eval(prev, action, next)),
        }
    }

    pub fn and(_name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            eval: Box::new(move |prev, action, next| {
                parts.iter().all(|part| part.eval(prev, action, next))
            }),
        }
    }

    pub fn or(_name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            eval: Box::new(move |prev, action, next| {
                parts.iter().any(|part| part.eval(prev, action, next))
            }),
        }
    }

    pub fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.eval)(prev, action, next)
    }

    pub fn is_ast_native(&self) -> bool {
        true
    }
}

pub struct GuardExpr<S, A> {
    eval: Box<dyn Fn(&S, &A) -> bool>,
}

impl<S: 'static, A: 'static> GuardExpr<S, A> {
    pub fn literal(_name: &'static str, value: bool) -> Self {
        Self {
            eval: Box::new(move |_, _| value),
        }
    }

    pub fn field(_name: &'static str, _path: &'static str, read: fn(&S, &A) -> bool) -> Self {
        Self {
            eval: Box::new(move |prev, action| read(prev, action)),
        }
    }

    pub fn pure_call(_name: &'static str, eval: fn(&S, &A) -> bool) -> Self {
        Self {
            eval: Box::new(move |prev, action| eval(prev, action)),
        }
    }

    pub fn eq<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            eval: Box::new(move |prev, action| lhs_eval(prev, action) == rhs_eval(prev, action)),
        }
    }

    pub fn ne<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            eval: Box::new(move |prev, action| lhs_eval(prev, action) != rhs_eval(prev, action)),
        }
    }

    pub fn lt<T>(
        _name: &'static str,
        _lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        _rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        T: PartialOrd + 'static,
    {
        Self {
            eval: Box::new(move |prev, action| lhs_eval(prev, action) < rhs_eval(prev, action)),
        }
    }

    pub fn matches_variant(
        _name: &'static str,
        _value: &'static str,
        _pattern: &'static str,
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            eval: Box::new(move |prev, action| eval(prev, action)),
        }
    }

    pub fn not(_name: &'static str, inner: Self) -> Self {
        Self {
            eval: Box::new(move |prev, action| !inner.eval(prev, action)),
        }
    }

    pub fn and(_name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            eval: Box::new(move |prev, action| parts.iter().all(|part| part.eval(prev, action))),
        }
    }

    pub fn or(_name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            eval: Box::new(move |prev, action| parts.iter().any(|part| part.eval(prev, action))),
        }
    }

    pub fn eval(&self, prev: &S, action: &A) -> bool {
        (self.eval)(prev, action)
    }
}

pub struct UpdateOp<S, A> {
    apply: fn(&S, &mut S, &A),
}

impl<S, A> UpdateOp<S, A> {
    pub fn assign(
        _target: &'static str,
        _value: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self { apply }
    }

    pub fn set_insert(
        _target: &'static str,
        _item: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self { apply }
    }

    pub fn set_remove(
        _target: &'static str,
        _item: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self { apply }
    }

    pub fn effect(_name: &'static str, apply: fn(&S, &mut S, &A)) -> Self {
        Self { apply }
    }
}

pub struct UpdateProgram<S, A> {
    ops: Vec<UpdateOp<S, A>>,
}

impl<S, A> UpdateProgram<S, A> {
    pub fn ast(_name: &'static str, ops: Vec<UpdateOp<S, A>>) -> Self {
        Self { ops }
    }

    pub fn apply(&self, prev: &S, action: &A) -> S
    where
        S: Clone,
    {
        let mut next = prev.clone();
        for op in &self.ops {
            (op.apply)(prev, &mut next, action);
        }
        next
    }

    pub fn ast_body(&self) -> Option<()> {
        Some(())
    }
}

pub struct TransitionRule<S, A> {
    guard: GuardExpr<S, A>,
    update: UpdateProgram<S, A>,
}

impl<S: 'static, A: 'static> TransitionRule<S, A> {
    pub fn ast(
        _name: &'static str,
        guard: GuardExpr<S, A>,
        update: UpdateProgram<S, A>,
    ) -> Self {
        Self { guard, update }
    }

    pub fn matches(&self, prev: &S, action: &A) -> bool {
        self.guard.eval(prev, action)
    }

    pub fn apply(&self, prev: &S, action: &A) -> S
    where
        S: Clone,
    {
        self.update.apply(prev, action)
    }

    pub fn is_ast_native(&self) -> bool {
        true
    }

    pub fn guard_ast(&self) -> Option<()> {
        Some(())
    }

    pub fn update_ast(&self) -> Option<()> {
        self.update.ast_body()
    }
}

pub struct TransitionProgram<S, A> {
    rules: Vec<TransitionRule<S, A>>,
}

impl<S: 'static, A: 'static> TransitionProgram<S, A> {
    pub fn new(rules: Vec<TransitionRule<S, A>>) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &[TransitionRule<S, A>] {
        &self.rules
    }

    pub fn evaluate(&self, prev: &S, action: &A) -> Result<Option<S>, &'static str>
    where
        S: Clone,
    {
        let mut iter = self.rules.iter().filter(|rule| rule.matches(prev, action));
        let Some(rule) = iter.next() else {
            return Ok(None);
        };
        if iter.next().is_some() {
            return Err("ambiguous");
        }
        Ok(Some(rule.apply(prev, action)))
    }
}
