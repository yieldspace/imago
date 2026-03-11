use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantifierKind {
    ForAll,
    Exists,
}

#[derive(Debug, Clone)]
pub enum StateExprAst<S, T> {
    Literal(T),
    FieldRead {
        path: &'static str,
        read: fn(&S) -> T,
    },
    PureCall {
        name: &'static str,
        eval: fn(&S) -> T,
    },
}

impl<S, T> StateExprAst<S, T>
where
    T: Clone,
{
    fn eval(&self, state: &S) -> T {
        match self {
            Self::Literal(value) => value.clone(),
            Self::FieldRead { read, .. } => read(state),
            Self::PureCall { eval, .. } => eval(state),
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum StateExprBody<S, T> {
    RustFn(fn(&S) -> T),
    Ast(StateExprAst<S, T>),
}

#[derive(Clone)]
pub struct StateExpr<S, T> {
    name: &'static str,
    body: StateExprBody<S, T>,
}

impl<S, T> StateExpr<S, T>
where
    T: Clone,
{
    #[allow(dead_code)]
    pub(crate) const fn new(name: &'static str, eval: fn(&S) -> T) -> Self {
        Self {
            name,
            body: StateExprBody::RustFn(eval),
        }
    }

    pub fn literal(name: &'static str, value: T) -> Self {
        Self {
            name,
            body: StateExprBody::Ast(StateExprAst::Literal(value)),
        }
    }

    pub const fn field(name: &'static str, path: &'static str, read: fn(&S) -> T) -> Self {
        Self {
            name,
            body: StateExprBody::Ast(StateExprAst::FieldRead { path, read }),
        }
    }

    pub const fn pure_call(name: &'static str, eval: fn(&S) -> T) -> Self {
        Self {
            name,
            body: StateExprBody::Ast(StateExprAst::PureCall { name, eval }),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast(&self) -> Option<&StateExprAst<S, T>> {
        match &self.body {
            StateExprBody::RustFn(_) => None,
            StateExprBody::Ast(ast) => Some(ast),
        }
    }

    pub fn is_ast_native(&self) -> bool {
        matches!(self.body, StateExprBody::Ast(_))
    }

    pub fn eval(&self, state: &S) -> T {
        match &self.body {
            StateExprBody::RustFn(eval) => eval(state),
            StateExprBody::Ast(ast) => ast.eval(state),
        }
    }
}

#[allow(dead_code)]
pub(crate) const fn legacy_state_expr<S, T>(
    name: &'static str,
    eval: fn(&S) -> T,
) -> StateExpr<S, T> {
    StateExpr {
        name,
        body: StateExprBody::RustFn(eval),
    }
}

impl<S, T> fmt::Debug for StateExpr<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.body {
            StateExprBody::RustFn(_) => "RustFn",
            StateExprBody::Ast(_) => "Ast",
        };
        f.debug_struct("StateExpr")
            .field("name", &self.name)
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Clone)]
pub struct StateComparison<S> {
    lhs: &'static str,
    rhs: &'static str,
    eval: Arc<dyn Fn(&S) -> bool + 'static>,
}

impl<S> StateComparison<S> {
    pub fn new(lhs: &'static str, rhs: &'static str, eval: impl Fn(&S) -> bool + 'static) -> Self {
        Self {
            lhs,
            rhs,
            eval: Arc::new(eval),
        }
    }

    pub const fn lhs(&self) -> &'static str {
        self.lhs
    }

    pub const fn rhs(&self) -> &'static str {
        self.rhs
    }

    fn eval(&self, state: &S) -> bool {
        (self.eval)(state)
    }
}

impl<S> fmt::Debug for StateComparison<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateComparison")
            .field("lhs", &self.lhs)
            .field("rhs", &self.rhs)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct StateMatch<S> {
    value: &'static str,
    pattern: &'static str,
    eval: fn(&S) -> bool,
}

impl<S> StateMatch<S> {
    pub const fn new(value: &'static str, pattern: &'static str, eval: fn(&S) -> bool) -> Self {
        Self {
            value,
            pattern,
            eval,
        }
    }

    pub const fn value(&self) -> &'static str {
        self.value
    }

    pub const fn pattern(&self) -> &'static str {
        self.pattern
    }

    fn eval(&self, state: &S) -> bool {
        (self.eval)(state)
    }
}

#[derive(Debug, Clone)]
pub struct StateBoolLeaf<S> {
    label: &'static str,
    eval: fn(&S) -> bool,
}

impl<S> StateBoolLeaf<S> {
    pub const fn new(label: &'static str, eval: fn(&S) -> bool) -> Self {
        Self { label, eval }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    fn eval(&self, state: &S) -> bool {
        (self.eval)(state)
    }
}

#[derive(Debug, Clone)]
pub struct StateQuantifier<S> {
    kind: QuantifierKind,
    domain: &'static str,
    body: &'static str,
    eval: fn(&S) -> bool,
}

impl<S> StateQuantifier<S> {
    pub const fn new(
        kind: QuantifierKind,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            kind,
            domain,
            body,
            eval,
        }
    }

