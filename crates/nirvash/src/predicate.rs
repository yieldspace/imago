use std::collections::BTreeSet;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::{
    normalize_symbolic_state_path,
    registry::{has_registered_symbolic_effect, has_registered_symbolic_pure_helper},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantifierKind {
    ForAll,
    Exists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolicRegistration {
    Builtin,
    Registered(&'static str),
    Unregistered(&'static str),
}

impl SymbolicRegistration {
    pub const fn builtin() -> Self {
        Self::Builtin
    }

    pub const fn registered(key: &'static str) -> Self {
        Self::Registered(key)
    }

    pub const fn unregistered(key: &'static str) -> Self {
        Self::Unregistered(key)
    }

    pub const fn is_symbolically_encodable(self) -> bool {
        !matches!(self, Self::Unregistered(_))
    }

    pub const fn symbolic_key(self) -> Option<&'static str> {
        match self {
            Self::Builtin => None,
            Self::Registered(key) | Self::Unregistered(key) => Some(key),
        }
    }

    pub const fn first_unencodable(self) -> Option<&'static str> {
        match self {
            Self::Builtin | Self::Registered(_) => None,
            Self::Unregistered(key) => Some(key),
        }
    }

    fn collect_key(self, keys: &mut BTreeSet<&'static str>) {
        if let Some(key) = self.symbolic_key() {
            keys.insert(key);
        }
    }

    fn collect_unregistered_key(self, keys: &mut BTreeSet<&'static str>) {
        if let Self::Unregistered(key) = self {
            keys.insert(key);
        }
    }
}

fn symbolic_pure_registration(key: &'static str) -> SymbolicRegistration {
    if has_registered_symbolic_pure_helper(key) {
        SymbolicRegistration::Registered(key)
    } else {
        SymbolicRegistration::Unregistered(key)
    }
}

fn symbolic_effect_registration(key: &'static str) -> SymbolicRegistration {
    if has_registered_symbolic_effect(key) {
        SymbolicRegistration::Registered(key)
    } else {
        SymbolicRegistration::Unregistered(key)
    }
}

fn collect_symbolic_state_path(paths: &mut BTreeSet<&'static str>, path: &'static str) {
    if let Some(normalized) = normalize_symbolic_state_path(path) {
        if !normalized.is_empty() {
            paths.insert(normalized);
        }
    }
}

fn collect_symbolic_state_paths_from_hints(
    paths: &mut BTreeSet<&'static str>,
    read_paths: &'static [&'static str],
) {
    for path in read_paths {
        collect_symbolic_state_path(paths, path);
    }
}

#[derive(Debug, Clone)]
pub enum ErasedStateExprAst<S> {
    Opaque {
        repr: &'static str,
    },
    Literal {
        repr: &'static str,
    },
    FieldRead {
        path: &'static str,
    },
    PureCall {
        name: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
    },
    Add {
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    IfElse {
        condition: Box<BoolExpr<S>>,
        then_branch: Box<Self>,
        else_branch: Box<Self>,
    },
}

impl<S: 'static> ErasedStateExprAst<S> {
    pub(crate) fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Opaque { repr } => Some(repr),
            Self::Literal { .. } | Self::FieldRead { .. } => None,
            Self::PureCall { symbolic, .. } => symbolic.first_unencodable(),
            Self::Add { lhs, rhs } => lhs.first_unencodable().or_else(|| rhs.first_unencodable()),
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => condition
                .first_unencodable_symbolic_node()
                .or_else(|| then_branch.first_unencodable())
                .or_else(|| else_branch.first_unencodable()),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } | Self::FieldRead { .. } => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_pure_keys(keys);
                rhs.collect_symbolic_pure_keys(keys);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_symbolic_pure_keys(keys);
                then_branch.collect_symbolic_pure_keys(keys);
                else_branch.collect_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } | Self::FieldRead { .. } => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_unregistered_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_unregistered_symbolic_pure_keys(keys);
                rhs.collect_unregistered_symbolic_pure_keys(keys);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_unregistered_symbolic_pure_keys(keys);
                then_branch.collect_unregistered_symbolic_pure_keys(keys);
                else_branch.collect_unregistered_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } => {}
            Self::FieldRead { path } => collect_symbolic_state_path(paths, path),
            Self::PureCall { read_paths, .. } => {
                collect_symbolic_state_paths_from_hints(paths, read_paths);
            }
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_state_paths(paths);
                rhs.collect_symbolic_state_paths(paths);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_symbolic_state_paths(paths);
                then_branch.collect_symbolic_state_paths(paths);
                else_branch.collect_symbolic_state_paths(paths);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum StateExprAst<S, T> {
    Opaque {
        repr: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    Literal {
        repr: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    FieldRead {
        path: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    PureCall {
        name: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
        _marker: PhantomData<fn() -> T>,
    },
    Add {
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    IfElse {
        condition: Box<BoolExpr<S>>,
        then_branch: Box<Self>,
        else_branch: Box<Self>,
    },
}

impl<S: Clone, T> StateExprAst<S, T> {
    fn erase(&self) -> ErasedStateExprAst<S> {
        match self {
            Self::Opaque { repr, .. } => ErasedStateExprAst::Opaque { repr },
            Self::Literal { repr, .. } => ErasedStateExprAst::Literal { repr },
            Self::FieldRead { path, .. } => ErasedStateExprAst::FieldRead { path },
            Self::PureCall {
                name,
                symbolic,
                read_paths,
                ..
            } => ErasedStateExprAst::PureCall {
                name,
                symbolic: *symbolic,
                read_paths,
            },
            Self::Add { lhs, rhs } => ErasedStateExprAst::Add {
                lhs: Box::new(lhs.erase()),
                rhs: Box::new(rhs.erase()),
            },
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => ErasedStateExprAst::IfElse {
                condition: condition.clone(),
                then_branch: Box::new(then_branch.erase()),
                else_branch: Box::new(else_branch.erase()),
            },
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum StateExprBody<S, T> {
    RustFn(fn(&S) -> T),
    Ast {
        ast: StateExprAst<S, T>,
        eval: Arc<dyn Fn(&S) -> T + 'static>,
    },
}

#[derive(Clone)]
pub struct StateExpr<S, T> {
    name: &'static str,
    body: StateExprBody<S, T>,
}

impl<S: 'static, T> StateExpr<S, T>
where
    T: Clone + 'static,
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
            body: StateExprBody::Ast {
                ast: StateExprAst::Literal {
                    repr: name,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |_| value.clone()),
            },
        }
    }

    pub fn literal_with_repr(name: &'static str, repr: &'static str, value: T) -> Self {
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::Literal {
                    repr,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |_| value.clone()),
            },
        }
    }

    pub fn opaque(name: &'static str, repr: &'static str, eval: fn(&S) -> T) -> Self {
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::Opaque {
                    repr,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |state| eval(state)),
            },
        }
    }

    pub fn field(name: &'static str, path: &'static str, read: fn(&S) -> T) -> Self {
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::FieldRead {
                    path,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |state| read(state)),
            },
        }
    }

    pub fn pure_call(name: &'static str, eval: fn(&S) -> T) -> Self {
        Self::pure_call_with_paths(name, &[], eval)
    }

    pub fn pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S) -> T,
    ) -> Self {
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::PureCall {
                    name,
                    symbolic: SymbolicRegistration::Unregistered(name),
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |state| eval(state)),
            },
        }
    }

    pub fn builtin_pure_call(name: &'static str, eval: fn(&S) -> T) -> Self {
        Self::builtin_pure_call_with_paths(name, &[], eval)
    }

    pub fn builtin_pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S) -> T,
    ) -> Self {
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::PureCall {
                    name,
                    symbolic: SymbolicRegistration::Builtin,
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |state| eval(state)),
            },
        }
    }

    pub fn registered_pure_call(
        name: &'static str,
        registration: &'static str,
        eval: fn(&S) -> T,
    ) -> Self {
        Self::registered_pure_call_with_paths(name, registration, &[], eval)
    }

    pub fn registered_pure_call_with_paths(
        name: &'static str,
        registration: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S) -> T,
    ) -> Self {
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::PureCall {
                    name,
                    symbolic: symbolic_pure_registration(registration),
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |state| eval(state)),
            },
        }
    }

    pub fn add(name: &'static str, lhs: Self, rhs: Self) -> Self
    where
        S: Clone,
        T: std::ops::Add<Output = T> + 'static,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        let lhs_ast = lhs.ast().cloned().unwrap_or_else(|| StateExprAst::Opaque {
            repr: lhs.name(),
            _marker: PhantomData,
        });
        let rhs_ast = rhs.ast().cloned().unwrap_or_else(|| StateExprAst::Opaque {
            repr: rhs.name(),
            _marker: PhantomData,
        });
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::Add {
                    lhs: Box::new(lhs_ast),
                    rhs: Box::new(rhs_ast),
                },
                eval: Arc::new(move |state| lhs_eval.eval(state) + rhs_eval.eval(state)),
            },
        }
    }

    pub fn if_else(
        name: &'static str,
        condition: BoolExpr<S>,
        then_branch: Self,
        else_branch: Self,
    ) -> Self
    where
        S: Clone,
        T: 'static,
    {
        let then_eval = then_branch.clone();
        let else_eval = else_branch.clone();
        let then_ast = then_branch
            .ast()
            .cloned()
            .unwrap_or_else(|| StateExprAst::Opaque {
                repr: then_branch.name(),
                _marker: PhantomData,
            });
        let else_ast = else_branch
            .ast()
            .cloned()
            .unwrap_or_else(|| StateExprAst::Opaque {
                repr: else_branch.name(),
                _marker: PhantomData,
            });
        Self {
            name,
            body: StateExprBody::Ast {
                ast: StateExprAst::IfElse {
                    condition: Box::new(condition.clone()),
                    then_branch: Box::new(then_ast),
                    else_branch: Box::new(else_ast),
                },
                eval: Arc::new(move |state| {
                    if condition.eval(state) {
                        then_eval.eval(state)
                    } else {
                        else_eval.eval(state)
                    }
                }),
            },
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast(&self) -> Option<&StateExprAst<S, T>> {
        match &self.body {
            StateExprBody::RustFn(_) => None,
            StateExprBody::Ast { ast, .. } => Some(ast),
        }
    }

    pub fn is_ast_native(&self) -> bool {
        matches!(self.body, StateExprBody::Ast { .. })
    }

    fn erased_ast(&self) -> Option<ErasedStateExprAst<S>>
    where
        S: Clone,
    {
        self.ast().map(StateExprAst::erase)
    }

    pub(crate) fn first_unencodable(&self) -> Option<&'static str>
    where
        S: Clone,
    {
        self.erased_ast().and_then(|ast| ast.first_unencodable())
    }

    pub fn eval(&self, state: &S) -> T {
        match &self.body {
            StateExprBody::RustFn(eval) => eval(state),
            StateExprBody::Ast { eval, .. } => eval(state),
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
            StateExprBody::Ast { .. } => "Ast",
        };
        f.debug_struct("StateExpr")
            .field("name", &self.name)
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Clone)]
pub struct StateComparison<S> {
    op: ComparisonOp,
    lhs: &'static str,
    rhs: &'static str,
    lhs_ast: ErasedStateExprAst<S>,
    rhs_ast: ErasedStateExprAst<S>,
    eval: Arc<dyn Fn(&S) -> bool + 'static>,
}

impl<S: 'static> StateComparison<S> {
    pub fn new(
        op: ComparisonOp,
        lhs: &'static str,
        lhs_ast: ErasedStateExprAst<S>,
        rhs: &'static str,
        rhs_ast: ErasedStateExprAst<S>,
        eval: impl Fn(&S) -> bool + 'static,
    ) -> Self {
        Self {
            op,
            lhs,
            rhs,
            lhs_ast,
            rhs_ast,
            eval: Arc::new(eval),
        }
    }

    pub fn from_exprs<T>(
        op: ComparisonOp,
        lhs: StateExpr<S, T>,
        rhs: StateExpr<S, T>,
        eval: impl Fn(&S) -> bool + 'static,
    ) -> Self
    where
        S: Clone,
        T: Clone + 'static,
    {
        let lhs_name = lhs.name();
        let rhs_name = rhs.name();
        let lhs_ast = lhs
            .erased_ast()
            .unwrap_or(ErasedStateExprAst::Opaque { repr: lhs_name });
        let rhs_ast = rhs
            .erased_ast()
            .unwrap_or(ErasedStateExprAst::Opaque { repr: rhs_name });
        Self::new(op, lhs_name, lhs_ast, rhs_name, rhs_ast, eval)
    }

    pub const fn op(&self) -> ComparisonOp {
        self.op
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

    pub(crate) fn first_unencodable(&self) -> Option<&'static str> {
        self.lhs_ast
            .first_unencodable()
            .or_else(|| self.rhs_ast.first_unencodable())
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_symbolic_pure_keys(keys);
        self.rhs_ast.collect_symbolic_pure_keys(keys);
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_unregistered_symbolic_pure_keys(keys);
        self.rhs_ast.collect_unregistered_symbolic_pure_keys(keys);
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_symbolic_state_paths(paths);
        self.rhs_ast.collect_symbolic_state_paths(paths);
    }
}

