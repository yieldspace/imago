use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofObligationKind {
    InitImpliesInvariant,
    StepPreservesInvariant,
    SymmetryReduction,
    StateQuotientReduction,
    PorReduction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofObligation {
    pub label: String,
    pub kind: ProofObligationKind,
    pub tla_theorem: String,
    pub smtlib: String,
}

impl ProofObligation {
    pub fn new(
        label: impl Into<String>,
        kind: ProofObligationKind,
        tla_theorem: impl Into<String>,
        smtlib: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            kind,
            tla_theorem: tla_theorem.into(),
            smtlib: smtlib.into(),
        }
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FragmentProfile {
    pub has_opaque_nodes: bool,
    pub has_stringly_nodes: bool,
    pub has_stringly_quantifiers: bool,
    pub has_temporal_props: bool,
    pub has_fairness: bool,
    pub symbolic_supported: bool,
    pub proof_supported: bool,
}

impl FragmentProfile {
    pub fn symbolic_unsupported_reasons(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.has_opaque_nodes {
            reasons.push("opaque nodes");
        }
        if self.has_stringly_quantifiers {
            reasons.push("stringly quantifiers");
        }
        reasons
    }

    pub fn proof_unsupported_reasons(&self) -> Vec<&'static str> {
        let mut reasons = self.symbolic_unsupported_reasons();
        if self.has_temporal_props {
            reasons.push("temporal properties");
        }
        if self.has_fairness {
            reasons.push("fairness obligations");
        }
        reasons
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreNormalizationError {
    EmptyVariableName { index: usize },
    EmptyDefinitionName { index: usize },
}

impl std::fmt::Display for CoreNormalizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyVariableName { index } => {
                write!(f, "spec core variable at index {index} must not be empty")
            }
            Self::EmptyDefinitionName { index } => {
                write!(f, "spec core definition at index {index} must not be empty")
            }
        }
    }
}

impl std::error::Error for CoreNormalizationError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedSpecCore {
    pub core: SpecCore,
    pub fragment_profile: FragmentProfile,
}

impl NormalizedSpecCore {
    pub fn core(&self) -> &SpecCore {
        &self.core
    }

    pub fn fragment_profile(&self) -> &FragmentProfile {
        &self.fragment_profile
    }
}

impl AsRef<SpecCore> for NormalizedSpecCore {
    fn as_ref(&self) -> &SpecCore {
        self.core()
    }
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

    pub fn normalize(&self) -> Result<NormalizedSpecCore, CoreNormalizationError> {
        for (index, decl) in self.vars.iter().enumerate() {
            if decl.name.is_empty() {
                return Err(CoreNormalizationError::EmptyVariableName { index });
            }
        }
        for (index, def) in self.defs.iter().enumerate() {
            if def.name.is_empty() {
                return Err(CoreNormalizationError::EmptyDefinitionName { index });
            }
        }

        let mut fragment_profile = FragmentProfile {
            has_temporal_props: !self.temporal_props.is_empty(),
            has_fairness: !self.fairness.is_empty(),
            ..FragmentProfile::default()
        };
        classify_state_expr(&self.init, &mut fragment_profile);
        classify_action_expr(&self.next, &mut fragment_profile);
        for fairness in &self.fairness {
            classify_fairness_decl(fairness, &mut fragment_profile);
        }
        for invariant in &self.invariants {
            classify_state_expr(invariant, &mut fragment_profile);
        }
        for property in &self.temporal_props {
            classify_temporal_expr(property, &mut fragment_profile);
        }

        fragment_profile.symbolic_supported =
            !fragment_profile.has_opaque_nodes && !fragment_profile.has_stringly_quantifiers;
        fragment_profile.proof_supported = fragment_profile.symbolic_supported
            && !fragment_profile.has_temporal_props
            && !fragment_profile.has_fairness;

        Ok(NormalizedSpecCore {
            core: self.clone(),
            fragment_profile,
        })
    }
}

fn classify_value_expr(expr: &ValueExpr, profile: &mut FragmentProfile) {
    match expr {
        ValueExpr::Opaque(_) => profile.has_opaque_nodes = true,
        ValueExpr::Add(lhs, rhs)
        | ValueExpr::Sub(lhs, rhs)
        | ValueExpr::Mul(lhs, rhs)
        | ValueExpr::Union(lhs, rhs)
        | ValueExpr::Intersection(lhs, rhs)
        | ValueExpr::Difference(lhs, rhs) => {
            classify_value_expr(lhs, profile);
            classify_value_expr(rhs, profile);
        }
        ValueExpr::Neg(value) => classify_value_expr(value, profile),
        ValueExpr::SequenceUpdate { base, index, value }
        | ValueExpr::FunctionUpdate {
            base,
            key: index,
            value,
        } => {
            classify_value_expr(base, profile);
            classify_value_expr(index, profile);
            classify_value_expr(value, profile);
        }
        ValueExpr::RecordUpdate { base, value, .. } => {
            classify_value_expr(base, profile);
            classify_value_expr(value, profile);
        }
        ValueExpr::Comprehension { .. } | ValueExpr::Conditional { .. } => {
            profile.has_stringly_nodes = true;
            if let ValueExpr::Conditional {
                then_branch,
                else_branch,
                ..
            } = expr
            {
                classify_value_expr(then_branch, profile);
                classify_value_expr(else_branch, profile);
            }
        }
        ValueExpr::Unit
        | ValueExpr::Literal(_)
        | ValueExpr::Field(_)
        | ValueExpr::PureCall { .. } => {}
    }
}