    pub const fn kind(&self) -> QuantifierKind {
        self.kind
    }

    pub const fn domain(&self) -> &'static str {
        self.domain
    }

    pub const fn body(&self) -> &'static str {
        self.body
    }

    fn eval(&self, state: &S) -> bool {
        (self.eval)(state)
    }
}

#[derive(Debug, Clone)]
pub enum BoolExprAst<S> {
    Literal(bool),
    FieldRead(StateBoolLeaf<S>),
    PureCall(StateBoolLeaf<S>),
    Eq(StateComparison<S>),
    Ne(StateComparison<S>),
    Lt(StateComparison<S>),
    Match(StateMatch<S>),
    ForAll(StateQuantifier<S>),
    Exists(StateQuantifier<S>),
    Not(Box<BoolExpr<S>>),
    And(Vec<BoolExpr<S>>),
    Or(Vec<BoolExpr<S>>),
}

impl<S: 'static> BoolExprAst<S> {
    fn eval(&self, state: &S) -> bool {
        match self {
            Self::Literal(value) => *value,
            Self::FieldRead(field) | Self::PureCall(field) => field.eval(state),
            Self::Eq(compare) | Self::Ne(compare) | Self::Lt(compare) => compare.eval(state),
            Self::Match(matcher) => matcher.eval(state),
            Self::ForAll(quantifier) | Self::Exists(quantifier) => quantifier.eval(state),
            Self::Not(inner) => !inner.eval(state),
            Self::And(parts) => parts.iter().all(|part| part.eval(state)),
            Self::Or(parts) => parts.iter().any(|part| part.eval(state)),
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum BoolExprBody<S> {
    RustFn(fn(&S) -> bool),
    Ast(BoolExprAst<S>),
}

#[derive(Clone)]
pub struct BoolExpr<S> {
    name: &'static str,
    body: BoolExprBody<S>,
}

impl<S: 'static> BoolExpr<S> {
    #[allow(dead_code)]
    pub(crate) const fn new(name: &'static str, test: fn(&S) -> bool) -> Self {
        legacy_bool_expr(name, test)
    }

    pub const fn literal(name: &'static str, value: bool) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Literal(value)),
        }
    }

    pub const fn field(name: &'static str, path: &'static str, read: fn(&S) -> bool) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::FieldRead(StateBoolLeaf::new(path, read))),
        }
    }

    pub const fn pure_call(name: &'static str, eval: fn(&S) -> bool) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::PureCall(StateBoolLeaf::new(name, eval))),
        }
    }

    pub fn eq<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Eq(StateComparison::new(
                lhs,
                rhs,
                move |state| lhs_eval(state) == rhs_eval(state),
            ))),
        }
    }

    pub fn ne<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Ne(StateComparison::new(
                lhs,
                rhs,
                move |state| lhs_eval(state) != rhs_eval(state),
            ))),
        }
    }

    pub fn lt<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        T: PartialOrd + 'static,
    {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Lt(StateComparison::new(
                lhs,
                rhs,
                move |state| lhs_eval(state) < rhs_eval(state),
            ))),
        }
    }

    pub const fn matches_variant(
        name: &'static str,
        value: &'static str,
        pattern: &'static str,
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Match(StateMatch::new(value, pattern, eval))),
        }
    }

    pub fn not(name: &'static str, inner: Self) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Not(Box::new(inner))),
        }
    }

    pub fn and(name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::And(parts)),
        }
    }

    pub fn or(name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Or(parts)),
        }
    }

    pub const fn forall(
        name: &'static str,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::ForAll(StateQuantifier::new(
                QuantifierKind::ForAll,
                domain,
                body,
                eval,
            ))),
        }
    }

    pub const fn exists(
        name: &'static str,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Exists(StateQuantifier::new(
                QuantifierKind::Exists,
                domain,
                body,
                eval,
            ))),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast(&self) -> Option<&BoolExprAst<S>> {
        match &self.body {
            BoolExprBody::RustFn(_) => None,
            BoolExprBody::Ast(ast) => Some(ast),
        }
    }

    pub fn is_ast_native(&self) -> bool {
        matches!(self.body, BoolExprBody::Ast(_))
    }

    pub fn eval(&self, state: &S) -> bool {
        match &self.body {
            BoolExprBody::RustFn(test) => test(state),
            BoolExprBody::Ast(ast) => ast.eval(state),
        }
    }
}

#[allow(dead_code)]
pub(crate) const fn legacy_bool_expr<S>(name: &'static str, test: fn(&S) -> bool) -> BoolExpr<S> {
    BoolExpr {
        name,
        body: BoolExprBody::RustFn(test),
    }
}

impl<S> fmt::Debug for BoolExpr<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.body {
            BoolExprBody::RustFn(_) => "RustFn",
            BoolExprBody::Ast(_) => "Ast",
        };
        f.debug_struct("BoolExpr")
            .field("name", &self.name)
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Clone)]
pub struct StepComparison<S, A> {
    lhs: &'static str,
    rhs: &'static str,
    eval: Arc<dyn Fn(&S, &A, &S) -> bool + 'static>,
}