impl<S> fmt::Debug for StateComparison<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateComparison")
            .field("op", &self.op)
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

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        collect_symbolic_state_path(paths, self.value);
    }
}

#[derive(Debug, Clone)]
pub struct StateBoolLeaf<S> {
    label: &'static str,
    symbolic: SymbolicRegistration,
    read_paths: &'static [&'static str],
    eval: fn(&S) -> bool,
}

impl<S> StateBoolLeaf<S> {
    pub const fn new(
        label: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            label,
            symbolic,
            read_paths,
            eval,
        }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    fn eval(&self, state: &S) -> bool {
        (self.eval)(state)
    }

    pub(crate) fn first_unencodable(&self) -> Option<&'static str> {
        self.symbolic.first_unencodable()
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.symbolic.collect_key(keys);
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.symbolic.collect_unregistered_key(keys);
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        collect_symbolic_state_paths_from_hints(paths, self.read_paths);
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
    Le(StateComparison<S>),
    Gt(StateComparison<S>),
    Ge(StateComparison<S>),
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
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.eval(state),
            Self::Match(matcher) => matcher.eval(state),
            Self::ForAll(quantifier) | Self::Exists(quantifier) => quantifier.eval(state),
            Self::Not(inner) => !inner.eval(state),
            Self::And(parts) => parts.iter().all(|part| part.eval(state)),
            Self::Or(parts) => parts.iter().any(|part| part.eval(state)),
        }
    }

    pub(crate) fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => None,
            Self::FieldRead(field) | Self::PureCall(field) => field.first_unencodable(),
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.first_unencodable(),
            Self::Not(inner) => inner.first_unencodable_symbolic_node(),
            Self::And(parts) | Self::Or(parts) => parts
                .iter()
                .find_map(BoolExpr::first_unencodable_symbolic_node),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_symbolic_pure_keys(keys)
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_symbolic_pure_keys(keys),
            Self::Not(inner) => inner.collect_symbolic_pure_keys(keys),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_unregistered_symbolic_pure_keys(keys);
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_unregistered_symbolic_pure_keys(keys),
            Self::Not(inner) => inner.collect_unregistered_symbolic_pure_keys(keys),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_unregistered_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_symbolic_state_paths(paths)
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_symbolic_state_paths(paths),
            Self::Match(matcher) => matcher.collect_symbolic_state_paths(paths),
            Self::Not(inner) => inner.collect_symbolic_state_paths(paths),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_symbolic_state_paths(paths);
                }
            }
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
            body: BoolExprBody::Ast(BoolExprAst::FieldRead(StateBoolLeaf::new(
                path,
                SymbolicRegistration::Builtin,
                &[],
                read,
            ))),
        }
    }

    pub const fn pure_call(name: &'static str, eval: fn(&S) -> bool) -> Self {
        Self::pure_call_with_paths(name, &[], eval)
    }

    pub const fn pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::PureCall(StateBoolLeaf::new(
                name,
                SymbolicRegistration::Unregistered(name),
                read_paths,
                eval,
            ))),
        }
    }

    pub const fn builtin_pure_call(name: &'static str, eval: fn(&S) -> bool) -> Self {
        Self::builtin_pure_call_with_paths(name, &[], eval)
    }

    pub const fn builtin_pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::PureCall(StateBoolLeaf::new(
                name,
                SymbolicRegistration::Builtin,
                read_paths,
                eval,
            ))),
        }
    }

    pub fn registered_pure_call(
        name: &'static str,
        registration: &'static str,
        eval: fn(&S) -> bool,
    ) -> Self {
        Self::registered_pure_call_with_paths(name, registration, &[], eval)
    }

    pub fn registered_pure_call_with_paths(
        name: &'static str,
        registration: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S) -> bool,
    ) -> Self {
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::PureCall(StateBoolLeaf::new(
                name,
                symbolic_pure_registration(registration),
                read_paths,
                eval,
            ))),
        }
    }

    pub fn eq_expr<T>(name: &'static str, lhs: StateExpr<S, T>, rhs: StateExpr<S, T>) -> Self
    where
        S: Clone,
        T: PartialEq + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Eq(StateComparison::from_exprs(
                ComparisonOp::Eq,
                lhs,
                rhs,
                move |state| lhs_eval.eval(state) == rhs_eval.eval(state),
            ))),
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
        S: Clone,
        T: PartialEq + 'static + Clone,
    {
        Self::eq_expr(
            name,
            StateExpr::opaque(lhs, lhs, lhs_eval),
            StateExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn ne_expr<T>(name: &'static str, lhs: StateExpr<S, T>, rhs: StateExpr<S, T>) -> Self
    where
        S: Clone,
        T: PartialEq + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Ne(StateComparison::from_exprs(
                ComparisonOp::Ne,
                lhs,
                rhs,
                move |state| lhs_eval.eval(state) != rhs_eval.eval(state),
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
        S: Clone,
        T: PartialEq + 'static + Clone,
    {
        Self::ne_expr(
            name,
            StateExpr::opaque(lhs, lhs, lhs_eval),
            StateExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn lt_expr<T>(name: &'static str, lhs: StateExpr<S, T>, rhs: StateExpr<S, T>) -> Self
    where
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Lt(StateComparison::from_exprs(
                ComparisonOp::Lt,
                lhs,
                rhs,
                move |state| lhs_eval.eval(state) < rhs_eval.eval(state),
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
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::lt_expr(
            name,
            StateExpr::opaque(lhs, lhs, lhs_eval),
            StateExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn le_expr<T>(name: &'static str, lhs: StateExpr<S, T>, rhs: StateExpr<S, T>) -> Self
    where
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Le(StateComparison::from_exprs(
                ComparisonOp::Le,
                lhs,
                rhs,
                move |state| lhs_eval.eval(state) <= rhs_eval.eval(state),
            ))),
        }
    }

    pub fn le<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::le_expr(
            name,
            StateExpr::opaque(lhs, lhs, lhs_eval),
            StateExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn gt_expr<T>(name: &'static str, lhs: StateExpr<S, T>, rhs: StateExpr<S, T>) -> Self
    where
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Gt(StateComparison::from_exprs(
                ComparisonOp::Gt,
                lhs,
                rhs,
                move |state| lhs_eval.eval(state) > rhs_eval.eval(state),
            ))),
        }
    }

    pub fn gt<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::gt_expr(
            name,
            StateExpr::opaque(lhs, lhs, lhs_eval),
            StateExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn ge_expr<T>(name: &'static str, lhs: StateExpr<S, T>, rhs: StateExpr<S, T>) -> Self
    where
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: BoolExprBody::Ast(BoolExprAst::Ge(StateComparison::from_exprs(
                ComparisonOp::Ge,
                lhs,
                rhs,
                move |state| lhs_eval.eval(state) >= rhs_eval.eval(state),
            ))),
        }
    }

    pub fn ge<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S) -> T,
    ) -> Self
    where
        S: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::ge_expr(
            name,
            StateExpr::opaque(lhs, lhs, lhs_eval),
            StateExpr::opaque(rhs, rhs, rhs_eval),
        )
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

    pub fn first_unencodable_symbolic_node(&self) -> Option<&'static str> {
        self.ast().and_then(BoolExprAst::first_unencodable)
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_symbolic_pure_keys(keys);
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_unregistered_symbolic_pure_keys(keys);
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_symbolic_state_paths(paths);
        }
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        let mut paths = BTreeSet::new();
        self.collect_symbolic_state_paths(&mut paths);
        paths.into_iter().collect()
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

#[derive(Debug, Clone)]
pub enum ErasedStepValueExprAst<S, A> {
    Opaque {
        repr: &'static str,
    },
    Literal {
        repr: &'static str,
    },
    FieldRead {
        path: &'static str,
    },
    PureCall {
        name: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
    },
    Add {
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    IfElse {
        condition: Box<StepExpr<S, A>>,
        then_branch: Box<Self>,
        else_branch: Box<Self>,
    },
}

impl<S: 'static, A: 'static> ErasedStepValueExprAst<S, A> {
    pub(crate) fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Opaque { repr } => Some(repr),
            Self::Literal { .. } | Self::FieldRead { .. } => None,
            Self::PureCall { symbolic, .. } => symbolic.first_unencodable(),
            Self::Add { lhs, rhs } => lhs.first_unencodable().or_else(|| rhs.first_unencodable()),
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => condition
                .first_unencodable_symbolic_node()
                .or_else(|| then_branch.first_unencodable())
                .or_else(|| else_branch.first_unencodable()),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } | Self::FieldRead { .. } => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_pure_keys(keys);
                rhs.collect_symbolic_pure_keys(keys);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_symbolic_pure_keys(keys);
                then_branch.collect_symbolic_pure_keys(keys);
                else_branch.collect_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } | Self::FieldRead { .. } => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_unregistered_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_unregistered_symbolic_pure_keys(keys);
                rhs.collect_unregistered_symbolic_pure_keys(keys);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_unregistered_symbolic_pure_keys(keys);
                then_branch.collect_unregistered_symbolic_pure_keys(keys);
                else_branch.collect_unregistered_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } => {}
            Self::FieldRead { path } => collect_symbolic_state_path(paths, path),
            Self::PureCall { read_paths, .. } => {
                collect_symbolic_state_paths_from_hints(paths, read_paths);
            }
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_state_paths(paths);
                rhs.collect_symbolic_state_paths(paths);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_symbolic_state_paths(paths);
                then_branch.collect_symbolic_state_paths(paths);
                else_branch.collect_symbolic_state_paths(paths);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum StepValueExprAst<S, A, T> {
    Opaque {
        repr: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    Literal {
        repr: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    FieldRead {
        path: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    PureCall {
        name: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
        _marker: PhantomData<fn() -> T>,
    },
    Add {
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    IfElse {
        condition: Box<StepExpr<S, A>>,
        then_branch: Box<Self>,
        else_branch: Box<Self>,
    },
}

impl<S: Clone, A: Clone, T> StepValueExprAst<S, A, T> {
    fn erase(&self) -> ErasedStepValueExprAst<S, A> {
        match self {
            Self::Opaque { repr, .. } => ErasedStepValueExprAst::Opaque { repr },
            Self::Literal { repr, .. } => ErasedStepValueExprAst::Literal { repr },
            Self::FieldRead { path, .. } => ErasedStepValueExprAst::FieldRead { path },
            Self::PureCall {
                name,
                symbolic,
                read_paths,
                ..
            } => ErasedStepValueExprAst::PureCall {
                name,
                symbolic: *symbolic,
                read_paths,
            },
            Self::Add { lhs, rhs } => ErasedStepValueExprAst::Add {
                lhs: Box::new(lhs.erase()),
                rhs: Box::new(rhs.erase()),
            },
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => ErasedStepValueExprAst::IfElse {
                condition: condition.clone(),
                then_branch: Box::new(then_branch.erase()),
                else_branch: Box::new(else_branch.erase()),
            },
        }
    }
}

#[derive(Clone)]
pub enum StepValueExprBody<S, A, T> {
    RustFn(fn(&S, &A, &S) -> T),
    Ast {
        ast: StepValueExprAst<S, A, T>,
        eval: Arc<dyn Fn(&S, &A, &S) -> T + 'static>,
    },
}

#[derive(Clone)]
pub struct StepValueExpr<S, A, T> {
    name: &'static str,
    body: StepValueExprBody<S, A, T>,
}

impl<S: 'static, A: 'static, T> StepValueExpr<S, A, T>
where
    T: Clone + 'static,
{
    pub fn literal(name: &'static str, value: T) -> Self {
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::Literal {
                    repr: name,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |_, _, _| value.clone()),
            },
        }
    }

    pub fn literal_with_repr(name: &'static str, repr: &'static str, value: T) -> Self {
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::Literal {
                    repr,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |_, _, _| value.clone()),
            },
        }
    }

    pub fn opaque(name: &'static str, repr: &'static str, eval: fn(&S, &A, &S) -> T) -> Self {
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::Opaque {
                    repr,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action, next| eval(prev, action, next)),
            },
        }
    }

    pub fn field(name: &'static str, path: &'static str, read: fn(&S, &A, &S) -> T) -> Self {
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::FieldRead {
                    path,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action, next| read(prev, action, next)),
            },
        }
    }

    pub fn pure_call(name: &'static str, eval: fn(&S, &A, &S) -> T) -> Self {
        Self::pure_call_with_paths(name, &[], eval)
    }

    pub fn pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A, &S) -> T,
    ) -> Self {
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::PureCall {
                    name,
                    symbolic: SymbolicRegistration::Unregistered(name),
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action, next| eval(prev, action, next)),
            },
        }
    }

    pub fn builtin_pure_call(name: &'static str, eval: fn(&S, &A, &S) -> T) -> Self {
        Self::builtin_pure_call_with_paths(name, &[], eval)
    }

    pub fn builtin_pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A, &S) -> T,
    ) -> Self {
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::PureCall {
                    name,
                    symbolic: SymbolicRegistration::Builtin,
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action, next| eval(prev, action, next)),
            },
        }
    }

    pub fn registered_pure_call(
        name: &'static str,
        registration: &'static str,
        eval: fn(&S, &A, &S) -> T,
    ) -> Self {
        Self::registered_pure_call_with_paths(name, registration, &[], eval)
    }

    pub fn registered_pure_call_with_paths(
        name: &'static str,
        registration: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A, &S) -> T,
    ) -> Self {
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::PureCall {
                    name,
                    symbolic: symbolic_pure_registration(registration),
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action, next| eval(prev, action, next)),
            },
        }
    }

    pub fn add(name: &'static str, lhs: Self, rhs: Self) -> Self
    where
        S: Clone,
        A: Clone,
        T: std::ops::Add<Output = T> + 'static,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        let lhs_ast = lhs
            .ast()
            .cloned()
            .unwrap_or_else(|| StepValueExprAst::Opaque {
                repr: lhs.name(),
                _marker: PhantomData,
            });
        let rhs_ast = rhs
            .ast()
            .cloned()
            .unwrap_or_else(|| StepValueExprAst::Opaque {
                repr: rhs.name(),
                _marker: PhantomData,
            });
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::Add {
                    lhs: Box::new(lhs_ast),
                    rhs: Box::new(rhs_ast),
                },
                eval: Arc::new(move |prev, action, next| {
                    lhs_eval.eval(prev, action, next) + rhs_eval.eval(prev, action, next)
                }),
            },
        }
    }

    pub fn if_else(
        name: &'static str,
        condition: StepExpr<S, A>,
        then_branch: Self,
        else_branch: Self,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: 'static,
    {
        let then_eval = then_branch.clone();
        let else_eval = else_branch.clone();
        let then_ast = then_branch
            .ast()
            .cloned()
            .unwrap_or_else(|| StepValueExprAst::Opaque {
                repr: then_branch.name(),
                _marker: PhantomData,
            });
        let else_ast = else_branch
            .ast()
            .cloned()
            .unwrap_or_else(|| StepValueExprAst::Opaque {
                repr: else_branch.name(),
                _marker: PhantomData,
            });
        Self {
            name,
            body: StepValueExprBody::Ast {
                ast: StepValueExprAst::IfElse {
                    condition: Box::new(condition.clone()),
                    then_branch: Box::new(then_ast),
                    else_branch: Box::new(else_ast),
                },
                eval: Arc::new(move |prev, action, next| {
                    if condition.eval(prev, action, next) {
                        then_eval.eval(prev, action, next)
                    } else {
                        else_eval.eval(prev, action, next)
                    }
                }),
            },
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast(&self) -> Option<&StepValueExprAst<S, A, T>> {
        match &self.body {
            StepValueExprBody::RustFn(_) => None,
            StepValueExprBody::Ast { ast, .. } => Some(ast),
        }
    }

    fn erased_ast(&self) -> Option<ErasedStepValueExprAst<S, A>>
    where
        S: Clone,
        A: Clone,
    {
        self.ast().map(StepValueExprAst::erase)
    }

    pub fn eval(&self, prev: &S, action: &A, next: &S) -> T {
        match &self.body {
            StepValueExprBody::RustFn(eval) => eval(prev, action, next),
            StepValueExprBody::Ast { eval, .. } => eval(prev, action, next),
        }
    }
}

#[derive(Clone)]
pub struct StepComparison<S, A> {
    op: ComparisonOp,
    lhs: &'static str,
    rhs: &'static str,
    lhs_ast: ErasedStepValueExprAst<S, A>,
    rhs_ast: ErasedStepValueExprAst<S, A>,
    eval: Arc<dyn Fn(&S, &A, &S) -> bool + 'static>,
}

impl<S: 'static, A: 'static> StepComparison<S, A> {
    pub fn new(
        op: ComparisonOp,
        lhs: &'static str,
        lhs_ast: ErasedStepValueExprAst<S, A>,
        rhs: &'static str,
        rhs_ast: ErasedStepValueExprAst<S, A>,
        eval: impl Fn(&S, &A, &S) -> bool + 'static,
    ) -> Self {
        Self {
            op,
            lhs,
            rhs,
            lhs_ast,
            rhs_ast,
            eval: Arc::new(eval),
        }
    }

    pub fn from_exprs<T>(
        op: ComparisonOp,
        lhs: StepValueExpr<S, A, T>,
        rhs: StepValueExpr<S, A, T>,
        eval: impl Fn(&S, &A, &S) -> bool + 'static,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: Clone + 'static,
    {
        let lhs_name = lhs.name();
        let rhs_name = rhs.name();
        let lhs_ast = lhs
            .erased_ast()
            .unwrap_or(ErasedStepValueExprAst::Opaque { repr: lhs_name });
        let rhs_ast = rhs
            .erased_ast()
            .unwrap_or(ErasedStepValueExprAst::Opaque { repr: rhs_name });
        Self::new(op, lhs_name, lhs_ast, rhs_name, rhs_ast, eval)
    }

    pub const fn op(&self) -> ComparisonOp {
        self.op
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

    fn first_unencodable(&self) -> Option<&'static str> {
        self.lhs_ast
            .first_unencodable()
            .or_else(|| self.rhs_ast.first_unencodable())
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_symbolic_pure_keys(keys);
        self.rhs_ast.collect_symbolic_pure_keys(keys);
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_unregistered_symbolic_pure_keys(keys);
        self.rhs_ast.collect_unregistered_symbolic_pure_keys(keys);
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_symbolic_state_paths(paths);
        self.rhs_ast.collect_symbolic_state_paths(paths);
    }
}

impl<S, A> fmt::Debug for StepComparison<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StepComparison")
            .field("op", &self.op)
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

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        collect_symbolic_state_path(paths, self.value);
    }
}

#[derive(Debug, Clone)]
pub struct StepBoolLeaf<S, A> {
    label: &'static str,
    symbolic: SymbolicRegistration,
    read_paths: &'static [&'static str],
    eval: fn(&S, &A, &S) -> bool,
}

