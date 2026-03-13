use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VarDecl {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Definition {
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantifierKind {
    ForAll,
    Exists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComparisonOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuiltinPredicateOp {
    Contains,
    SubsetOf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ViewExpr {
    #[default]
    Vars,
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ValueExpr {
    #[default]
    Unit,
    Opaque(String),
    Literal(String),
    Field(String),
    PureCall {
        name: String,
        read_paths: Vec<String>,
        symbolic_key: Option<String>,
    },
    Add(Box<ValueExpr>, Box<ValueExpr>),
    Sub(Box<ValueExpr>, Box<ValueExpr>),
    Mul(Box<ValueExpr>, Box<ValueExpr>),
    Neg(Box<ValueExpr>),
    Union(Box<ValueExpr>, Box<ValueExpr>),
    Intersection(Box<ValueExpr>, Box<ValueExpr>),
    Difference(Box<ValueExpr>, Box<ValueExpr>),
    SequenceUpdate {
        base: Box<ValueExpr>,
        index: Box<ValueExpr>,
        value: Box<ValueExpr>,
    },
    FunctionUpdate {
        base: Box<ValueExpr>,
        key: Box<ValueExpr>,
        value: Box<ValueExpr>,
    },
    RecordUpdate {
        base: Box<ValueExpr>,
        field: String,
        value: Box<ValueExpr>,
    },
    Comprehension {
        domain: String,
        body: String,
        read_paths: Vec<String>,
    },
    Conditional {
        condition: String,
        then_branch: Box<ValueExpr>,
        else_branch: Box<ValueExpr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateOpDecl {
    Assign {
        target: String,
        value: ValueExpr,
    },
    SetInsert {
        target: String,
        item: ValueExpr,
    },
    SetRemove {
        target: String,
        item: ValueExpr,
    },
    Effect {
        name: String,
        symbolic_key: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateExpr {
    Sequence(Vec<UpdateOpDecl>),
    Choice {
        domain: String,
        body: String,
        read_paths: Vec<String>,
        write_paths: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StateExpr {
    #[default]
    True,
    False,
    Var(String),
    Ref(String),
    Const(String),
    Eq(Box<StateExpr>, Box<StateExpr>),
    In(Box<StateExpr>, Box<StateExpr>),
    Not(Box<StateExpr>),
    And(Vec<StateExpr>),
    Or(Vec<StateExpr>),
    Implies(Box<StateExpr>, Box<StateExpr>),
    Compare {
        op: ComparisonOp,
        lhs: ValueExpr,
        rhs: ValueExpr,
    },
    Builtin {
        op: BuiltinPredicateOp,
        lhs: ValueExpr,
        rhs: ValueExpr,
    },
    Match {
        value: String,
        pattern: String,
    },
    Quantified {
        kind: QuantifierKind,
        domain: String,
        body: String,
        read_paths: Vec<String>,
        symbolic_supported: bool,
    },
    Forall(Vec<String>, Box<StateExpr>),
    Exists(Vec<String>, Box<StateExpr>),
    Choose(String, Box<StateExpr>),
    Opaque(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ActionExpr {
    #[default]
    True,
    False,
    Ref(String),
    Pred(StateExpr),
    Unchanged(Vec<String>),
    And(Vec<ActionExpr>),
    Or(Vec<ActionExpr>),
    Implies(Box<ActionExpr>, Box<ActionExpr>),
    Exists(Vec<String>, Box<ActionExpr>),
    Enabled(Box<ActionExpr>),
    Compare {
        op: ComparisonOp,
        lhs: ValueExpr,
        rhs: ValueExpr,
    },
    Builtin {
        op: BuiltinPredicateOp,
        lhs: ValueExpr,
        rhs: ValueExpr,
    },
    Match {
        value: String,
        pattern: String,
    },
    Quantified {
        kind: QuantifierKind,
        domain: String,
        body: String,
        read_paths: Vec<String>,
        symbolic_supported: bool,
    },
    Rule {
        name: String,
        guard: Box<ActionExpr>,
        update: UpdateExpr,
    },
    BoxAction {
        action: Box<ActionExpr>,
        view: ViewExpr,
    },
    AngleAction {
        action: Box<ActionExpr>,
        view: ViewExpr,
    },
    Opaque(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemporalExpr {
    State(StateExpr),
    Action(ActionExpr),
    Ref(String),
    Not(Box<TemporalExpr>),
    And(Vec<TemporalExpr>),
    Or(Vec<TemporalExpr>),
    Implies(Box<TemporalExpr>, Box<TemporalExpr>),
    Next(Box<TemporalExpr>),
    Always(Box<TemporalExpr>),
    Eventually(Box<TemporalExpr>),
    Until(Box<TemporalExpr>, Box<TemporalExpr>),
    LeadsTo(Box<TemporalExpr>, Box<TemporalExpr>),
    Enabled(ActionExpr),
    Opaque(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FairnessDecl {
    WF { view: ViewExpr, action: ActionExpr },
    SF { view: ViewExpr, action: ActionExpr },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SpecCore {
    pub vars: Vec<VarDecl>,
    pub defs: Vec<Definition>,
    pub init: StateExpr,
    pub next: ActionExpr,
    pub fairness: Vec<FairnessDecl>,
    pub invariants: Vec<StateExpr>,
    pub temporal_props: Vec<TemporalExpr>,
}

impl SpecCore {
    pub fn named(frontend_name: &'static str) -> Self {
        Self::opaque(frontend_name)
    }

    pub fn opaque(frontend_name: &'static str) -> Self {
        Self {
            vars: vec![VarDecl {
                name: "state".to_owned(),
            }],
            defs: vec![Definition {
                name: "frontend".to_owned(),
                body: frontend_name.to_owned(),
            }],
            init: StateExpr::Opaque("init".to_owned()),
            next: ActionExpr::BoxAction {
                action: Box::new(ActionExpr::Opaque("next".to_owned())),
                view: ViewExpr::Vars,
            },
            fairness: Vec::new(),
            invariants: Vec::new(),
            temporal_props: Vec::new(),
        }
    }
}