impl<S, A> StepComparison<S, A> {
    pub fn new(
        lhs: &'static str,
        rhs: &'static str,
        eval: impl Fn(&S, &A, &S) -> bool + 'static,
    ) -> Self {
        Self {
            lhs,
            rhs,
            eval: Arc::new(eval),
        }
    }

    pub const fn lhs(&self) -> &'static str {
        self.lhs
    }

    pub const fn rhs(&self) -> &'static str {
        self.rhs
    }

    fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.eval)(prev, action, next)
    }
}

impl<S, A> fmt::Debug for StepComparison<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StepComparison")
            .field("lhs", &self.lhs)
            .field("rhs", &self.rhs)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct StepMatch<S, A> {
    value: &'static str,
    pattern: &'static str,
    eval: fn(&S, &A, &S) -> bool,
}

impl<S, A> StepMatch<S, A> {
    pub const fn new(
        value: &'static str,
        pattern: &'static str,
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            value,
            pattern,
            eval,
        }
    }

    pub const fn value(&self) -> &'static str {
        self.value
    }

    pub const fn pattern(&self) -> &'static str {
        self.pattern
    }

    fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.eval)(prev, action, next)
    }
}

#[derive(Debug, Clone)]
pub struct StepBoolLeaf<S, A> {
    label: &'static str,
    eval: fn(&S, &A, &S) -> bool,
}

impl<S, A> StepBoolLeaf<S, A> {
    pub const fn new(label: &'static str, eval: fn(&S, &A, &S) -> bool) -> Self {
        Self { label, eval }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.eval)(prev, action, next)
    }
}

#[derive(Debug, Clone)]
pub struct StepQuantifier<S, A> {
    kind: QuantifierKind,
    domain: &'static str,
    body: &'static str,
    eval: fn(&S, &A, &S) -> bool,
}

impl<S, A> StepQuantifier<S, A> {
    pub const fn new(
        kind: QuantifierKind,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            kind,
            domain,
            body,
            eval,
        }
    }

    pub const fn kind(&self) -> QuantifierKind {
        self.kind
    }

    pub const fn domain(&self) -> &'static str {
        self.domain
    }

    pub const fn body(&self) -> &'static str {
        self.body
    }

    fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.eval)(prev, action, next)
    }
}

#[derive(Debug, Clone)]
pub enum StepExprAst<S, A> {
    Literal(bool),
    FieldRead(StepBoolLeaf<S, A>),
    PureCall(StepBoolLeaf<S, A>),
    Eq(StepComparison<S, A>),
    Ne(StepComparison<S, A>),
    Lt(StepComparison<S, A>),
    Match(StepMatch<S, A>),
    ForAll(StepQuantifier<S, A>),
    Exists(StepQuantifier<S, A>),
    Not(Box<StepExpr<S, A>>),
    And(Vec<StepExpr<S, A>>),
    Or(Vec<StepExpr<S, A>>),
}

impl<S: 'static, A: 'static> StepExprAst<S, A> {
    fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        match self {
            Self::Literal(value) => *value,
            Self::FieldRead(field) | Self::PureCall(field) => field.eval(prev, action, next),
            Self::Eq(compare) | Self::Ne(compare) | Self::Lt(compare) => {
                compare.eval(prev, action, next)
            }
            Self::Match(matcher) => matcher.eval(prev, action, next),
            Self::ForAll(quantifier) | Self::Exists(quantifier) => {
                quantifier.eval(prev, action, next)
            }
            Self::Not(inner) => !inner.eval(prev, action, next),
            Self::And(parts) => parts.iter().all(|part| part.eval(prev, action, next)),
            Self::Or(parts) => parts.iter().any(|part| part.eval(prev, action, next)),
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum StepExprBody<S, A> {
    RustFn(fn(&S, &A, &S) -> bool),
    Ast(StepExprAst<S, A>),
}

#[derive(Clone)]
pub struct StepExpr<S, A> {
    name: &'static str,
    body: StepExprBody<S, A>,
}

impl<S: 'static, A: 'static> StepExpr<S, A> {
    #[allow(dead_code)]
    pub(crate) const fn new(name: &'static str, test: fn(&S, &A, &S) -> bool) -> Self {
        legacy_step_expr(name, test)
    }

    pub const fn literal(name: &'static str, value: bool) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Literal(value)),
        }
    }

    pub const fn field(
        name: &'static str,
        path: &'static str,
        read: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::FieldRead(StepBoolLeaf::new(path, read))),
        }
    }

    pub const fn pure_call(name: &'static str, eval: fn(&S, &A, &S) -> bool) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::PureCall(StepBoolLeaf::new(name, eval))),
        }
    }

    pub fn eq<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Eq(StepComparison::new(
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval(prev, action, next) == rhs_eval(prev, action, next)
                },
            ))),
        }
    }

    pub fn ne<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Ne(StepComparison::new(
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval(prev, action, next) != rhs_eval(prev, action, next)
                },
            ))),
        }
    }

    pub fn lt<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        T: PartialOrd + 'static,
    {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Lt(StepComparison::new(
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval(prev, action, next) < rhs_eval(prev, action, next)
                },
            ))),
        }
    }

    pub const fn matches_variant(
        name: &'static str,
        value: &'static str,
        pattern: &'static str,
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Match(StepMatch::new(value, pattern, eval))),
        }
    }

    pub fn not(name: &'static str, inner: Self) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Not(Box::new(inner))),
        }
    }

    pub fn and(name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::And(parts)),
        }
    }

    pub fn or(name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Or(parts)),
        }
    }

    pub const fn forall(
        name: &'static str,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::ForAll(StepQuantifier::new(
                QuantifierKind::ForAll,
                domain,
                body,
                eval,
            ))),
        }
    }

    pub const fn exists(
        name: &'static str,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Exists(StepQuantifier::new(
                QuantifierKind::Exists,
                domain,
                body,
                eval,
            ))),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast(&self) -> Option<&StepExprAst<S, A>> {
        match &self.body {
            StepExprBody::RustFn(_) => None,
            StepExprBody::Ast(ast) => Some(ast),
        }
    }

    pub fn is_ast_native(&self) -> bool {
        matches!(self.body, StepExprBody::Ast(_))
    }

    pub fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        match &self.body {
            StepExprBody::RustFn(test) => test(prev, action, next),
            StepExprBody::Ast(ast) => ast.eval(prev, action, next),
        }
    }
}