fn classify_update_expr(expr: &UpdateExpr, profile: &mut FragmentProfile) {
    match expr {
        UpdateExpr::Sequence(ops) => {
            for op in ops {
                classify_update_op(op, profile);
            }
        }
        UpdateExpr::Choice { .. } => profile.has_stringly_nodes = true,
    }
}

fn classify_update_op(op: &UpdateOpDecl, profile: &mut FragmentProfile) {
    match op {
        UpdateOpDecl::Assign { value, .. }
        | UpdateOpDecl::SetInsert { item: value, .. }
        | UpdateOpDecl::SetRemove { item: value, .. } => classify_value_expr(value, profile),
        UpdateOpDecl::Effect { .. } => {}
    }
}

fn classify_state_expr(expr: &StateExpr, profile: &mut FragmentProfile) {
    match expr {
        StateExpr::Eq(lhs, rhs) | StateExpr::In(lhs, rhs) => {
            classify_state_expr(lhs, profile);
            classify_state_expr(rhs, profile);
        }
        StateExpr::Not(value) => classify_state_expr(value, profile),
        StateExpr::And(values) | StateExpr::Or(values) => {
            for value in values {
                classify_state_expr(value, profile);
            }
        }
        StateExpr::Implies(lhs, rhs) => {
            classify_state_expr(lhs, profile);
            classify_state_expr(rhs, profile);
        }
        StateExpr::Compare { lhs, rhs, .. } | StateExpr::Builtin { lhs, rhs, .. } => {
            classify_value_expr(lhs, profile);
            classify_value_expr(rhs, profile);
        }
        StateExpr::Match { .. } => profile.has_stringly_nodes = true,
        StateExpr::Quantified { .. } => {
            profile.has_stringly_nodes = true;
            profile.has_stringly_quantifiers = true;
        }
        StateExpr::Forall(_, body) | StateExpr::Exists(_, body) | StateExpr::Choose(_, body) => {
            classify_state_expr(body, profile);
        }
        StateExpr::Opaque(_) => profile.has_opaque_nodes = true,
        StateExpr::True
        | StateExpr::False
        | StateExpr::Var(_)
        | StateExpr::Ref(_)
        | StateExpr::Const(_) => {}
    }
}

fn classify_action_expr(expr: &ActionExpr, profile: &mut FragmentProfile) {
    match expr {
        ActionExpr::Pred(state) => classify_state_expr(state, profile),
        ActionExpr::And(values) | ActionExpr::Or(values) => {
            for value in values {
                classify_action_expr(value, profile);
            }
        }
        ActionExpr::Implies(lhs, rhs) => {
            classify_action_expr(lhs, profile);
            classify_action_expr(rhs, profile);
        }
        ActionExpr::Exists(_, body) | ActionExpr::Enabled(body) => {
            classify_action_expr(body, profile);
        }
        ActionExpr::Compare { lhs, rhs, .. } | ActionExpr::Builtin { lhs, rhs, .. } => {
            classify_value_expr(lhs, profile);
            classify_value_expr(rhs, profile);
        }
        ActionExpr::Match { .. } => profile.has_stringly_nodes = true,
        ActionExpr::Quantified { .. } => {
            profile.has_stringly_nodes = true;
            profile.has_stringly_quantifiers = true;
        }
        ActionExpr::Rule { guard, update, .. } => {
            classify_action_expr(guard, profile);
            classify_update_expr(update, profile);
        }
        ActionExpr::BoxAction { action, .. } | ActionExpr::AngleAction { action, .. } => {
            classify_action_expr(action, profile);
        }
        ActionExpr::Opaque(_) => profile.has_opaque_nodes = true,
        ActionExpr::True | ActionExpr::False | ActionExpr::Ref(_) | ActionExpr::Unchanged(_) => {}
    }
}

fn classify_temporal_expr(expr: &TemporalExpr, profile: &mut FragmentProfile) {
    match expr {
        TemporalExpr::State(state) => classify_state_expr(state, profile),
        TemporalExpr::Action(action) | TemporalExpr::Enabled(action) => {
            classify_action_expr(action, profile)
        }
        TemporalExpr::Not(value)
        | TemporalExpr::Next(value)
        | TemporalExpr::Always(value)
        | TemporalExpr::Eventually(value) => classify_temporal_expr(value, profile),
        TemporalExpr::And(values) | TemporalExpr::Or(values) => {
            for value in values {
                classify_temporal_expr(value, profile);
            }
        }
        TemporalExpr::Implies(lhs, rhs)
        | TemporalExpr::Until(lhs, rhs)
        | TemporalExpr::LeadsTo(lhs, rhs) => {
            classify_temporal_expr(lhs, profile);
            classify_temporal_expr(rhs, profile);
        }
        TemporalExpr::Opaque(_) => profile.has_opaque_nodes = true,
        TemporalExpr::Ref(_) => {}
    }
}

fn classify_fairness_decl(fairness: &FairnessDecl, profile: &mut FragmentProfile) {
    match fairness {
        FairnessDecl::WF { action, .. } | FairnessDecl::SF { action, .. } => {
            classify_action_expr(action, profile);
        }
    }
}