impl<S, A> StepBoolLeaf<S, A> {
    pub const fn new(
        label: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            label,
            symbolic,
            read_paths,
            eval,
        }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    fn eval(&self, prev: &S, action: &A, next: &S) -> bool {
        (self.eval)(prev, action, next)
    }

    fn first_unencodable(&self) -> Option<&'static str> {
        self.symbolic.first_unencodable()
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.symbolic.collect_key(keys);
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.symbolic.collect_unregistered_key(keys);
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        collect_symbolic_state_paths_from_hints(paths, self.read_paths);
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
    Le(StepComparison<S, A>),
    Gt(StepComparison<S, A>),
    Ge(StepComparison<S, A>),
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
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.eval(prev, action, next),
            Self::Match(matcher) => matcher.eval(prev, action, next),
            Self::ForAll(quantifier) | Self::Exists(quantifier) => {
                quantifier.eval(prev, action, next)
            }
            Self::Not(inner) => !inner.eval(prev, action, next),
            Self::And(parts) => parts.iter().all(|part| part.eval(prev, action, next)),
            Self::Or(parts) => parts.iter().any(|part| part.eval(prev, action, next)),
        }
    }

    fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => None,
            Self::FieldRead(field) | Self::PureCall(field) => field.first_unencodable(),
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.first_unencodable(),
            Self::Not(inner) => inner.first_unencodable_symbolic_node(),
            Self::And(parts) | Self::Or(parts) => parts
                .iter()
                .find_map(StepExpr::first_unencodable_symbolic_node),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_symbolic_pure_keys(keys)
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_symbolic_pure_keys(keys),
            Self::Not(inner) => inner.collect_symbolic_pure_keys(keys),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_unregistered_symbolic_pure_keys(keys);
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_unregistered_symbolic_pure_keys(keys),
            Self::Not(inner) => inner.collect_unregistered_symbolic_pure_keys(keys),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_unregistered_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) => collect_symbolic_state_path(paths, field.label()),
            Self::PureCall(field) => field.collect_symbolic_state_paths(paths),
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_symbolic_state_paths(paths),
            Self::Match(matcher) => matcher.collect_symbolic_state_paths(paths),
            Self::Not(inner) => inner.collect_symbolic_state_paths(paths),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_symbolic_state_paths(paths);
                }
            }
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
            body: StepExprBody::Ast(StepExprAst::FieldRead(StepBoolLeaf::new(
                path,
                SymbolicRegistration::Builtin,
                &[],
                read,
            ))),
        }
    }

    pub const fn pure_call(name: &'static str, eval: fn(&S, &A, &S) -> bool) -> Self {
        Self::pure_call_with_paths(name, &[], eval)
    }

    pub const fn pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::PureCall(StepBoolLeaf::new(
                name,
                SymbolicRegistration::Unregistered(name),
                read_paths,
                eval,
            ))),
        }
    }

    pub const fn builtin_pure_call(name: &'static str, eval: fn(&S, &A, &S) -> bool) -> Self {
        Self::builtin_pure_call_with_paths(name, &[], eval)
    }

    pub const fn builtin_pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::PureCall(StepBoolLeaf::new(
                name,
                SymbolicRegistration::Builtin,
                read_paths,
                eval,
            ))),
        }
    }

    pub fn registered_pure_call(
        name: &'static str,
        registration: &'static str,
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self::registered_pure_call_with_paths(name, registration, &[], eval)
    }

    pub fn registered_pure_call_with_paths(
        name: &'static str,
        registration: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A, &S) -> bool,
    ) -> Self {
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::PureCall(StepBoolLeaf::new(
                name,
                symbolic_pure_registration(registration),
                read_paths,
                eval,
            ))),
        }
    }

    pub fn eq_expr<T>(
        name: &'static str,
        lhs: StepValueExpr<S, A, T>,
        rhs: StepValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Eq(StepComparison::from_exprs(
                ComparisonOp::Eq,
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval.eval(prev, action, next) == rhs_eval.eval(prev, action, next)
                },
            ))),
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
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        Self::eq_expr(
            name,
            StepValueExpr::opaque(lhs, lhs, lhs_eval),
            StepValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn ne_expr<T>(
        name: &'static str,
        lhs: StepValueExpr<S, A, T>,
        rhs: StepValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Ne(StepComparison::from_exprs(
                ComparisonOp::Ne,
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval.eval(prev, action, next) != rhs_eval.eval(prev, action, next)
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
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        Self::ne_expr(
            name,
            StepValueExpr::opaque(lhs, lhs, lhs_eval),
            StepValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn lt_expr<T>(
        name: &'static str,
        lhs: StepValueExpr<S, A, T>,
        rhs: StepValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Lt(StepComparison::from_exprs(
                ComparisonOp::Lt,
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval.eval(prev, action, next) < rhs_eval.eval(prev, action, next)
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
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::lt_expr(
            name,
            StepValueExpr::opaque(lhs, lhs, lhs_eval),
            StepValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn le_expr<T>(
        name: &'static str,
        lhs: StepValueExpr<S, A, T>,
        rhs: StepValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Le(StepComparison::from_exprs(
                ComparisonOp::Le,
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval.eval(prev, action, next) <= rhs_eval.eval(prev, action, next)
                },
            ))),
        }
    }

    pub fn le<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::le_expr(
            name,
            StepValueExpr::opaque(lhs, lhs, lhs_eval),
            StepValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn gt_expr<T>(
        name: &'static str,
        lhs: StepValueExpr<S, A, T>,
        rhs: StepValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Gt(StepComparison::from_exprs(
                ComparisonOp::Gt,
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval.eval(prev, action, next) > rhs_eval.eval(prev, action, next)
                },
            ))),
        }
    }

    pub fn gt<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::gt_expr(
            name,
            StepValueExpr::opaque(lhs, lhs, lhs_eval),
            StepValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn ge_expr<T>(
        name: &'static str,
        lhs: StepValueExpr<S, A, T>,
        rhs: StepValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: StepExprBody::Ast(StepExprAst::Ge(StepComparison::from_exprs(
                ComparisonOp::Ge,
                lhs,
                rhs,
                move |prev, action, next| {
                    lhs_eval.eval(prev, action, next) >= rhs_eval.eval(prev, action, next)
                },
            ))),
        }
    }

    pub fn ge<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A, &S) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A, &S) -> T,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::ge_expr(
            name,
            StepValueExpr::opaque(lhs, lhs, lhs_eval),
            StepValueExpr::opaque(rhs, rhs, rhs_eval),
        )
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

    pub fn first_unencodable_symbolic_node(&self) -> Option<&'static str> {
        self.ast().and_then(StepExprAst::first_unencodable)
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_symbolic_pure_keys(keys);
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_unregistered_symbolic_pure_keys(keys);
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_symbolic_state_paths(paths);
        }
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        let mut paths = BTreeSet::new();
        self.collect_symbolic_state_paths(&mut paths);
        paths.into_iter().collect()
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

#[derive(Debug, Clone)]
pub enum ErasedGuardValueExprAst<S, A> {
    Opaque {
        repr: &'static str,
    },
    Literal {
        repr: &'static str,
    },
    FieldRead {
        path: &'static str,
    },
    PureCall {
        name: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
    },
    Add {
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    IfElse {
        condition: Box<GuardExpr<S, A>>,
        then_branch: Box<Self>,
        else_branch: Box<Self>,
    },
}

impl<S: 'static, A: 'static> ErasedGuardValueExprAst<S, A> {
    fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Opaque { repr } => Some(repr),
            Self::Literal { .. } | Self::FieldRead { .. } => None,
            Self::PureCall { symbolic, .. } => symbolic.first_unencodable(),
            Self::Add { lhs, rhs } => lhs.first_unencodable().or_else(|| rhs.first_unencodable()),
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => condition
                .first_unencodable_symbolic_node()
                .or_else(|| then_branch.first_unencodable())
                .or_else(|| else_branch.first_unencodable()),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } | Self::FieldRead { .. } => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_pure_keys(keys);
                rhs.collect_symbolic_pure_keys(keys);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_symbolic_pure_keys(keys);
                then_branch.collect_symbolic_pure_keys(keys);
                else_branch.collect_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } | Self::FieldRead { .. } => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_unregistered_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_unregistered_symbolic_pure_keys(keys);
                rhs.collect_unregistered_symbolic_pure_keys(keys);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_unregistered_symbolic_pure_keys(keys);
                then_branch.collect_unregistered_symbolic_pure_keys(keys);
                else_branch.collect_unregistered_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } => {}
            Self::FieldRead { path } => collect_symbolic_state_path(paths, path),
            Self::PureCall { read_paths, .. } => {
                collect_symbolic_state_paths_from_hints(paths, read_paths);
            }
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_state_paths(paths);
                rhs.collect_symbolic_state_paths(paths);
            }
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => {
                condition.collect_symbolic_state_paths(paths);
                then_branch.collect_symbolic_state_paths(paths);
                else_branch.collect_symbolic_state_paths(paths);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum GuardValueExprAst<S, A, T> {
    Opaque {
        repr: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    Literal {
        repr: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    FieldRead {
        path: &'static str,
        _marker: PhantomData<fn() -> T>,
    },
    PureCall {
        name: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
        _marker: PhantomData<fn() -> T>,
    },
    Add {
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    IfElse {
        condition: Box<GuardExpr<S, A>>,
        then_branch: Box<Self>,
        else_branch: Box<Self>,
    },
}

impl<S: Clone, A: Clone, T> GuardValueExprAst<S, A, T> {
    fn erase(&self) -> ErasedGuardValueExprAst<S, A> {
        match self {
            Self::Opaque { repr, .. } => ErasedGuardValueExprAst::Opaque { repr },
            Self::Literal { repr, .. } => ErasedGuardValueExprAst::Literal { repr },
            Self::FieldRead { path, .. } => ErasedGuardValueExprAst::FieldRead { path },
            Self::PureCall {
                name,
                symbolic,
                read_paths,
                ..
            } => ErasedGuardValueExprAst::PureCall {
                name,
                symbolic: *symbolic,
                read_paths,
            },
            Self::Add { lhs, rhs } => ErasedGuardValueExprAst::Add {
                lhs: Box::new(lhs.erase()),
                rhs: Box::new(rhs.erase()),
            },
            Self::IfElse {
                condition,
                then_branch,
                else_branch,
            } => ErasedGuardValueExprAst::IfElse {
                condition: condition.clone(),
                then_branch: Box::new(then_branch.erase()),
                else_branch: Box::new(else_branch.erase()),
            },
        }
    }
}

#[derive(Clone)]
pub enum GuardValueExprBody<S, A, T> {
    RustFn(fn(&S, &A) -> T),
    Ast {
        ast: GuardValueExprAst<S, A, T>,
        eval: Arc<dyn Fn(&S, &A) -> T + 'static>,
    },
}

#[derive(Clone)]
pub struct GuardValueExpr<S, A, T> {
    name: &'static str,
    body: GuardValueExprBody<S, A, T>,
}

impl<S: 'static, A: 'static, T> GuardValueExpr<S, A, T>
where
    T: Clone + 'static,
{
    pub fn literal(name: &'static str, value: T) -> Self {
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::Literal {
                    repr: name,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |_, _| value.clone()),
            },
        }
    }

    pub fn literal_with_repr(name: &'static str, repr: &'static str, value: T) -> Self {
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::Literal {
                    repr,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |_, _| value.clone()),
            },
        }
    }

    pub fn opaque(name: &'static str, repr: &'static str, eval: fn(&S, &A) -> T) -> Self {
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::Opaque {
                    repr,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action| eval(prev, action)),
            },
        }
    }

    pub fn field(name: &'static str, path: &'static str, read: fn(&S, &A) -> T) -> Self {
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::FieldRead {
                    path,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action| read(prev, action)),
            },
        }
    }

    pub fn pure_call(name: &'static str, eval: fn(&S, &A) -> T) -> Self {
        Self::pure_call_with_paths(name, &[], eval)
    }

    pub fn pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A) -> T,
    ) -> Self {
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::PureCall {
                    name,
                    symbolic: SymbolicRegistration::Unregistered(name),
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action| eval(prev, action)),
            },
        }
    }

    pub fn builtin_pure_call(name: &'static str, eval: fn(&S, &A) -> T) -> Self {
        Self::builtin_pure_call_with_paths(name, &[], eval)
    }

    pub fn builtin_pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A) -> T,
    ) -> Self {
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::PureCall {
                    name,
                    symbolic: SymbolicRegistration::Builtin,
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action| eval(prev, action)),
            },
        }
    }

    pub fn registered_pure_call(
        name: &'static str,
        registration: &'static str,
        eval: fn(&S, &A) -> T,
    ) -> Self {
        Self::registered_pure_call_with_paths(name, registration, &[], eval)
    }

    pub fn registered_pure_call_with_paths(
        name: &'static str,
        registration: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A) -> T,
    ) -> Self {
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::PureCall {
                    name,
                    symbolic: symbolic_pure_registration(registration),
                    read_paths,
                    _marker: PhantomData,
                },
                eval: Arc::new(move |prev, action| eval(prev, action)),
            },
        }
    }

    pub fn add(name: &'static str, lhs: Self, rhs: Self) -> Self
    where
        S: Clone,
        A: Clone,
        T: std::ops::Add<Output = T> + 'static,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        let lhs_ast = lhs
            .ast()
            .cloned()
            .unwrap_or_else(|| GuardValueExprAst::Opaque {
                repr: lhs.name(),
                _marker: PhantomData,
            });
        let rhs_ast = rhs
            .ast()
            .cloned()
            .unwrap_or_else(|| GuardValueExprAst::Opaque {
                repr: rhs.name(),
                _marker: PhantomData,
            });
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::Add {
                    lhs: Box::new(lhs_ast),
                    rhs: Box::new(rhs_ast),
                },
                eval: Arc::new(move |prev, action| {
                    lhs_eval.eval(prev, action) + rhs_eval.eval(prev, action)
                }),
            },
        }
    }

    pub fn if_else(
        name: &'static str,
        condition: GuardExpr<S, A>,
        then_branch: Self,
        else_branch: Self,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: 'static,
    {
        let then_eval = then_branch.clone();
        let else_eval = else_branch.clone();
        let then_ast = then_branch
            .ast()
            .cloned()
            .unwrap_or_else(|| GuardValueExprAst::Opaque {
                repr: then_branch.name(),
                _marker: PhantomData,
            });
        let else_ast = else_branch
            .ast()
            .cloned()
            .unwrap_or_else(|| GuardValueExprAst::Opaque {
                repr: else_branch.name(),
                _marker: PhantomData,
            });
        Self {
            name,
            body: GuardValueExprBody::Ast {
                ast: GuardValueExprAst::IfElse {
                    condition: Box::new(condition.clone()),
                    then_branch: Box::new(then_ast),
                    else_branch: Box::new(else_ast),
                },
                eval: Arc::new(move |prev, action| {
                    if condition.eval(prev, action) {
                        then_eval.eval(prev, action)
                    } else {
                        else_eval.eval(prev, action)
                    }
                }),
            },
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn ast(&self) -> Option<&GuardValueExprAst<S, A, T>> {
        match &self.body {
            GuardValueExprBody::RustFn(_) => None,
            GuardValueExprBody::Ast { ast, .. } => Some(ast),
        }
    }

    fn erased_ast(&self) -> Option<ErasedGuardValueExprAst<S, A>>
    where
        S: Clone,
        A: Clone,
    {
        self.ast().map(GuardValueExprAst::erase)
    }

    pub fn eval(&self, prev: &S, action: &A) -> T {
        match &self.body {
            GuardValueExprBody::RustFn(eval) => eval(prev, action),
            GuardValueExprBody::Ast { eval, .. } => eval(prev, action),
        }
    }
}

#[derive(Clone)]
pub struct GuardComparison<S, A> {
    op: ComparisonOp,
    lhs: &'static str,
    rhs: &'static str,
    lhs_ast: ErasedGuardValueExprAst<S, A>,
    rhs_ast: ErasedGuardValueExprAst<S, A>,
    eval: Arc<dyn Fn(&S, &A) -> bool + 'static>,
}

impl<S: 'static, A: 'static> GuardComparison<S, A> {
    pub fn new(
        op: ComparisonOp,
        lhs: &'static str,
        lhs_ast: ErasedGuardValueExprAst<S, A>,
        rhs: &'static str,
        rhs_ast: ErasedGuardValueExprAst<S, A>,
        eval: impl Fn(&S, &A) -> bool + 'static,
    ) -> Self {
        Self {
            op,
            lhs,
            rhs,
            lhs_ast,
            rhs_ast,
            eval: Arc::new(eval),
        }
    }

    pub fn from_exprs<T>(
        op: ComparisonOp,
        lhs: GuardValueExpr<S, A, T>,
        rhs: GuardValueExpr<S, A, T>,
        eval: impl Fn(&S, &A) -> bool + 'static,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: Clone + 'static,
    {
        let lhs_name = lhs.name();
        let rhs_name = rhs.name();
        let lhs_ast = lhs
            .erased_ast()
            .unwrap_or(ErasedGuardValueExprAst::Opaque { repr: lhs_name });
        let rhs_ast = rhs
            .erased_ast()
            .unwrap_or(ErasedGuardValueExprAst::Opaque { repr: rhs_name });
        Self::new(op, lhs_name, lhs_ast, rhs_name, rhs_ast, eval)
    }

    pub const fn op(&self) -> ComparisonOp {
        self.op
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

    fn first_unencodable(&self) -> Option<&'static str> {
        self.lhs_ast
            .first_unencodable()
            .or_else(|| self.rhs_ast.first_unencodable())
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_symbolic_pure_keys(keys);
        self.rhs_ast.collect_symbolic_pure_keys(keys);
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_unregistered_symbolic_pure_keys(keys);
        self.rhs_ast.collect_unregistered_symbolic_pure_keys(keys);
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        self.lhs_ast.collect_symbolic_state_paths(paths);
        self.rhs_ast.collect_symbolic_state_paths(paths);
    }
}

impl<S, A> fmt::Debug for GuardComparison<S, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardComparison")
            .field("op", &self.op)
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

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        collect_symbolic_state_path(paths, self.value);
    }
}

#[derive(Debug, Clone)]
pub struct GuardBoolLeaf<S, A> {
    label: &'static str,
    symbolic: SymbolicRegistration,
    read_paths: &'static [&'static str],
    eval: fn(&S, &A) -> bool,
}