#[allow(dead_code)]
pub(crate) const fn legacy_step_expr<S, A>(
    name: &'static str,
    test: fn(&S, &A, &S) -> bool,
) -> StepExpr<S, A> {
    StepExpr {
        name,
        body: StepExprBody::RustFn(test),
    }
}

impl<S, A> fmt::Debug for StepExpr<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.body {
            StepExprBody::RustFn(_) => "RustFn",
            StepExprBody::Ast(_) => "Ast",
        };
        f.debug_struct("StepExpr")
            .field("name", &self.name)
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Clone)]
pub struct GuardComparison<S, A> {
    lhs: &'static str,
    rhs: &'static str,
    eval: Arc<dyn Fn(&S, &A) -> bool + 'static>,
}

impl<S, A> GuardComparison<S, A> {
    pub fn new(
        lhs: &'static str,
        rhs: &'static str,
        eval: impl Fn(&S, &A) -> bool + 'static,
    ) -> Self {
        Self {
            lhs,
            rhs,
            eval: Arc::new(eval),
        }
    }

    pub const fn lhs(&self) -> &'static str {
        self.lhs
    }

    pub const fn rhs(&self) -> &'static str {
        self.rhs
    }

    fn eval(&self, prev: &S, action: &A) -> bool {
        (self.eval)(prev, action)
    }
}

impl<S, A> fmt::Debug for GuardComparison<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardComparison")
            .field("lhs", &self.lhs)
            .field("rhs", &self.rhs)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct GuardMatch<S, A> {
    value: &'static str,
    pattern: &'static str,
    eval: fn(&S, &A) -> bool,
}

impl<S, A> GuardMatch<S, A> {
    pub const fn new(value: &'static str, pattern: &'static str, eval: fn(&S, &A) -> bool) -> Self {
        Self {
            value,
            pattern,
            eval,
        }
    }

    pub const fn value(&self) -> &'static str {
        self.value
    }

    pub const fn pattern(&self) -> &'static str {
        self.pattern
    }

    fn eval(&self, prev: &S, action: &A) -> bool {
        (self.eval)(prev, action)
    }
}

#[derive(Debug, Clone)]
pub struct GuardBoolLeaf<S, A> {
    label: &'static str,
    eval: fn(&S, &A) -> bool,
}

impl<S, A> GuardBoolLeaf<S, A> {
    pub const fn new(label: &'static str, eval: fn(&S, &A) -> bool) -> Self {
        Self { label, eval }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    fn eval(&self, prev: &S, action: &A) -> bool {
        (self.eval)(prev, action)
    }
}

#[derive(Debug, Clone)]
pub struct GuardQuantifier<S, A> {
    kind: QuantifierKind,
    domain: &'static str,
    body: &'static str,
    eval: fn(&S, &A) -> bool,
}

impl<S, A> GuardQuantifier<S, A> {
    pub const fn new(
        kind: QuantifierKind,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            kind,
            domain,
            body,
            eval,
        }
    }

    pub const fn kind(&self) -> QuantifierKind {
        self.kind
    }

    pub const fn domain(&self) -> &'static str {
        self.domain
    }

    pub const fn body(&self) -> &'static str {
        self.body
    }

    fn eval(&self, prev: &S, action: &A) -> bool {
        (self.eval)(prev, action)
    }
}

#[derive(Debug, Clone)]
pub enum GuardAst<S, A> {
    Literal(bool),
    FieldRead(GuardBoolLeaf<S, A>),
    PureCall(GuardBoolLeaf<S, A>),
    Eq(GuardComparison<S, A>),
    Ne(GuardComparison<S, A>),
    Lt(GuardComparison<S, A>),
    Match(GuardMatch<S, A>),
    ForAll(GuardQuantifier<S, A>),
    Exists(GuardQuantifier<S, A>),
    Not(Box<GuardExpr<S, A>>),
    And(Vec<GuardExpr<S, A>>),
    Or(Vec<GuardExpr<S, A>>),
}

impl<S: 'static, A: 'static> GuardAst<S, A> {
    fn eval(&self, prev: &S, action: &A) -> bool {
        match self {
            Self::Literal(value) => *value,
            Self::FieldRead(field) | Self::PureCall(field) => field.eval(prev, action),
            Self::Eq(compare) | Self::Ne(compare) | Self::Lt(compare) => compare.eval(prev, action),
            Self::Match(matcher) => matcher.eval(prev, action),
            Self::ForAll(quantifier) | Self::Exists(quantifier) => quantifier.eval(prev, action),
            Self::Not(inner) => !inner.eval(prev, action),
            Self::And(parts) => parts.iter().all(|part| part.eval(prev, action)),
            Self::Or(parts) => parts.iter().any(|part| part.eval(prev, action)),
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum GuardExprBody<S, A> {
    RustFn(fn(&S, &A) -> bool),
    Ast(GuardAst<S, A>),
}

#[derive(Clone)]
pub struct GuardExpr<S, A> {
    name: &'static str,
    body: GuardExprBody<S, A>,
}

impl<S: 'static, A: 'static> GuardExpr<S, A> {
    #[allow(dead_code)]
    pub(crate) const fn new(name: &'static str, eval: fn(&S, &A) -> bool) -> Self {
        legacy_guard_expr(name, eval)
    }

    pub const fn literal(name: &'static str, value: bool) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Literal(value)),
        }
    }

    pub const fn field(name: &'static str, path: &'static str, read: fn(&S, &A) -> bool) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::FieldRead(GuardBoolLeaf::new(path, read))),
        }
    }

    pub const fn pure_call(name: &'static str, eval: fn(&S, &A) -> bool) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::PureCall(GuardBoolLeaf::new(name, eval))),
        }
    }

    pub fn eq<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Eq(GuardComparison::new(
                lhs,
                rhs,
                move |prev, action| lhs_eval(prev, action) == rhs_eval(prev, action),
            ))),
        }
    }

    pub fn ne<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        T: PartialEq + 'static,
    {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Ne(GuardComparison::new(
                lhs,
                rhs,
                move |prev, action| lhs_eval(prev, action) != rhs_eval(prev, action),
            ))),
        }
    }

    pub fn lt<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        T: PartialOrd + 'static,
    {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Lt(GuardComparison::new(
                lhs,
                rhs,
                move |prev, action| lhs_eval(prev, action) < rhs_eval(prev, action),
            ))),
        }
    }

    pub const fn matches_variant(
        name: &'static str,
        value: &'static str,
        pattern: &'static str,
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Match(GuardMatch::new(value, pattern, eval))),
        }
    }

    pub fn not(name: &'static str, inner: Self) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Not(Box::new(inner))),
        }
    }

    pub fn and(name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::And(parts)),
        }
    }

    pub fn or(name: &'static str, parts: Vec<Self>) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Or(parts)),
        }
    }

    pub const fn forall(
        name: &'static str,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::ForAll(GuardQuantifier::new(
                QuantifierKind::ForAll,
                domain,
                body,
                eval,
            ))),
        }
    }

    pub const fn exists(
        name: &'static str,
        domain: &'static str,
        body: &'static str,
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Exists(GuardQuantifier::new(
                QuantifierKind::Exists,
                domain,
                body,
                eval,
            ))),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast(&self) -> Option<&GuardAst<S, A>> {
        match &self.body {
            GuardExprBody::RustFn(_) => None,
            GuardExprBody::Ast(ast) => Some(ast),
        }
    }

    pub fn is_ast_native(&self) -> bool {
        matches!(self.body, GuardExprBody::Ast(_))
    }

    pub fn eval(&self, prev: &S, action: &A) -> bool {
        match &self.body {
            GuardExprBody::RustFn(eval) => eval(prev, action),
            GuardExprBody::Ast(ast) => ast.eval(prev, action),
        }
    }
}

#[allow(dead_code)]
pub(crate) const fn legacy_guard_expr<S, A>(
    name: &'static str,
    eval: fn(&S, &A) -> bool,
) -> GuardExpr<S, A> {
    GuardExpr {
        name,
        body: GuardExprBody::RustFn(eval),
    }
}

impl<S, A> fmt::Debug for GuardExpr<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.body {
            GuardExprBody::RustFn(_) => "RustFn",
            GuardExprBody::Ast(_) => "Ast",
        };
        f.debug_struct("GuardExpr")
            .field("name", &self.name)
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum UpdateOp<S, A> {
    Assign {
        target: &'static str,
        value: &'static str,
        apply: fn(&S, &mut S, &A),
    },
    SetInsert {
        target: &'static str,
        item: &'static str,
        apply: fn(&S, &mut S, &A),
    },
    SetRemove {
        target: &'static str,
        item: &'static str,
        apply: fn(&S, &mut S, &A),
    },
    Effect {
        name: &'static str,
        apply: fn(&S, &mut S, &A),
    },
}