impl<S, A> GuardBoolLeaf<S, A> {
    pub const fn new(
        label: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            label,
            symbolic,
            read_paths,
            eval,
        }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    fn eval(&self, prev: &S, action: &A) -> bool {
        (self.eval)(prev, action)
    }

    fn first_unencodable(&self) -> Option<&'static str> {
        self.symbolic.first_unencodable()
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.symbolic.collect_key(keys);
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        self.symbolic.collect_unregistered_key(keys);
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        collect_symbolic_state_paths_from_hints(paths, self.read_paths);
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
    Le(GuardComparison<S, A>),
    Gt(GuardComparison<S, A>),
    Ge(GuardComparison<S, A>),
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
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.eval(prev, action),
            Self::Match(matcher) => matcher.eval(prev, action),
            Self::ForAll(quantifier) | Self::Exists(quantifier) => quantifier.eval(prev, action),
            Self::Not(inner) => !inner.eval(prev, action),
            Self::And(parts) => parts.iter().all(|part| part.eval(prev, action)),
            Self::Or(parts) => parts.iter().any(|part| part.eval(prev, action)),
        }
    }

    fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => None,
            Self::FieldRead(field) | Self::PureCall(field) => field.first_unencodable(),
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.first_unencodable(),
            Self::Not(inner) => inner.first_unencodable_symbolic_node(),
            Self::And(parts) | Self::Or(parts) => parts
                .iter()
                .find_map(GuardExpr::first_unencodable_symbolic_node),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_symbolic_pure_keys(keys)
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_symbolic_pure_keys(keys),
            Self::Not(inner) => inner.collect_symbolic_pure_keys(keys),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::Match(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_unregistered_symbolic_pure_keys(keys);
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_unregistered_symbolic_pure_keys(keys),
            Self::Not(inner) => inner.collect_unregistered_symbolic_pure_keys(keys),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_unregistered_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Literal(_) | Self::ForAll(_) | Self::Exists(_) => {}
            Self::FieldRead(field) | Self::PureCall(field) => {
                field.collect_symbolic_state_paths(paths)
            }
            Self::Eq(compare)
            | Self::Ne(compare)
            | Self::Lt(compare)
            | Self::Le(compare)
            | Self::Gt(compare)
            | Self::Ge(compare) => compare.collect_symbolic_state_paths(paths),
            Self::Match(matcher) => matcher.collect_symbolic_state_paths(paths),
            Self::Not(inner) => inner.collect_symbolic_state_paths(paths),
            Self::And(parts) | Self::Or(parts) => {
                for part in parts {
                    part.collect_symbolic_state_paths(paths);
                }
            }
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
            body: GuardExprBody::Ast(GuardAst::FieldRead(GuardBoolLeaf::new(
                path,
                SymbolicRegistration::Builtin,
                &[],
                read,
            ))),
        }
    }

    pub const fn pure_call(name: &'static str, eval: fn(&S, &A) -> bool) -> Self {
        Self::pure_call_with_paths(name, &[], eval)
    }

    pub const fn pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::PureCall(GuardBoolLeaf::new(
                name,
                SymbolicRegistration::Unregistered(name),
                read_paths,
                eval,
            ))),
        }
    }

    pub const fn builtin_pure_call(name: &'static str, eval: fn(&S, &A) -> bool) -> Self {
        Self::builtin_pure_call_with_paths(name, &[], eval)
    }

    pub const fn builtin_pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::PureCall(GuardBoolLeaf::new(
                name,
                SymbolicRegistration::Builtin,
                read_paths,
                eval,
            ))),
        }
    }

    pub fn registered_pure_call(
        name: &'static str,
        registration: &'static str,
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self::registered_pure_call_with_paths(name, registration, &[], eval)
    }

    pub fn registered_pure_call_with_paths(
        name: &'static str,
        registration: &'static str,
        read_paths: &'static [&'static str],
        eval: fn(&S, &A) -> bool,
    ) -> Self {
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::PureCall(GuardBoolLeaf::new(
                name,
                symbolic_pure_registration(registration),
                read_paths,
                eval,
            ))),
        }
    }

    pub fn eq_expr<T>(
        name: &'static str,
        lhs: GuardValueExpr<S, A, T>,
        rhs: GuardValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Eq(GuardComparison::from_exprs(
                ComparisonOp::Eq,
                lhs,
                rhs,
                move |prev, action| lhs_eval.eval(prev, action) == rhs_eval.eval(prev, action),
            ))),
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
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        Self::eq_expr(
            name,
            GuardValueExpr::opaque(lhs, lhs, lhs_eval),
            GuardValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn ne_expr<T>(
        name: &'static str,
        lhs: GuardValueExpr<S, A, T>,
        rhs: GuardValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Ne(GuardComparison::from_exprs(
                ComparisonOp::Ne,
                lhs,
                rhs,
                move |prev, action| lhs_eval.eval(prev, action) != rhs_eval.eval(prev, action),
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
        S: Clone,
        A: Clone,
        T: PartialEq + 'static + Clone,
    {
        Self::ne_expr(
            name,
            GuardValueExpr::opaque(lhs, lhs, lhs_eval),
            GuardValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn lt_expr<T>(
        name: &'static str,
        lhs: GuardValueExpr<S, A, T>,
        rhs: GuardValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Lt(GuardComparison::from_exprs(
                ComparisonOp::Lt,
                lhs,
                rhs,
                move |prev, action| lhs_eval.eval(prev, action) < rhs_eval.eval(prev, action),
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
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::lt_expr(
            name,
            GuardValueExpr::opaque(lhs, lhs, lhs_eval),
            GuardValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn le_expr<T>(
        name: &'static str,
        lhs: GuardValueExpr<S, A, T>,
        rhs: GuardValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Le(GuardComparison::from_exprs(
                ComparisonOp::Le,
                lhs,
                rhs,
                move |prev, action| lhs_eval.eval(prev, action) <= rhs_eval.eval(prev, action),
            ))),
        }
    }

    pub fn le<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::le_expr(
            name,
            GuardValueExpr::opaque(lhs, lhs, lhs_eval),
            GuardValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn gt_expr<T>(
        name: &'static str,
        lhs: GuardValueExpr<S, A, T>,
        rhs: GuardValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Gt(GuardComparison::from_exprs(
                ComparisonOp::Gt,
                lhs,
                rhs,
                move |prev, action| lhs_eval.eval(prev, action) > rhs_eval.eval(prev, action),
            ))),
        }
    }

    pub fn gt<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::gt_expr(
            name,
            GuardValueExpr::opaque(lhs, lhs, lhs_eval),
            GuardValueExpr::opaque(rhs, rhs, rhs_eval),
        )
    }

    pub fn ge_expr<T>(
        name: &'static str,
        lhs: GuardValueExpr<S, A, T>,
        rhs: GuardValueExpr<S, A, T>,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        let lhs_eval = lhs.clone();
        let rhs_eval = rhs.clone();
        Self {
            name,
            body: GuardExprBody::Ast(GuardAst::Ge(GuardComparison::from_exprs(
                ComparisonOp::Ge,
                lhs,
                rhs,
                move |prev, action| lhs_eval.eval(prev, action) >= rhs_eval.eval(prev, action),
            ))),
        }
    }

    pub fn ge<T>(
        name: &'static str,
        lhs: &'static str,
        lhs_eval: fn(&S, &A) -> T,
        rhs: &'static str,
        rhs_eval: fn(&S, &A) -> T,
    ) -> Self
    where
        S: Clone,
        A: Clone,
        T: PartialOrd + 'static + Clone,
    {
        Self::ge_expr(
            name,
            GuardValueExpr::opaque(lhs, lhs, lhs_eval),
            GuardValueExpr::opaque(rhs, rhs, rhs_eval),
        )
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

    pub fn first_unencodable_symbolic_node(&self) -> Option<&'static str> {
        self.ast().and_then(GuardAst::first_unencodable)
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_symbolic_pure_keys(keys);
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_unregistered_symbolic_pure_keys(keys);
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        if let Some(ast) = self.ast() {
            ast.collect_symbolic_state_paths(paths);
        }
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        let mut paths = BTreeSet::new();
        self.collect_symbolic_state_paths(&mut paths);
        paths.into_iter().collect()
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
pub enum UpdateValueExprAst<S, A> {
    Opaque {
        repr: &'static str,
    },
    Literal {
        repr: &'static str,
    },
    FieldRead {
        path: &'static str,
    },
    PureCall {
        name: &'static str,
        symbolic: SymbolicRegistration,
        read_paths: &'static [&'static str],
    },
    Add {
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    _Phantom(PhantomData<fn(&S, &A)>),
}

impl<S, A> UpdateValueExprAst<S, A> {
    pub const fn opaque(repr: &'static str) -> Self {
        Self::Opaque { repr }
    }

    pub const fn literal(repr: &'static str) -> Self {
        Self::Literal { repr }
    }

    pub const fn field(path: &'static str) -> Self {
        Self::FieldRead { path }
    }

    pub const fn builtin_pure_call(name: &'static str) -> Self {
        Self::builtin_pure_call_with_paths(name, &[])
    }

    pub const fn builtin_pure_call_with_paths(
        name: &'static str,
        read_paths: &'static [&'static str],
    ) -> Self {
        Self::PureCall {
            name,
            symbolic: SymbolicRegistration::Builtin,
            read_paths,
        }
    }

    pub fn registered_pure_call(name: &'static str, registration: &'static str) -> Self {
        Self::registered_pure_call_with_paths(name, registration, &[])
    }

    pub fn registered_pure_call_with_paths(
        name: &'static str,
        registration: &'static str,
        read_paths: &'static [&'static str],
    ) -> Self {
        Self::PureCall {
            name,
            symbolic: symbolic_pure_registration(registration),
            read_paths,
        }
    }

    pub fn add(lhs: Self, rhs: Self) -> Self {
        Self::Add {
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Opaque { repr } => Some(repr),
            Self::Literal { .. } | Self::FieldRead { .. } | Self::_Phantom(_) => None,
            Self::PureCall { symbolic, .. } => symbolic.first_unencodable(),
            Self::Add { lhs, rhs } => lhs.first_unencodable().or_else(|| rhs.first_unencodable()),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. }
            | Self::Literal { .. }
            | Self::FieldRead { .. }
            | Self::_Phantom(_) => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_pure_keys(keys);
                rhs.collect_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. }
            | Self::Literal { .. }
            | Self::FieldRead { .. }
            | Self::_Phantom(_) => {}
            Self::PureCall { symbolic, .. } => symbolic.collect_unregistered_key(keys),
            Self::Add { lhs, rhs } => {
                lhs.collect_unregistered_symbolic_pure_keys(keys);
                rhs.collect_unregistered_symbolic_pure_keys(keys);
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Opaque { .. } | Self::Literal { .. } | Self::_Phantom(_) => {}
            Self::FieldRead { path } => collect_symbolic_state_path(paths, path),
            Self::PureCall { read_paths, .. } => {
                collect_symbolic_state_paths_from_hints(paths, read_paths);
            }
            Self::Add { lhs, rhs } => {
                lhs.collect_symbolic_state_paths(paths);
                rhs.collect_symbolic_state_paths(paths);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum UpdateOp<S, A> {
    Assign {
        target: &'static str,
        value_ast: UpdateValueExprAst<S, A>,
        apply: fn(&S, &mut S, &A),
    },
    SetInsert {
        target: &'static str,
        item_ast: UpdateValueExprAst<S, A>,
        apply: fn(&S, &mut S, &A),
    },
    SetRemove {
        target: &'static str,
        item_ast: UpdateValueExprAst<S, A>,
        apply: fn(&S, &mut S, &A),
    },
    Effect {
        name: &'static str,
        symbolic: SymbolicRegistration,
        apply: fn(&S, &mut S, &A),
    },
}

impl<S, A> UpdateOp<S, A> {
    pub const fn assign(
        target: &'static str,
        value: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::assign_ast(target, UpdateValueExprAst::Opaque { repr: value }, apply)
    }

    pub const fn assign_ast(
        target: &'static str,
        value_ast: UpdateValueExprAst<S, A>,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::Assign {
            target,
            value_ast,
            apply,
        }
    }

    pub const fn set_insert(
        target: &'static str,
        item: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::set_insert_ast(target, UpdateValueExprAst::Opaque { repr: item }, apply)
    }

    pub const fn set_insert_ast(
        target: &'static str,
        item_ast: UpdateValueExprAst<S, A>,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::SetInsert {
            target,
            item_ast,
            apply,
        }
    }

    pub const fn set_remove(
        target: &'static str,
        item: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::set_remove_ast(target, UpdateValueExprAst::Opaque { repr: item }, apply)
    }

    pub const fn set_remove_ast(
        target: &'static str,
        item_ast: UpdateValueExprAst<S, A>,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::SetRemove {
            target,
            item_ast,
            apply,
        }
    }

    pub const fn effect(name: &'static str, apply: fn(&S, &mut S, &A)) -> Self {
        Self::Effect {
            name,
            symbolic: SymbolicRegistration::Unregistered(name),
            apply,
        }
    }

    pub fn registered_effect(
        name: &'static str,
        registration: &'static str,
        apply: fn(&S, &mut S, &A),
    ) -> Self {
        Self::Effect {
            name,
            symbolic: symbolic_effect_registration(registration),
            apply,
        }
    }

    fn apply(&self, prev: &S, state: &mut S, action: &A) {
        match self {
            Self::Assign { apply, .. }
            | Self::SetInsert { apply, .. }
            | Self::SetRemove { apply, .. }
            | Self::Effect { apply, .. } => apply(prev, state, action),
        }
    }

    fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Assign { value_ast, .. } => value_ast.first_unencodable(),
            Self::SetInsert { item_ast, .. } | Self::SetRemove { item_ast, .. } => {
                item_ast.first_unencodable()
            }
            Self::Effect { symbolic, .. } => symbolic.first_unencodable(),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Assign { value_ast, .. } => value_ast.collect_symbolic_pure_keys(keys),
            Self::SetInsert { item_ast, .. } | Self::SetRemove { item_ast, .. } => {
                item_ast.collect_symbolic_pure_keys(keys);
            }
            Self::Effect { .. } => {}
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Assign { value_ast, .. } => {
                value_ast.collect_unregistered_symbolic_pure_keys(keys)
            }
            Self::SetInsert { item_ast, .. } | Self::SetRemove { item_ast, .. } => {
                item_ast.collect_unregistered_symbolic_pure_keys(keys);
            }
            Self::Effect { .. } => {}
        }
    }

    fn collect_symbolic_effect_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Self::Effect { symbolic, .. } = self {
            symbolic.collect_key(keys);
        }
    }

    fn collect_unregistered_symbolic_effect_keys(&self, keys: &mut BTreeSet<&'static str>) {
        if let Self::Effect { symbolic, .. } = self {
            symbolic.collect_unregistered_key(keys);
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Assign { value_ast, .. } => value_ast.collect_symbolic_state_paths(paths),
            Self::SetInsert { item_ast, .. } | Self::SetRemove { item_ast, .. } => {
                item_ast.collect_symbolic_state_paths(paths);
            }
            Self::Effect { .. } => {}
        }
    }

    fn effect_name(&self) -> Option<&'static str> {
        match self {
            Self::Effect { name, .. } => Some(name),
            Self::Assign { .. } | Self::SetInsert { .. } | Self::SetRemove { .. } => None,
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

    fn first_unencodable(&self) -> Option<&'static str> {
        match self {
            Self::Sequence(ops) => ops.iter().find_map(UpdateOp::first_unencodable),
        }
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Sequence(ops) => {
                for op in ops {
                    op.collect_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Sequence(ops) => {
                for op in ops {
                    op.collect_unregistered_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_symbolic_effect_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Sequence(ops) => {
                for op in ops {
                    op.collect_symbolic_effect_keys(keys);
                }
            }
        }
    }

    fn collect_unregistered_symbolic_effect_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match self {
            Self::Sequence(ops) => {
                for op in ops {
                    op.collect_unregistered_symbolic_effect_keys(keys);
                }
            }
        }
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::Sequence(ops) => {
                for op in ops {
                    op.collect_symbolic_state_paths(paths);
                }
            }
        }
    }

    fn collect_effect_names(&self, names: &mut BTreeSet<&'static str>) {
        match self {
            Self::Sequence(ops) => {
                for op in ops {
                    if let Some(name) = op.effect_name() {
                        names.insert(name);
                    }
                }
            }
        }
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        let mut paths = BTreeSet::new();
        self.collect_symbolic_state_paths(&mut paths);
        paths.into_iter().collect()
    }

    pub fn effect_names(&self) -> Vec<&'static str> {
        let mut names = BTreeSet::new();
        self.collect_effect_names(&mut names);
        names.into_iter().collect()
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionSuccessor<S> {
    rule_name: &'static str,
    next: S,
}

impl<S> TransitionSuccessor<S> {
    pub const fn new(rule_name: &'static str, next: S) -> Self {
        Self { rule_name, next }
    }

    pub const fn rule_name(&self) -> &'static str {
        self.rule_name
    }

    pub fn next(&self) -> &S {
        &self.next
    }

    pub fn into_next(self) -> S {
        self.next
    }
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

    pub fn first_unencodable_symbolic_node(&self) -> Option<&'static str> {
        match &self.body {
            TransitionRuleBody::Guarded { guard, update } => guard
                .first_unencodable_symbolic_node()
                .or_else(|| update.ast_body().and_then(UpdateAst::first_unencodable)),
        }
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        let mut paths = BTreeSet::new();
        if let Some(ast) = self.guard_ast() {
            ast.collect_symbolic_state_paths(&mut paths);
        }
        if let Some(ast) = self.update_ast() {
            ast.collect_symbolic_state_paths(&mut paths);
        }
        paths.into_iter().collect()
    }

    pub fn effect_names(&self) -> Vec<&'static str> {
        self.update_ast()
            .map(UpdateAst::effect_names)
            .unwrap_or_default()
    }

    fn collect_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match &self.body {
            TransitionRuleBody::Guarded { guard, update } => {
                guard.collect_symbolic_pure_keys(keys);
                if let Some(ast) = update.ast_body() {
                    ast.collect_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_unregistered_symbolic_pure_keys(&self, keys: &mut BTreeSet<&'static str>) {
        match &self.body {
            TransitionRuleBody::Guarded { guard, update } => {
                guard.collect_unregistered_symbolic_pure_keys(keys);
                if let Some(ast) = update.ast_body() {
                    ast.collect_unregistered_symbolic_pure_keys(keys);
                }
            }
        }
    }

    fn collect_symbolic_effect_keys(&self, keys: &mut BTreeSet<&'static str>) {
        let TransitionRuleBody::Guarded { update, .. } = &self.body;
        if let Some(ast) = update.ast_body() {
            ast.collect_symbolic_effect_keys(keys);
        }
    }

    fn collect_unregistered_symbolic_effect_keys(&self, keys: &mut BTreeSet<&'static str>) {
        let TransitionRuleBody::Guarded { update, .. } = &self.body;
        if let Some(ast) = update.ast_body() {
            ast.collect_unregistered_symbolic_effect_keys(keys);
        }
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

    pub fn first_unencodable_symbolic_node(&self) -> Option<&'static str> {
        self.rules
            .iter()
            .find_map(TransitionRule::first_unencodable_symbolic_node)
    }

    pub fn symbolic_pure_helper_keys(&self) -> Vec<&'static str> {
        let mut keys = BTreeSet::new();
        for rule in &self.rules {
            rule.collect_symbolic_pure_keys(&mut keys);
        }
        keys.into_iter().collect()
    }

    pub fn unregistered_symbolic_pure_helper_keys(&self) -> Vec<&'static str> {
        let mut keys = BTreeSet::new();
        for rule in &self.rules {
            rule.collect_unregistered_symbolic_pure_keys(&mut keys);
        }
        keys.into_iter().collect()
    }

    pub fn symbolic_effect_keys(&self) -> Vec<&'static str> {
        let mut keys = BTreeSet::new();
        for rule in &self.rules {
            rule.collect_symbolic_effect_keys(&mut keys);
        }
        keys.into_iter().collect()
    }

    pub fn unregistered_symbolic_effect_keys(&self) -> Vec<&'static str> {
        let mut keys = BTreeSet::new();
        for rule in &self.rules {
            rule.collect_unregistered_symbolic_effect_keys(&mut keys);
        }
        keys.into_iter().collect()
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        let mut paths = BTreeSet::new();
        for rule in &self.rules {
            for path in rule.symbolic_state_paths() {
                paths.insert(path);
            }
        }
        paths.into_iter().collect()
    }

    pub fn effect_names(&self) -> Vec<&'static str> {
        let mut names = BTreeSet::new();
        for rule in &self.rules {
            for name in rule.effect_names() {
                names.insert(name);
            }
        }
        names.into_iter().collect()
    }

    pub fn successors(&self, prev: &S, action: &A) -> Vec<TransitionSuccessor<S>>
    where
        S: Clone,
    {
        self.rules
            .iter()
            .filter(|rule| rule.matches(prev, action))
            .map(|rule| TransitionSuccessor::new(rule.name(), rule.apply(prev, action)))
            .collect()
    }

    pub fn evaluate(&self, prev: &S, action: &A) -> Result<Option<S>, TransitionProgramError>
    where
        S: Clone,
    {
        let matches = self.successors(prev, action);
        let Some(first) = matches.first() else {
            return Ok(None);
        };

        if matches.len() > 1 {
            return Err(TransitionProgramError::AmbiguousRuleMatch {
                program: self.name,
                rule_names: matches.iter().map(TransitionSuccessor::rule_name).collect(),
            });
        }

        Ok(Some(first.next().clone()))
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
        UpdateValueExprAst,
    };

    crate::register_symbolic_pure_helpers!("predicate_tests::registered_helper");
    crate::register_symbolic_effects!("predicate_tests::registered_effect");

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
    fn state_expr_exposes_conditional_ast() {
        let expr = StateExpr::if_else(
            "next_or_current",
            BoolExpr::field("ready", "state.ready", |state: &Worker| state.ready),
            StateExpr::field("next_state", "state.next_state", |state: &Worker| {
                state.next_state
            }),
            StateExpr::field("state", "state.state", |state: &Worker| state.state),
        );
        let worker = Worker {
            ready: true,
            count: 0,
            state: State::Idle,
            next_state: State::Busy,
        };

        match expr.ast() {
            Some(StateExprAst::IfElse {
                condition,
                then_branch,
                else_branch,
                ..
            }) => {
                assert!(condition.eval(&worker));
                assert!(matches!(
                    then_branch.as_ref(),
                    StateExprAst::FieldRead {
                        path: "state.next_state",
                        ..
                    }
                ));
                assert!(matches!(
                    else_branch.as_ref(),
                    StateExprAst::FieldRead {
                        path: "state.state",
                        ..
                    }
                ));
            }
            other => panic!("unexpected ast: {other:?}"),
        }
        assert_eq!(expr.eval(&worker), State::Busy);
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
    fn bool_expr_supports_extended_comparisons() {
        let worker = Worker {
            ready: true,
            count: 2,
            state: State::Busy,
            next_state: State::Idle,
        };
        let expr = BoolExpr::and(
            "comparison_suite",
            vec![
                BoolExpr::le(
                    "count_le_two",
                    "state.count",
                    |state: &Worker| state.count,
                    "2",
                    |_state: &Worker| 2,
                ),
                BoolExpr::gt(
                    "count_gt_one",
                    "state.count",
                    |state: &Worker| state.count,
                    "1",
                    |_state: &Worker| 1,
                ),
                BoolExpr::ge(
                    "count_ge_two",
                    "state.count",
                    |state: &Worker| state.count,
                    "2",
                    |_state: &Worker| 2,
                ),
            ],
        );

        assert!(expr.eval(&worker));
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
    fn step_expr_supports_extended_comparisons() {
        let expr = StepExpr::and(
            "step_comparison_suite",
            vec![
                StepExpr::le(
                    "prev_count_le_next_count",
                    "prev.count",
                    |prev: &Worker, _action: &Action, _next: &Worker| prev.count,
                    "next.count",
                    |_prev: &Worker, _action: &Action, next: &Worker| next.count,
                ),
                StepExpr::gt(
                    "next_count_gt_zero",
                    "next.count",
                    |_prev: &Worker, _action: &Action, next: &Worker| next.count,
                    "0",
                    |_prev: &Worker, _action: &Action, _next: &Worker| 0,
                ),
                StepExpr::ge(
                    "next_count_ge_prev_count",
                    "next.count",
                    |_prev: &Worker, _action: &Action, next: &Worker| next.count,
                    "prev.count",
                    |prev: &Worker, _action: &Action, _next: &Worker| prev.count,
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
    fn update_program_preserves_state_so_far_semantics() {
        let update = UpdateProgram::ast(
            "state_so_far",
            vec![
                UpdateOp::assign_ast(
                    "count",
                    UpdateValueExprAst::literal("1"),
                    |_prev: &Worker, state: &mut Worker, _action: &Action| {
                        state.count = 1;
                    },
                ),
                UpdateOp::assign_ast(
                    "ready",
                    UpdateValueExprAst::registered_pure_call(
                        "count_positive(state.count)",
                        "predicate_tests::registered_helper",
                    ),
                    |_prev: &Worker, state: &mut Worker, _action: &Action| {
                        state.ready = state.count > 0;
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
        assert_eq!(next.count, 1);
        assert!(next.ready);
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
    fn transition_program_exposes_relation_successors() {
        let program = TransitionProgram::named(
            "relation_worker",
            vec![
                TransitionRule::ast(
                    "start_a",
                    GuardExpr::pure_call("start_a", |state, action| {
                        matches!((state, action), (State::Idle, Action::Start))
                    }),
                    UpdateProgram::ast(
                        "to_busy",
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
                        "to_idle",
                        vec![UpdateOp::assign(
                            "self",
                            "State::Idle",
                            |_prev: &State, state: &mut State, _action: &Action| {
                                *state = State::Idle;
                            },
                        )],
                    ),
                ),
            ],
        );

        let successors = program.successors(&State::Idle, &Action::Start);
        assert_eq!(
            successors
                .iter()
                .map(|successor| successor.rule_name())
                .collect::<Vec<_>>(),
            vec!["start_a", "start_b"]
        );
        assert_eq!(
            successors
                .into_iter()
                .map(|successor| successor.into_next())
                .collect::<Vec<_>>(),
            vec![State::Busy, State::Idle]
        );
    }

    #[test]
    fn transition_program_reports_first_unencodable_symbolic_node() {
        let helper_program = TransitionProgram::named(
            "missing_helper",
            vec![TransitionRule::ast(
                "missing_helper_rule",
                GuardExpr::registered_pure_call(
                    "missing_helper(prev, action)",
                    "predicate_tests::missing_helper",
                    |_state: &State, _action: &Action| true,
                ),
                UpdateProgram::ast("noop", vec![]),
            )],
        );
        assert_eq!(
            helper_program.first_unencodable_symbolic_node(),
            Some("predicate_tests::missing_helper")
        );
        assert_eq!(
            helper_program.symbolic_pure_helper_keys(),
            vec!["predicate_tests::missing_helper"]
        );
        assert_eq!(
            helper_program.unregistered_symbolic_pure_helper_keys(),
            vec!["predicate_tests::missing_helper"]
        );

        let effect_program = TransitionProgram::named(
            "missing_effect",
            vec![TransitionRule::ast(
                "missing_effect_rule",
                GuardExpr::matches_variant(
                    "always_idle",
                    "prev",
                    "State::Idle",
                    |state: &State, _action: &Action| matches!(state, State::Idle),
                ),
                UpdateProgram::ast(
                    "missing_effect_update",
                    vec![UpdateOp::registered_effect(
                        "missing_effect()",
                        "predicate_tests::missing_effect",
                        |_prev: &State, _state: &mut State, _action: &Action| {},
                    )],
                ),
            )],
        );
        assert_eq!(
            effect_program.first_unencodable_symbolic_node(),
            Some("predicate_tests::missing_effect")
        );
        assert_eq!(
            effect_program.symbolic_effect_keys(),
            vec!["predicate_tests::missing_effect"]
        );
        assert_eq!(
            effect_program.unregistered_symbolic_effect_keys(),
            vec!["predicate_tests::missing_effect"]
        );
    }

    #[test]
    fn transition_program_accepts_registered_symbolic_nodes() {
        let program = TransitionProgram::named(
            "registered_nodes",
            vec![TransitionRule::ast(
                "registered_rule",
                GuardExpr::registered_pure_call(
                    "registered_helper(prev, action)",
                    "predicate_tests::registered_helper",
                    |_state: &State, _action: &Action| true,
                ),
                UpdateProgram::ast(
                    "registered_update",
                    vec![
                        UpdateOp::assign_ast(
                            "self",
                            UpdateValueExprAst::registered_pure_call(
                                "registered_next_state(prev)",
                                "predicate_tests::registered_helper",
                            ),
                            |_prev: &State, state: &mut State, _action: &Action| {
                                *state = State::Busy;
                            },
                        ),
                        UpdateOp::registered_effect(
                            "registered_effect()",
                            "predicate_tests::registered_effect",
                            |_prev: &State, _state: &mut State, _action: &Action| {},
                        ),
                    ],
                ),
            )],
        );

        assert_eq!(program.first_unencodable_symbolic_node(), None);
        assert_eq!(
            program.symbolic_pure_helper_keys(),
            vec!["predicate_tests::registered_helper"]
        );
        assert_eq!(
            program.symbolic_effect_keys(),
            vec!["predicate_tests::registered_effect"]
        );
        assert!(program.unregistered_symbolic_pure_helper_keys().is_empty());
        assert!(program.unregistered_symbolic_effect_keys().is_empty());
    }

    #[test]
    fn transition_program_collects_pure_call_read_paths() {
        let program = TransitionProgram::named(
            "pure_call_paths",
            vec![TransitionRule::ast(
                "pure_rule",
                GuardExpr::builtin_pure_call_with_paths(
                    "ready.clone()",
                    &["prev.ready"],
                    |state: &Worker, _action: &Action| state.ready,
                ),
                UpdateProgram::ast(
                    "copy_phase",
                    vec![UpdateOp::assign_ast(
                        "state",
                        UpdateValueExprAst::builtin_pure_call_with_paths(
                            "prev.state.clone()",
                            &["prev.state"],
                        ),
                        |prev: &Worker, state: &mut Worker, _action: &Action| {
                            state.state = prev.state;
                        },
                    )],
                ),
            )],
        );

        assert_eq!(program.symbolic_state_paths(), vec!["ready", "state"]);
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