impl<S, A> UpdateOp<S, A> {
    pub const fn assign(
        target: &'static str,
        value: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::Assign {
            target,
            value,
            apply,
        }
    }

    pub const fn set_insert(
        target: &'static str,
        item: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::SetInsert {
            target,
            item,
            apply,
        }
    }

    pub const fn set_remove(
        target: &'static str,
        item: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::SetRemove {
            target,
            item,
            apply,
        }
    }

    pub const fn effect(name: &'static str, apply: fn(&S, &mut S, &A)) -> Self {
        Self::Effect { name, apply }
    }

    fn apply(&self, prev: &S, state: &mut S, action: &A) {
        match self {
            Self::Assign { apply, .. }
            | Self::SetInsert { apply, .. }
            | Self::SetRemove { apply, .. }
            | Self::Effect { apply, .. } => apply(prev, state, action),
        }
    }
}

#[derive(Debug, Clone)]
pub enum UpdateAst<S, A> {
    Sequence(Vec<UpdateOp<S, A>>),
}

impl<S, A> UpdateAst<S, A> {
    fn apply(&self, prev: &S, state: &mut S, action: &A) {
        match self {
            Self::Sequence(ops) => {
                for op in ops {
                    op.apply(prev, state, action);
                }
            }
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum UpdateProgramBody<S, A> {
    RustFn(fn(&S, &A) -> S),
    Ast(UpdateAst<S, A>),
}

#[derive(Clone)]
pub struct UpdateProgram<S, A = ()> {
    name: &'static str,
    body: UpdateProgramBody<S, A>,
}

impl<S, A> UpdateProgram<S, A> {
    #[allow(dead_code)]
    pub(crate) const fn new(name: &'static str, update: fn(&S, &A) -> S) -> Self {
        legacy_update_program(name, update)
    }

    pub fn ast(name: &'static str, ops: Vec<UpdateOp<S, A>>) -> Self {
        Self {
            name,
            body: UpdateProgramBody::Ast(UpdateAst::Sequence(ops)),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast_body(&self) -> Option<&UpdateAst<S, A>> {
        match &self.body {
            UpdateProgramBody::RustFn(_) => None,
            UpdateProgramBody::Ast(ast) => Some(ast),
        }
    }

    pub fn is_ast_native(&self) -> bool {
        matches!(self.body, UpdateProgramBody::Ast(_))
    }

    pub fn apply(&self, state: &S, action: &A) -> S
    where
        S: Clone,
    {
        match &self.body {
            UpdateProgramBody::RustFn(update) => update(state, action),
            UpdateProgramBody::Ast(ast) => {
                let mut next = state.clone();
                ast.apply(state, &mut next, action);
                next
            }
        }
    }
}

#[allow(dead_code)]
pub(crate) const fn legacy_update_program<S, A>(
    name: &'static str,
    update: fn(&S, &A) -> S,
) -> UpdateProgram<S, A> {
    UpdateProgram {
        name,
        body: UpdateProgramBody::RustFn(update),
    }
}

impl<S, A> fmt::Debug for UpdateProgram<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.body {
            UpdateProgramBody::RustFn(_) => "RustFn",
            UpdateProgramBody::Ast(_) => "Ast",
        };
        f.debug_struct("UpdateProgram")
            .field("name", &self.name)
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionProgramError {
    AmbiguousRuleMatch {
        program: &'static str,
        rule_names: Vec<&'static str>,
    },
}

#[derive(Clone)]
pub enum TransitionRuleBody<S, A> {
    Guarded {
        guard: GuardExpr<S, A>,
        update: UpdateProgram<S, A>,
    },
}

#[derive(Clone)]
pub struct TransitionRule<S, A> {
    name: &'static str,
    body: TransitionRuleBody<S, A>,
}

impl<S: 'static, A: 'static> TransitionRule<S, A> {
    #[allow(dead_code)]
    pub(crate) const fn new(
        name: &'static str,
        guard: fn(&S, &A) -> bool,
        update: UpdateProgram<S, A>,
    ) -> Self {
        legacy_transition_rule(name, guard, update)
    }

    pub const fn ast(
        name: &'static str,
        guard: GuardExpr<S, A>,
        update: UpdateProgram<S, A>,
    ) -> Self {
        Self {
            name,
            body: TransitionRuleBody::Guarded { guard, update },
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn guard_ast(&self) -> Option<&GuardAst<S, A>> {
        match &self.body {
            TransitionRuleBody::Guarded { guard, .. } => guard.ast(),
        }
    }

    pub fn update_ast(&self) -> Option<&UpdateAst<S, A>> {
        match &self.body {
            TransitionRuleBody::Guarded { update, .. } => update.ast_body(),
        }
    }

    pub fn is_ast_native(&self) -> bool {
        matches!(
            &self.body,
            TransitionRuleBody::Guarded { guard, update }
                if guard.is_ast_native() && update.is_ast_native()
        )
    }

    pub fn matches(&self, prev: &S, action: &A) -> bool {
        match &self.body {
            TransitionRuleBody::Guarded { guard, .. } => guard.eval(prev, action),
        }
    }

    pub fn apply(&self, prev: &S, action: &A) -> S
    where
        S: Clone,
    {
        match &self.body {
            TransitionRuleBody::Guarded { update, .. } => update.apply(prev, action),
        }
    }
}

#[allow(dead_code)]
pub(crate) const fn legacy_transition_rule<S, A>(
    name: &'static str,
    guard: fn(&S, &A) -> bool,
    update: UpdateProgram<S, A>,
) -> TransitionRule<S, A> {
    TransitionRule {
        name,
        body: TransitionRuleBody::Guarded {
            guard: legacy_guard_expr(name, guard),
            update,
        },
    }
}

impl<S, A> fmt::Debug for TransitionRule<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransitionRule")
            .field("name", &self.name)
            .field("kind", &"Guarded")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct TransitionProgram<S, A> {
    name: &'static str,
    rules: Vec<TransitionRule<S, A>>,
}

impl<S: 'static, A: 'static> TransitionProgram<S, A> {
    pub fn new(rules: Vec<TransitionRule<S, A>>) -> Self {
        Self::named("transition_program", rules)
    }

    pub fn named(name: &'static str, rules: Vec<TransitionRule<S, A>>) -> Self {
        Self { name, rules }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn rules(&self) -> &[TransitionRule<S, A>] {
        &self.rules
    }

    pub fn is_ast_native(&self) -> bool {
        self.rules.iter().all(TransitionRule::is_ast_native)
    }

    pub fn evaluate(&self, prev: &S, action: &A) -> Result<Option<S>, TransitionProgramError>
    where
        S: Clone,
    {
        let mut matches = self.rules.iter().filter(|rule| rule.matches(prev, action));
        let Some(first) = matches.next() else {
            return Ok(None);
        };

        let mut matched_names = vec![first.name()];
        let next = first.apply(prev, action);
        for rule in matches {
            matched_names.push(rule.name());
        }

        if matched_names.len() > 1 {
            return Err(TransitionProgramError::AmbiguousRuleMatch {
                program: self.name,
                rule_names: matched_names,
            });
        }

        Ok(Some(next))
    }
}

impl<S: 'static, A: 'static> Default for TransitionProgram<S, A> {
    fn default() -> Self {
        Self::named("transition_program", Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoolExpr, GuardAst, GuardExpr, QuantifierKind, StateExpr, StateExprAst, StepExpr,
        TransitionProgram, TransitionProgramError, TransitionRule, UpdateOp, UpdateProgram,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Idle,
        Busy,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Action {
        Start,
        Stop,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Worker {
        ready: bool,
        count: usize,
        state: State,
        next_state: State,
    }

    #[test]
    fn state_expr_exposes_field_ast() {
        let expr = StateExpr::field("ready", "state.ready", |state: &Worker| state.ready);
        match expr.ast() {
            Some(StateExprAst::FieldRead { path, .. }) => assert_eq!(*path, "state.ready"),
            other => panic!("unexpected ast: {other:?}"),
        }
        assert!(expr.eval(&Worker {
            ready: true,
            count: 0,
            state: State::Idle,
            next_state: State::Idle,
        }));
    }

    #[test]
    fn bool_expr_ast_evaluates_combinators_and_matches() {
        let worker = Worker {
            ready: true,
            count: 2,
            state: State::Busy,
            next_state: State::Idle,
        };
        let expr = BoolExpr::and(
            "busy_and_ready",
            vec![
                BoolExpr::field("ready", "state.ready", |state: &Worker| state.ready),
                BoolExpr::matches_variant(
                    "busy",
                    "state.state",
                    "State::Busy",
                    |state: &Worker| matches!(state.state, State::Busy),
                ),
                BoolExpr::not(
                    "not_zero",
                    BoolExpr::eq(
                        "count_zero",
                        "state.count",
                        |state: &Worker| state.count,
                        "0",
                        |_state: &Worker| 0,
                    ),
                ),
            ],
        );

        assert!(expr.eval(&worker));
        assert!(expr.is_ast_native());
    }

    #[test]
    fn bool_expr_quantifiers_use_ast_interpreter() {
        let forall = BoolExpr::forall("all_small", "0..=2", "count <= 2", |state: &Worker| {
            (0..=2).all(|value| state.count >= value)
        });
        let exists = BoolExpr::exists("has_two", "0..=3", "value == count", |state: &Worker| {
            (0..=3).any(|value| value == state.count)
        });
        let worker = Worker {
            ready: true,
            count: 2,
            state: State::Idle,
            next_state: State::Idle,
        };

        assert!(forall.eval(&worker));
        assert!(exists.eval(&worker));
        assert!(matches!(
            forall.ast(),
            Some(super::BoolExprAst::ForAll(quantifier))
                if quantifier.kind() == QuantifierKind::ForAll
        ));
        assert!(matches!(
            exists.ast(),
            Some(super::BoolExprAst::Exists(quantifier))
                if quantifier.kind() == QuantifierKind::Exists
        ));
    }

    #[test]
    fn step_expr_supports_next_state_comparisons() {
        let expr = StepExpr::and(
            "start_to_busy",
            vec![
                StepExpr::matches_variant(
                    "start",
                    "action",
                    "Action::Start",
                    |_prev: &Worker, action: &Action, _next: &Worker| {
                        matches!(action, Action::Start)
                    },
                ),
                StepExpr::eq(
                    "next_busy",
                    "next.state",
                    |_prev: &Worker, _action: &Action, next: &Worker| next.state,
                    "State::Busy",
                    |_prev: &Worker, _action: &Action, _next: &Worker| State::Busy,
                ),
            ],
        );
        let prev = Worker {
            ready: true,
            count: 0,
            state: State::Idle,
            next_state: State::Idle,
        };
        let next = Worker {
            ready: true,
            count: 1,
            state: State::Busy,
            next_state: State::Busy,
        };
        assert!(expr.eval(&prev, &Action::Start, &next));
    }

    #[test]
    fn update_program_applies_structured_ops() {
        let update = UpdateProgram::ast(
            "promote",
            vec![
                UpdateOp::assign(
                    "ready",
                    "true",
                    |_prev: &Worker, state: &mut Worker, _action: &Action| {
                        state.ready = true;
                    },
                ),
                UpdateOp::assign(
                    "count",
                    "state.count + 1",
                    |_prev: &Worker, state: &mut Worker, _action: &Action| {
                        state.count += 1;
                    },
                ),
                UpdateOp::assign(
                    "state",
                    "State::Busy",
                    |_prev: &Worker, state: &mut Worker, _action: &Action| {
                        state.state = State::Busy;
                    },
                ),
            ],
        );
        let initial = Worker {
            ready: false,
            count: 0,
            state: State::Idle,
            next_state: State::Idle,
        };
        let next = update.apply(&initial, &Action::Start);
        assert_eq!(
            next,
            Worker {
                ready: true,
                count: 1,
                state: State::Busy,
                next_state: State::Idle,
            }
        );
        assert!(update.is_ast_native());
    }

    #[test]
    fn transition_program_applies_single_matching_rule() {
        let program = TransitionProgram::named(
            "worker",
            vec![TransitionRule::ast(
                "start",
                GuardExpr::and(
                    "start_guard",
                    vec![
                        GuardExpr::matches_variant(
                            "is_start",
                            "action",
                            "Action::Start",
                            |_state: &State, action: &Action| matches!(action, Action::Start),
                        ),
                        GuardExpr::matches_variant(
                            "is_idle",
                            "prev",
                            "State::Idle",
                            |state: &State, _action: &Action| matches!(state, State::Idle),
                        ),
                    ],
                ),
                UpdateProgram::ast(
                    "to_busy",
                    vec![UpdateOp::assign(
                        "state",
                        "State::Busy",
                        |_prev: &State, state: &mut State, _action: &Action| {
                            *state = State::Busy;
                        },
                    )],
                ),
            )],
        );

        let next = program.evaluate(&State::Idle, &Action::Start).unwrap();
        assert_eq!(next, Some(State::Busy));
        assert_eq!(program.evaluate(&State::Busy, &Action::Stop).unwrap(), None);
        assert!(matches!(
            program.rules()[0].guard_ast(),
            Some(GuardAst::And(parts)) if parts.len() == 2
        ));
    }

    #[test]
    fn transition_program_rejects_ambiguous_rule_match() {
        let program = TransitionProgram::named(
            "ambiguous_worker",
            vec![
                TransitionRule::ast(
                    "start_a",
                    GuardExpr::pure_call("start_a", |state, action| {
                        matches!((state, action), (State::Idle, Action::Start))
                    }),
                    UpdateProgram::ast(
                        "to_busy_a",
                        vec![UpdateOp::assign(
                            "self",
                            "State::Busy",
                            |_prev: &State, state: &mut State, _action: &Action| {
                                *state = State::Busy;
                            },
                        )],
                    ),
                ),
                TransitionRule::ast(
                    "start_b",
                    GuardExpr::pure_call("start_b", |state, action| {
                        matches!((state, action), (State::Idle, Action::Start))
                    }),
                    UpdateProgram::ast(
                        "to_busy_b",
                        vec![UpdateOp::assign(
                            "self",
                            "State::Busy",
                            |_prev: &State, state: &mut State, _action: &Action| {
                                *state = State::Busy;
                            },
                        )],
                    ),
                ),
            ],
        );

        let error = program.evaluate(&State::Idle, &Action::Start).unwrap_err();
        assert_eq!(
            error,
            TransitionProgramError::AmbiguousRuleMatch {
                program: "ambiguous_worker",
                rule_names: vec!["start_a", "start_b"],
            }
        );
    }
}
