//! Proof/export surface for `nirvash`.
//!
//! `ProofBundleExporter` fail-closes on unsupported fragments and exports proof bundles
//! from the normalized lowered spec core.

use std::{collections::BTreeSet, path::PathBuf};

use nirvash_ir::{
    ActionExpr, BuiltinPredicateOp, ComparisonOp, CoreNormalizationError, FairnessDecl,
    NormalizedSpecCore, QuantifierKind, SpecCore, StateExpr, TemporalExpr, UpdateExpr,
    UpdateOpDecl, ValueExpr, ViewExpr,
};
pub use nirvash_ir::{ProofObligation, ProofObligationKind};
use nirvash_lower::ModelInstance;
pub use nirvash_lower::{ProofBackendId, ProofCertificate};
use sha2::{Digest, Sha256};

pub fn invariant_obligations(spec: &SpecCore) -> Vec<ProofObligation> {
    let invariant_names = invariant_names(spec);

    spec.invariants
        .iter()
        .enumerate()
        .flat_map(|(index, invariant)| {
            let name = invariant_names[index].clone();
            let init_label = format!("{}_init", sanitize_label(&name));
            let step_label = format!("{}_step", sanitize_label(&name));
            let init_theorem = format!("THEOREM {init_label} == Init => {name}");
            let step_theorem = format!(
                "THEOREM {step_label} == {name} /\\ Next => {}",
                render_state_tla(invariant, Timepoint::Next)
            );

            [
                ProofObligation::new(
                    init_label.clone(),
                    ProofObligationKind::InitImpliesInvariant,
                    init_theorem,
                    export_init_invariant_smt(&init_label, spec, invariant),
                ),
                ProofObligation::new(
                    step_label.clone(),
                    ProofObligationKind::StepPreservesInvariant,
                    step_theorem,
                    export_step_invariant_smt(&step_label, spec, invariant),
                ),
            ]
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofExportError {
    Normalization(CoreNormalizationError),
    UnsupportedFragment(String),
}

impl std::fmt::Display for ProofExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normalization(error) => write!(f, "{error}"),
            Self::UnsupportedFragment(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ProofExportError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportedArtifactKind {
    TlaModule,
    SmtlibObligation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedArtifact {
    pub label: String,
    pub kind: ExportedArtifactKind,
    pub content: String,
}

impl ExportedArtifact {
    pub fn new(
        label: impl Into<String>,
        kind: ExportedArtifactKind,
        content: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            kind,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProofBundle {
    pub obligations: Vec<ProofObligation>,
    pub exported_artifacts: Vec<ExportedArtifact>,
    pub certificates: Vec<ProofCertificate>,
}

impl ProofBundle {
    pub fn obligation_hash(&self) -> String {
        hash_proof_obligations(&self.obligations)
    }

    pub fn artifact_hash(&self) -> String {
        hash_exported_artifacts(&self.exported_artifacts)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofDischargeError {
    UnsupportedBundle(String),
    Rejected(String),
}

impl std::fmt::Display for ProofDischargeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedBundle(message) | Self::Rejected(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ProofDischargeError {}

pub trait ProofDischarger {
    fn backend(&self) -> ProofBackendId;

    fn discharge(&self, bundle: &ProofBundle) -> Result<ProofCertificate, ProofDischargeError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeProofDischarger {
    backend: ProofBackendId,
    artifact_path: Option<PathBuf>,
}

impl FakeProofDischarger {
    pub fn new(backend: ProofBackendId) -> Self {
        Self {
            backend,
            artifact_path: None,
        }
    }

    pub fn with_artifact_path(mut self, artifact_path: impl Into<PathBuf>) -> Self {
        self.artifact_path = Some(artifact_path.into());
        self
    }
}

impl ProofDischarger for FakeProofDischarger {
    fn backend(&self) -> ProofBackendId {
        self.backend.clone()
    }

    fn discharge(&self, bundle: &ProofBundle) -> Result<ProofCertificate, ProofDischargeError> {
        if bundle.exported_artifacts.is_empty() || bundle.obligations.is_empty() {
            return Err(ProofDischargeError::UnsupportedBundle(
                "proof bundle must contain obligations and exported artifacts".to_owned(),
            ));
        }
        Ok(ProofCertificate {
            backend: self.backend(),
            obligation_hash: bundle.obligation_hash(),
            artifact_hash: bundle.artifact_hash(),
            artifact_path: self.artifact_path.clone(),
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProofBundleExporter;

impl ProofBundleExporter {
    pub fn export(
        &self,
        spec: &SpecCore,
        reduction_obligations: &[ProofObligation],
    ) -> Result<ProofBundle, ProofExportError> {
        let normalized = spec.normalize().map_err(ProofExportError::Normalization)?;
        self.export_normalized_with_certificates(&normalized, reduction_obligations, &[])
    }

    pub fn export_normalized(
        &self,
        normalized: &NormalizedSpecCore,
        reduction_obligations: &[ProofObligation],
    ) -> Result<ProofBundle, ProofExportError> {
        self.export_normalized_with_certificates(normalized, reduction_obligations, &[])
    }

    pub fn export_model_instance<S, A>(
        &self,
        spec: &SpecCore,
        model_case: &ModelInstance<S, A>,
    ) -> Result<ProofBundle, ProofExportError> {
        let normalized = spec.normalize().map_err(ProofExportError::Normalization)?;
        self.export_normalized_with_certificates(
            &normalized,
            &model_case.reduction_obligations(),
            &model_case.reduction_certificates(),
        )
    }

    pub fn export_normalized_with_certificates(
        &self,
        normalized: &NormalizedSpecCore,
        reduction_obligations: &[ProofObligation],
        certificates: &[ProofCertificate],
    ) -> Result<ProofBundle, ProofExportError> {
        ensure_proof_fragment_supported(normalized)?;
        for obligation in reduction_obligations {
            ensure_obligation_supported(obligation)?;
        }
        let spec = normalized.core();
        let mut obligations = invariant_obligations(spec);
        obligations.extend_from_slice(reduction_obligations);

        let mut exported_artifacts = vec![ExportedArtifact::new(
            module_name(spec),
            ExportedArtifactKind::TlaModule,
            render_tla_module(spec, reduction_obligations),
        )];
        exported_artifacts.extend(obligations.iter().map(|obligation| {
            ExportedArtifact::new(
                obligation.label.clone(),
                ExportedArtifactKind::SmtlibObligation,
                obligation.smtlib.clone(),
            )
        }));

        Ok(ProofBundle {
            obligations,
            exported_artifacts,
            certificates: certificates.to_vec(),
        })
    }
}

pub mod importer {
    use super::{ProofBackendId, ProofBundle, ProofCertificate};
    use std::path::PathBuf;

    pub fn import_certificate(
        backend: ProofBackendId,
        bundle: &ProofBundle,
        artifact_path: impl Into<PathBuf>,
    ) -> ProofCertificate {
        ProofCertificate {
            backend,
            obligation_hash: bundle.obligation_hash(),
            artifact_hash: bundle.artifact_hash(),
            artifact_path: Some(artifact_path.into()),
        }
    }

    pub fn import_verus_certificate(
        bundle: &ProofBundle,
        artifact_path: impl Into<PathBuf>,
    ) -> ProofCertificate {
        import_certificate(ProofBackendId::Verus, bundle, artifact_path)
    }

    pub fn import_refined_rust_certificate(
        bundle: &ProofBundle,
        artifact_path: impl Into<PathBuf>,
    ) -> ProofCertificate {
        import_certificate(ProofBackendId::RefinedRust, bundle, artifact_path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timepoint {
    Current,
    Next,
}

impl Timepoint {
    const fn smt_suffix(self) -> &'static str {
        match self {
            Self::Current => "0",
            Self::Next => "1",
        }
    }
}

fn ensure_proof_fragment_supported(
    normalized: &NormalizedSpecCore,
) -> Result<(), ProofExportError> {
    let reasons = normalized.fragment_profile().proof_unsupported_reasons();
    if reasons.is_empty() {
        return Ok(());
    }
    Err(ProofExportError::UnsupportedFragment(format!(
        "proof bundle export does not support {}",
        reasons.join(", ")
    )))
}

fn ensure_obligation_supported(obligation: &ProofObligation) -> Result<(), ProofExportError> {
    match obligation.kind {
        ProofObligationKind::InitImpliesInvariant
        | ProofObligationKind::StepPreservesInvariant
        | ProofObligationKind::SymmetryReduction
        | ProofObligationKind::StateQuotientReduction
        | ProofObligationKind::PorReduction => Ok(()),
    }
}

fn hash_proof_obligations(obligations: &[ProofObligation]) -> String {
    let mut hasher = Sha256::new();
    for obligation in obligations {
        hasher.update(obligation.label.as_bytes());
        hasher.update([0]);
        hasher.update(format!("{:?}", obligation.kind).as_bytes());
        hasher.update([0]);
        hasher.update(obligation.tla_theorem.as_bytes());
        hasher.update([0]);
        hasher.update(obligation.smtlib.as_bytes());
        hasher.update([0xff]);
    }
    format!("{:x}", hasher.finalize())
}

fn hash_exported_artifacts(artifacts: &[ExportedArtifact]) -> String {
    let mut hasher = Sha256::new();
    for artifact in artifacts {
        hasher.update(artifact.label.as_bytes());
        hasher.update([0]);
        hasher.update(format!("{:?}", artifact.kind).as_bytes());
        hasher.update([0]);
        hasher.update(artifact.content.as_bytes());
        hasher.update([0xff]);
    }
    format!("{:x}", hasher.finalize())
}

fn render_tla_module(spec: &SpecCore, extra_obligations: &[ProofObligation]) -> String {
    let module_name = module_name(spec);
    let variables = if spec.vars.is_empty() {
        "vars".to_owned()
    } else {
        spec.vars
            .iter()
            .map(|decl| decl.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let invariant_names = invariant_names(spec);
    let property_names = named_items(spec, "property::", spec.temporal_props.len(), "Property");
    let fairness_names = fairness_names(spec);
    let mut obligations = invariant_obligations(spec);
    obligations.extend_from_slice(extra_obligations);

    let mut lines = vec![
        format!("---- MODULE {module_name} ----"),
        "EXTENDS Naturals, Sequences, TLC".to_owned(),
        format!("VARIABLES {variables}"),
        String::new(),
        format!(
            "Init == {}",
            render_state_tla(&spec.init, Timepoint::Current)
        ),
        format!("Next == {}", render_action_tla(&spec.next)),
    ];

    for (name, invariant) in invariant_names.iter().zip(spec.invariants.iter()) {
        lines.push(format!(
            "{name} == {}",
            render_state_tla(invariant, Timepoint::Current)
        ));
    }
    for (name, property) in property_names.iter().zip(spec.temporal_props.iter()) {
        lines.push(format!("{name} == {}", render_temporal_tla(property)));
    }
    for (name, fairness) in fairness_names.iter().zip(spec.fairness.iter()) {
        lines.push(format!("{name} == {}", render_fairness_tla(fairness)));
    }

    if !obligations.is_empty() {
        lines.push(String::new());
        for obligation in obligations {
            lines.push(obligation.tla_theorem);
        }
    }

    lines.push("====".to_owned());
    lines.join("\n")
}

fn module_name(spec: &SpecCore) -> String {
    spec.defs
        .iter()
        .find(|definition| definition.name == "frontend" && !definition.body.is_empty())
        .map(|definition| definition.body.clone())
        .unwrap_or_else(|| "SpecCore".to_owned())
}

fn named_items(spec: &SpecCore, prefix: &str, len: usize, fallback_prefix: &str) -> Vec<String> {
    let mut names = spec
        .defs
        .iter()
        .filter(|definition| definition.name.starts_with(prefix))
        .map(|definition| {
            if definition.body.is_empty() {
                definition.name[prefix.len()..].to_owned()
            } else {
                definition.body.clone()
            }
        })
        .collect::<Vec<_>>();

    while names.len() < len {
        names.push(format!("{fallback_prefix}_{}", names.len()));
    }

    names
}

fn invariant_names(spec: &SpecCore) -> Vec<String> {
    let mut names = named_items(spec, "invariant::", spec.invariants.len(), "Invariant");
    for (index, invariant) in spec.invariants.iter().enumerate() {
        if names[index].starts_with("Invariant_")
            && let Some(name) = fallback_state_name(invariant)
        {
            names[index] = name;
        }
    }
    names
}

fn fairness_names(spec: &SpecCore) -> Vec<String> {
    let mut names = named_items(spec, "fairness::", spec.fairness.len(), "Fairness");
    for (index, fairness) in spec.fairness.iter().enumerate() {
        if names[index].starts_with("Fairness_") {
            names[index] = match fairness {
                FairnessDecl::WF { action, .. } | FairnessDecl::SF { action, .. } => {
                    fallback_action_name(action).unwrap_or_else(|| names[index].clone())
                }
            };
        }
    }
    names
}

fn fallback_state_name(expr: &StateExpr) -> Option<String> {
    match expr {
        StateExpr::Ref(name) | StateExpr::Var(name) | StateExpr::Const(name) => Some(name.clone()),
        _ => None,
    }
}

fn fallback_action_name(expr: &ActionExpr) -> Option<String> {
    match expr {
        ActionExpr::Ref(name) => Some(name.clone()),
        ActionExpr::Rule { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn sanitize_label(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut last_was_underscore = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            result.push(ch);
            last_was_underscore = false;
        } else if !last_was_underscore {
            result.push('_');
            last_was_underscore = true;
        }
    }
    let trimmed = result.trim_matches('_');
    if trimmed.is_empty() {
        "proof_obligation".to_owned()
    } else if trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("proof_{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

fn render_view_tla(view: &ViewExpr) -> String {
    match view {
        ViewExpr::Vars => "vars".to_owned(),
        ViewExpr::Named(name) => name.clone(),
    }
}

fn render_state_tla(expr: &StateExpr, timepoint: Timepoint) -> String {
    match expr {
        StateExpr::True => "TRUE".to_owned(),
        StateExpr::False => "FALSE".to_owned(),
        StateExpr::Var(name) | StateExpr::Ref(name) | StateExpr::Const(name) => name.clone(),
        StateExpr::Eq(lhs, rhs) => format!(
            "({} = {})",
            render_state_tla(lhs, timepoint),
            render_state_tla(rhs, timepoint)
        ),
        StateExpr::In(lhs, rhs) => format!(
            "({} \\in {})",
            render_state_tla(lhs, timepoint),
            render_state_tla(rhs, timepoint)
        ),
        StateExpr::Not(inner) => format!("~({})", render_state_tla(inner, timepoint)),
        StateExpr::And(exprs) => render_joined_tla(
            exprs.iter().map(|expr| render_state_tla(expr, timepoint)),
            "/\\",
            "TRUE",
        ),
        StateExpr::Or(exprs) => render_joined_tla(
            exprs.iter().map(|expr| render_state_tla(expr, timepoint)),
            "\\/",
            "FALSE",
        ),
        StateExpr::Implies(lhs, rhs) => format!(
            "({} => {})",
            render_state_tla(lhs, timepoint),
            render_state_tla(rhs, timepoint)
        ),
        StateExpr::Compare { op, lhs, rhs } => format!(
            "({} {} {})",
            render_value_tla(lhs, timepoint),
            tla_compare_op(*op),
            render_value_tla(rhs, timepoint)
        ),
        StateExpr::Builtin { op, lhs, rhs } => match op {
            BuiltinPredicateOp::Contains => format!(
                "({} \\in {})",
                render_value_tla(lhs, timepoint),
                render_value_tla(rhs, timepoint)
            ),
            BuiltinPredicateOp::SubsetOf => format!(
                "({} \\subseteq {})",
                render_value_tla(lhs, timepoint),
                render_value_tla(rhs, timepoint)
            ),
        },
        StateExpr::Match { value, pattern } => format!("Match({value}, {pattern})"),
        StateExpr::Quantified {
            kind, domain, body, ..
        } => format!("{} x \\in {}: {}", tla_quantifier(*kind), domain, body),
        StateExpr::Forall(bindings, body) => format!(
            "\\A {}: {}",
            bindings.join(", "),
            render_state_tla(body, timepoint)
        ),
        StateExpr::Exists(bindings, body) => format!(
            "\\E {}: {}",
            bindings.join(", "),
            render_state_tla(body, timepoint)
        ),
        StateExpr::Choose(binding, body) => {
            format!("CHOOSE {binding}: {}", render_state_tla(body, timepoint))
        }
        StateExpr::Opaque(text) => text.clone(),
    }
}

fn render_action_tla(expr: &ActionExpr) -> String {
    match expr {
        ActionExpr::True => "TRUE".to_owned(),
        ActionExpr::False => "FALSE".to_owned(),
        ActionExpr::Ref(name) => name.clone(),
        ActionExpr::Pred(predicate) => render_state_tla(predicate, Timepoint::Current),
        ActionExpr::Unchanged(vars) => {
            if vars.is_empty() {
                "TRUE".to_owned()
            } else {
                format!("UNCHANGED <<{}>>", vars.join(", "))
            }
        }
        ActionExpr::And(exprs) => {
            render_joined_tla(exprs.iter().map(render_action_tla), "/\\", "TRUE")
        }
        ActionExpr::Or(exprs) => {
            render_joined_tla(exprs.iter().map(render_action_tla), "\\/", "FALSE")
        }
        ActionExpr::Implies(lhs, rhs) => {
            format!("({} => {})", render_action_tla(lhs), render_action_tla(rhs))
        }
        ActionExpr::Exists(bindings, body) => {
            format!("\\E {}: {}", bindings.join(", "), render_action_tla(body))
        }
        ActionExpr::Enabled(action) => format!("ENABLED ({})", render_action_tla(action)),
        ActionExpr::Compare { op, lhs, rhs } => format!(
            "({} {} {})",
            render_value_tla(lhs, Timepoint::Current),
            tla_compare_op(*op),
            render_value_tla(rhs, Timepoint::Current)
        ),
        ActionExpr::Builtin { op, lhs, rhs } => match op {
            BuiltinPredicateOp::Contains => format!(
                "({} \\in {})",
                render_value_tla(lhs, Timepoint::Current),
                render_value_tla(rhs, Timepoint::Current)
            ),
            BuiltinPredicateOp::SubsetOf => format!(
                "({} \\subseteq {})",
                render_value_tla(lhs, Timepoint::Current),
                render_value_tla(rhs, Timepoint::Current)
            ),
        },
        ActionExpr::Match { value, pattern } => format!("Match({value}, {pattern})"),
        ActionExpr::Quantified {
            kind, domain, body, ..
        } => format!("{} x \\in {}: {}", tla_quantifier(*kind), domain, body),
        ActionExpr::Rule { guard, update, .. } => {
            let guard_text = render_action_tla(guard);
            let update_text = render_update_tla(update);
            if update_text == "TRUE" {
                guard_text
            } else {
                format!("({guard_text} /\\ {update_text})")
            }
        }
        ActionExpr::BoxAction { action, view } => {
            format!("[{}]_{}", render_action_tla(action), render_view_tla(view))
        }
        ActionExpr::AngleAction { action, view } => {
            format!(
                "<<{}>>_{}",
                render_action_tla(action),
                render_view_tla(view)
            )
        }
        ActionExpr::Opaque(text) => text.clone(),
    }
}

fn render_update_tla(update: &UpdateExpr) -> String {
    match update {
        UpdateExpr::Sequence(ops) => {
            render_joined_tla(ops.iter().map(render_update_op_tla), "/\\", "TRUE")
        }
        UpdateExpr::Choice { domain, body, .. } => format!("CHOOSE x \\in {domain}: {body}"),
    }
}

fn render_update_op_tla(op: &UpdateOpDecl) -> String {
    match op {
        UpdateOpDecl::Assign { target, value } => {
            format!(
                "{target}' = {}",
                render_value_tla(value, Timepoint::Current)
            )
        }
        UpdateOpDecl::SetInsert { target, item } => format!(
            "{target}' = {target} \\cup {{{}}}",
            render_value_tla(item, Timepoint::Current)
        ),
        UpdateOpDecl::SetRemove { target, item } => format!(
            "{target}' = {target} \\ {{ {} }}",
            render_value_tla(item, Timepoint::Current)
        ),
        UpdateOpDecl::Effect { name, .. } => format!("Effect({name})"),
    }
}

fn render_value_tla(expr: &ValueExpr, timepoint: Timepoint) -> String {
    match expr {
        ValueExpr::Unit => "UNIT".to_owned(),
        ValueExpr::Opaque(text) | ValueExpr::Literal(text) => text.clone(),
        ValueExpr::Field(name) => match timepoint {
            Timepoint::Current => name.clone(),
            Timepoint::Next => format!("{name}'"),
        },
        ValueExpr::PureCall {
            name, read_paths, ..
        } => {
            if read_paths.is_empty() {
                name.clone()
            } else {
                format!("{name}({})", read_paths.join(", "))
            }
        }
        ValueExpr::Add(lhs, rhs) => format!(
            "({} + {})",
            render_value_tla(lhs, timepoint),
            render_value_tla(rhs, timepoint)
        ),
        ValueExpr::Sub(lhs, rhs) => format!(
            "({} - {})",
            render_value_tla(lhs, timepoint),
            render_value_tla(rhs, timepoint)
        ),
        ValueExpr::Mul(lhs, rhs) => format!(
            "({} * {})",
            render_value_tla(lhs, timepoint),
            render_value_tla(rhs, timepoint)
        ),
        ValueExpr::Neg(expr) => format!("-{}", render_value_tla(expr, timepoint)),
        ValueExpr::Union(lhs, rhs) => format!(
            "({} \\cup {})",
            render_value_tla(lhs, timepoint),
            render_value_tla(rhs, timepoint)
        ),
        ValueExpr::Intersection(lhs, rhs) => format!(
            "({} \\cap {})",
            render_value_tla(lhs, timepoint),
            render_value_tla(rhs, timepoint)
        ),
        ValueExpr::Difference(lhs, rhs) => format!(
            "({} \\ {})",
            render_value_tla(lhs, timepoint),
            render_value_tla(rhs, timepoint)
        ),
        ValueExpr::SequenceUpdate { base, index, value } => format!(
            "[{} EXCEPT ![{}] = {}]",
            render_value_tla(base, timepoint),
            render_value_tla(index, timepoint),
            render_value_tla(value, timepoint)
        ),
        ValueExpr::FunctionUpdate { base, key, value } => format!(
            "[{} EXCEPT ![{}] = {}]",
            render_value_tla(base, timepoint),
            render_value_tla(key, timepoint),
            render_value_tla(value, timepoint)
        ),
        ValueExpr::RecordUpdate { base, field, value } => format!(
            "[{} EXCEPT !.{field} = {}]",
            render_value_tla(base, timepoint),
            render_value_tla(value, timepoint)
        ),
        ValueExpr::Comprehension { domain, body, .. } => format!("{{ {body} : {domain} }}"),
        ValueExpr::Conditional {
            condition,
            then_branch,
            else_branch,
        } => format!(
            "IF {condition} THEN {} ELSE {}",
            render_value_tla(then_branch, timepoint),
            render_value_tla(else_branch, timepoint)
        ),
    }
}

fn render_temporal_tla(expr: &TemporalExpr) -> String {
    match expr {
        TemporalExpr::State(expr) => render_state_tla(expr, Timepoint::Current),
        TemporalExpr::Action(expr) => render_action_tla(expr),
        TemporalExpr::Ref(name) => name.clone(),
        TemporalExpr::Not(inner) => format!("~({})", render_temporal_tla(inner)),
        TemporalExpr::And(exprs) => {
            render_joined_tla(exprs.iter().map(render_temporal_tla), "/\\", "TRUE")
        }
        TemporalExpr::Or(exprs) => {
            render_joined_tla(exprs.iter().map(render_temporal_tla), "\\/", "FALSE")
        }
        TemporalExpr::Implies(lhs, rhs) => format!(
            "({} => {})",
            render_temporal_tla(lhs),
            render_temporal_tla(rhs)
        ),
        TemporalExpr::Next(inner) => format!("X({})", render_temporal_tla(inner)),
        TemporalExpr::Always(inner) => format!("[]({})", render_temporal_tla(inner)),
        TemporalExpr::Eventually(inner) => format!("<>({})", render_temporal_tla(inner)),
        TemporalExpr::Until(lhs, rhs) => {
            format!(
                "({}) U ({})",
                render_temporal_tla(lhs),
                render_temporal_tla(rhs)
            )
        }
        TemporalExpr::LeadsTo(lhs, rhs) => {
            format!(
                "({}) ~> ({})",
                render_temporal_tla(lhs),
                render_temporal_tla(rhs)
            )
        }
        TemporalExpr::Enabled(action) => format!("ENABLED ({})", render_action_tla(action)),
        TemporalExpr::Opaque(text) => text.clone(),
    }
}

fn render_fairness_tla(fairness: &FairnessDecl) -> String {
    match fairness {
        FairnessDecl::WF { view, action } => {
            format!(
                "WF_{}({})",
                render_view_tla(view),
                render_action_tla(action)
            )
        }
        FairnessDecl::SF { view, action } => {
            format!(
                "SF_{}({})",
                render_view_tla(view),
                render_action_tla(action)
            )
        }
    }
}

fn render_joined_tla(exprs: impl IntoIterator<Item = String>, op: &str, empty: &str) -> String {
    let parts = exprs.into_iter().collect::<Vec<_>>();
    match parts.len() {
        0 => empty.to_owned(),
        1 => parts.into_iter().next().unwrap_or_else(|| empty.to_owned()),
        _ => format!("({})", parts.join(&format!(" {op} "))),
    }
}

fn tla_compare_op(op: ComparisonOp) -> &'static str {
    match op {
        ComparisonOp::Eq => "=",
        ComparisonOp::Ne => "#",
        ComparisonOp::Lt => "<",
        ComparisonOp::Le => "<=",
        ComparisonOp::Gt => ">",
        ComparisonOp::Ge => ">=",
    }
}

fn tla_quantifier(kind: QuantifierKind) -> &'static str {
    match kind {
        QuantifierKind::ForAll => "\\A",
        QuantifierKind::Exists => "\\E",
    }
}

fn export_init_invariant_smt(label: &str, spec: &SpecCore, invariant: &StateExpr) -> String {
    let mut env = SmtEnv::default();
    let body = format!(
        "(=> {} {})",
        render_state_smt(&spec.init, &mut env, Timepoint::Current),
        render_state_smt(invariant, &mut env, Timepoint::Current)
    );
    env.finish(label, &body)
}

fn export_step_invariant_smt(label: &str, spec: &SpecCore, invariant: &StateExpr) -> String {
    let mut env = SmtEnv::default();
    let assumption = render_smt_and(vec![
        render_state_smt(invariant, &mut env, Timepoint::Current),
        render_action_smt(&spec.next, &mut env, &spec.vars),
    ]);
    let conclusion = render_state_smt(invariant, &mut env, Timepoint::Next);
    let body = format!("(=> {assumption} {conclusion})");
    env.finish(label, &body)
}

#[derive(Debug, Default)]
struct SmtEnv {
    bool_atoms: BTreeSet<String>,
    val_atoms: BTreeSet<String>,
    val_functions: BTreeSet<(String, usize)>,
    val_predicates: BTreeSet<(String, usize)>,
    bool_predicates: BTreeSet<(String, usize)>,
}

impl SmtEnv {
    fn bool_atom(&mut self, raw: impl Into<String>) -> String {
        let raw = raw.into();
        self.bool_atoms.insert(raw.clone());
        smt_symbol(&raw)
    }

    fn val_atom(&mut self, raw: impl Into<String>) -> String {
        let raw = raw.into();
        self.val_atoms.insert(raw.clone());
        smt_symbol(&raw)
    }

    fn val_function(&mut self, name: &'static str, args: Vec<String>) -> String {
        self.val_functions.insert((name.to_owned(), args.len()));
        render_smt_app(name, args)
    }

    fn val_predicate(&mut self, name: &'static str, args: Vec<String>) -> String {
        self.val_predicates.insert((name.to_owned(), args.len()));
        render_smt_app(name, args)
    }

    fn bool_predicate(&mut self, name: &'static str, args: Vec<String>) -> String {
        self.bool_predicates.insert((name.to_owned(), args.len()));
        render_smt_app(name, args)
    }

    fn finish(&self, label: &str, body: &str) -> String {
        let mut lines = vec![
            "(set-logic ALL)".to_owned(),
            "(declare-sort Val 0)".to_owned(),
        ];

        for atom in &self.val_atoms {
            lines.push(format!("(declare-fun {} () Val)", smt_symbol(atom)));
        }
        for atom in &self.bool_atoms {
            lines.push(format!("(declare-fun {} () Bool)", smt_symbol(atom)));
        }
        for (name, arity) in &self.val_functions {
            lines.push(format!(
                "(declare-fun {} ({}) Val)",
                smt_symbol(name),
                smt_sorts(*arity, "Val")
            ));
        }
        for (name, arity) in &self.val_predicates {
            lines.push(format!(
                "(declare-fun {} ({}) Bool)",
                smt_symbol(name),
                smt_sorts(*arity, "Val")
            ));
        }
        for (name, arity) in &self.bool_predicates {
            lines.push(format!(
                "(declare-fun {} ({}) Bool)",
                smt_symbol(name),
                smt_sorts(*arity, "Bool")
            ));
        }

        lines.push(format!("(assert (! {body} :named {}))", smt_symbol(label)));
        lines.push("(check-sat)".to_owned());
        lines.join("\n")
    }
}

fn render_smt_app(name: &str, args: Vec<String>) -> String {
    if args.is_empty() {
        smt_symbol(name)
    } else {
        format!("({} {})", smt_symbol(name), args.join(" "))
    }
}

fn smt_sorts(arity: usize, sort: &str) -> String {
    match arity {
        0 => String::new(),
        _ => std::iter::repeat_n(sort, arity)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn smt_symbol(raw: impl AsRef<str>) -> String {
    let escaped = raw
        .as_ref()
        .chars()
        .map(|ch| match ch {
            '|' | '\\' => '_',
            _ => ch,
        })
        .collect::<String>();
    format!("|{escaped}|")
}

fn render_state_smt(expr: &StateExpr, env: &mut SmtEnv, timepoint: Timepoint) -> String {
    match expr {
        StateExpr::True => "true".to_owned(),
        StateExpr::False => "false".to_owned(),
        StateExpr::Var(name) => {
            env.bool_atom(format!("state_var:{name}@{}", timepoint.smt_suffix()))
        }
        StateExpr::Ref(name) => {
            env.bool_atom(format!("state_ref:{name}@{}", timepoint.smt_suffix()))
        }
        StateExpr::Const(name) => {
            env.bool_atom(format!("state_const:{name}@{}", timepoint.smt_suffix()))
        }
        StateExpr::Eq(lhs, rhs) => format!(
            "(= {} {})",
            render_state_smt(lhs, env, timepoint),
            render_state_smt(rhs, env, timepoint)
        ),
        StateExpr::In(lhs, rhs) => {
            let lhs = render_state_smt(lhs, env, timepoint);
            let rhs = render_state_smt(rhs, env, timepoint);
            env.bool_predicate("state_in", vec![lhs, rhs])
        }
        StateExpr::Not(inner) => format!("(not {})", render_state_smt(inner, env, timepoint)),
        StateExpr::And(exprs) => render_smt_and(
            exprs
                .iter()
                .map(|expr| render_state_smt(expr, env, timepoint)),
        ),
        StateExpr::Or(exprs) => render_smt_or(
            exprs
                .iter()
                .map(|expr| render_state_smt(expr, env, timepoint)),
        ),
        StateExpr::Implies(lhs, rhs) => format!(
            "(=> {} {})",
            render_state_smt(lhs, env, timepoint),
            render_state_smt(rhs, env, timepoint)
        ),
        StateExpr::Compare { op, lhs, rhs } => {
            render_value_compare_smt(*op, lhs, rhs, env, timepoint)
        }
        StateExpr::Builtin { op, lhs, rhs } => {
            let name = match op {
                BuiltinPredicateOp::Contains => "contains",
                BuiltinPredicateOp::SubsetOf => "subset_of",
            };
            let lhs = render_value_smt(lhs, env, timepoint);
            let rhs = render_value_smt(rhs, env, timepoint);
            env.val_predicate(name, vec![lhs, rhs])
        }
        StateExpr::Match { value, pattern } => env.bool_atom(format!(
            "match:{value}:{pattern}@{}",
            timepoint.smt_suffix()
        )),
        StateExpr::Quantified {
            kind, domain, body, ..
        } => env.bool_atom(format!(
            "quantified:{:?}:{domain}:{body}@{}",
            kind,
            timepoint.smt_suffix()
        )),
        StateExpr::Forall(bindings, body) => {
            render_smt_quantifier("forall", bindings, render_state_smt(body, env, timepoint))
        }
        StateExpr::Exists(bindings, body) => {
            render_smt_quantifier("exists", bindings, render_state_smt(body, env, timepoint))
        }
        StateExpr::Choose(binding, body) => env.bool_atom(format!(
            "choose:{binding}:{}@{}",
            render_state_tla(body, timepoint),
            timepoint.smt_suffix()
        )),
        StateExpr::Opaque(text) => {
            env.bool_atom(format!("state_opaque:{text}@{}", timepoint.smt_suffix()))
        }
    }
}

fn render_action_smt(expr: &ActionExpr, env: &mut SmtEnv, vars: &[nirvash_ir::VarDecl]) -> String {
    match expr {
        ActionExpr::True => "true".to_owned(),
        ActionExpr::False => "false".to_owned(),
        ActionExpr::Ref(name) => env.bool_atom(format!("action_ref:{name}")),
        ActionExpr::Pred(predicate) => render_state_smt(predicate, env, Timepoint::Current),
        ActionExpr::Unchanged(unchanged_vars) => {
            render_unchanged_smt(env, unchanged_vars.iter().map(String::as_str))
        }
        ActionExpr::And(exprs) => {
            render_smt_and(exprs.iter().map(|expr| render_action_smt(expr, env, vars)))
        }
        ActionExpr::Or(exprs) => {
            render_smt_or(exprs.iter().map(|expr| render_action_smt(expr, env, vars)))
        }
        ActionExpr::Implies(lhs, rhs) => format!(
            "(=> {} {})",
            render_action_smt(lhs, env, vars),
            render_action_smt(rhs, env, vars)
        ),
        ActionExpr::Exists(bindings, body) => {
            render_smt_quantifier("exists", bindings, render_action_smt(body, env, vars))
        }
        ActionExpr::Enabled(action) => {
            let action = render_action_smt(action, env, vars);
            env.bool_predicate("enabled", vec![action])
        }
        ActionExpr::Compare { op, lhs, rhs } => {
            render_value_compare_smt(*op, lhs, rhs, env, Timepoint::Current)
        }
        ActionExpr::Builtin { op, lhs, rhs } => {
            let name = match op {
                BuiltinPredicateOp::Contains => "contains",
                BuiltinPredicateOp::SubsetOf => "subset_of",
            };
            let lhs = render_value_smt(lhs, env, Timepoint::Current);
            let rhs = render_value_smt(rhs, env, Timepoint::Current);
            env.val_predicate(name, vec![lhs, rhs])
        }
        ActionExpr::Match { value, pattern } => {
            env.bool_atom(format!("action_match:{value}:{pattern}"))
        }
        ActionExpr::Quantified {
            kind, domain, body, ..
        } => env.bool_atom(format!("action_quantified:{:?}:{domain}:{body}", kind)),
        ActionExpr::Rule { guard, update, .. } => render_smt_and([
            render_action_smt(guard, env, vars),
            render_update_smt(update, env),
        ]),
        ActionExpr::BoxAction { action, view } => match view {
            ViewExpr::Vars => render_smt_or([
                render_action_smt(action, env, vars),
                render_unchanged_smt(env, vars.iter().map(|var| var.name.as_str())),
            ]),
            ViewExpr::Named(name) => env.bool_atom(format!("box_named:{name}")),
        },
        ActionExpr::AngleAction { action, view } => match view {
            ViewExpr::Vars => render_smt_and([
                render_action_smt(action, env, vars),
                format!(
                    "(not {})",
                    render_unchanged_smt(env, vars.iter().map(|var| var.name.as_str()))
                ),
            ]),
            ViewExpr::Named(name) => env.bool_atom(format!("angle_named:{name}")),
        },
        ActionExpr::Opaque(text) => env.bool_atom(format!("action_opaque:{text}")),
    }
}

fn render_update_smt(update: &UpdateExpr, env: &mut SmtEnv) -> String {
    match update {
        UpdateExpr::Sequence(ops) => {
            render_smt_and(ops.iter().map(|op| render_update_op_smt(op, env)))
        }
        UpdateExpr::Choice { domain, body, .. } => env.bool_atom(format!("choice:{domain}:{body}")),
    }
}

fn render_update_op_smt(op: &UpdateOpDecl, env: &mut SmtEnv) -> String {
    match op {
        UpdateOpDecl::Assign { target, value } => format!(
            "(= {} {})",
            env.val_atom(format!("field:{target}@1")),
            render_value_smt(value, env, Timepoint::Current)
        ),
        UpdateOpDecl::SetInsert { target, item } => {
            let before = env.val_atom(format!("field:{target}@0"));
            let after = env.val_atom(format!("field:{target}@1"));
            let item = render_value_smt(item, env, Timepoint::Current);
            env.val_predicate("set_insert_update", vec![before, after, item])
        }
        UpdateOpDecl::SetRemove { target, item } => {
            let before = env.val_atom(format!("field:{target}@0"));
            let after = env.val_atom(format!("field:{target}@1"));
            let item = render_value_smt(item, env, Timepoint::Current);
            env.val_predicate("set_remove_update", vec![before, after, item])
        }
        UpdateOpDecl::Effect { name, .. } => env.bool_atom(format!("effect:{name}")),
    }
}

fn render_value_smt(expr: &ValueExpr, env: &mut SmtEnv, timepoint: Timepoint) -> String {
    match expr {
        ValueExpr::Unit => env.val_atom("unit"),
        ValueExpr::Opaque(text) => env.val_atom(format!("opaque:{text}")),
        ValueExpr::Literal(text) => env.val_atom(format!("literal:{text}")),
        ValueExpr::Field(name) => env.val_atom(format!("field:{name}@{}", timepoint.smt_suffix())),
        ValueExpr::PureCall { name, .. } => {
            env.val_atom(format!("pure:{name}@{}", timepoint.smt_suffix()))
        }
        ValueExpr::Add(lhs, rhs) => {
            let lhs = render_value_smt(lhs, env, timepoint);
            let rhs = render_value_smt(rhs, env, timepoint);
            env.val_function("val_add", vec![lhs, rhs])
        }
        ValueExpr::Sub(lhs, rhs) => {
            let lhs = render_value_smt(lhs, env, timepoint);
            let rhs = render_value_smt(rhs, env, timepoint);
            env.val_function("val_sub", vec![lhs, rhs])
        }
        ValueExpr::Mul(lhs, rhs) => {
            let lhs = render_value_smt(lhs, env, timepoint);
            let rhs = render_value_smt(rhs, env, timepoint);
            env.val_function("val_mul", vec![lhs, rhs])
        }
        ValueExpr::Neg(expr) => {
            let expr = render_value_smt(expr, env, timepoint);
            env.val_function("val_neg", vec![expr])
        }
        ValueExpr::Union(lhs, rhs) => {
            let lhs = render_value_smt(lhs, env, timepoint);
            let rhs = render_value_smt(rhs, env, timepoint);
            env.val_function("val_union", vec![lhs, rhs])
        }
        ValueExpr::Intersection(lhs, rhs) => {
            let lhs = render_value_smt(lhs, env, timepoint);
            let rhs = render_value_smt(rhs, env, timepoint);
            env.val_function("val_intersection", vec![lhs, rhs])
        }
        ValueExpr::Difference(lhs, rhs) => {
            let lhs = render_value_smt(lhs, env, timepoint);
            let rhs = render_value_smt(rhs, env, timepoint);
            env.val_function("val_difference", vec![lhs, rhs])
        }
        ValueExpr::SequenceUpdate { base, index, value } => {
            let base = render_value_smt(base, env, timepoint);
            let index = render_value_smt(index, env, timepoint);
            let value = render_value_smt(value, env, timepoint);
            env.val_function("sequence_update", vec![base, index, value])
        }
        ValueExpr::FunctionUpdate { base, key, value } => {
            let base = render_value_smt(base, env, timepoint);
            let key = render_value_smt(key, env, timepoint);
            let value = render_value_smt(value, env, timepoint);
            env.val_function("function_update", vec![base, key, value])
        }
        ValueExpr::RecordUpdate { base, field, value } => {
            let base = render_value_smt(base, env, timepoint);
            let field = env.val_atom(format!("record_field:{field}"));
            let value = render_value_smt(value, env, timepoint);
            env.val_function("record_update", vec![base, field, value])
        }
        ValueExpr::Comprehension { domain, body, .. } => env.val_atom(format!(
            "comprehension:{domain}:{body}@{}",
            timepoint.smt_suffix()
        )),
        ValueExpr::Conditional {
            condition,
            then_branch,
            else_branch,
        } => format!(
            "(ite {} {} {})",
            env.bool_atom(format!("condition:{condition}@{}", timepoint.smt_suffix())),
            render_value_smt(then_branch, env, timepoint),
            render_value_smt(else_branch, env, timepoint)
        ),
    }
}

fn render_value_compare_smt(
    op: ComparisonOp,
    lhs: &ValueExpr,
    rhs: &ValueExpr,
    env: &mut SmtEnv,
    timepoint: Timepoint,
) -> String {
    let lhs = render_value_smt(lhs, env, timepoint);
    let rhs = render_value_smt(rhs, env, timepoint);
    match op {
        ComparisonOp::Eq => format!("(= {lhs} {rhs})"),
        ComparisonOp::Ne => format!("(distinct {lhs} {rhs})"),
        ComparisonOp::Lt => env.val_predicate("cmp_lt", vec![lhs, rhs]),
        ComparisonOp::Le => env.val_predicate("cmp_le", vec![lhs, rhs]),
        ComparisonOp::Gt => env.val_predicate("cmp_gt", vec![lhs, rhs]),
        ComparisonOp::Ge => env.val_predicate("cmp_ge", vec![lhs, rhs]),
    }
}

fn render_smt_and(exprs: impl IntoIterator<Item = String>) -> String {
    let parts = exprs.into_iter().collect::<Vec<_>>();
    match parts.len() {
        0 => "true".to_owned(),
        1 => parts
            .into_iter()
            .next()
            .unwrap_or_else(|| "true".to_owned()),
        _ => format!("(and {})", parts.join(" ")),
    }
}

fn render_smt_or(exprs: impl IntoIterator<Item = String>) -> String {
    let parts = exprs.into_iter().collect::<Vec<_>>();
    match parts.len() {
        0 => "false".to_owned(),
        1 => parts
            .into_iter()
            .next()
            .unwrap_or_else(|| "false".to_owned()),
        _ => format!("(or {})", parts.join(" ")),
    }
}

fn render_smt_quantifier(kind: &str, bindings: &[String], body: String) -> String {
    let bound_vars = if bindings.is_empty() {
        "((_unused Val))".to_owned()
    } else {
        bindings
            .iter()
            .map(|binding| format!("({} Val)", smt_symbol(binding)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!("({kind} ({bound_vars}) {body})")
}

fn render_unchanged_smt<'a>(env: &mut SmtEnv, vars: impl IntoIterator<Item = &'a str>) -> String {
    render_smt_and(vars.into_iter().map(|name| {
        format!(
            "(= {} {})",
            env.val_atom(format!("field:{name}@0")),
            env.val_atom(format!("field:{name}@1"))
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        ExportedArtifactKind, FakeProofDischarger, ProofBundleExporter, ProofDischarger,
        ProofExportError, ProofObligation, ProofObligationKind, importer, invariant_obligations,
    };
    use nirvash_ir::{
        ActionExpr, ComparisonOp, FairnessDecl, SpecCore, StateExpr, TemporalExpr, ValueExpr,
        ViewExpr,
    };
    use nirvash_lower::{
        ClaimedReduction, ModelInstance, ProofBackendId, ProofCertificate, ReductionClaim,
        StateQuotientReduction,
    };

    #[test]
    fn exports_smt_obligations_for_invariants() {
        let obligations = invariant_obligations(&SpecCore {
            init: StateExpr::True,
            next: ActionExpr::Unchanged(vec!["vars".to_owned()]),
            invariants: vec![StateExpr::Compare {
                op: ComparisonOp::Eq,
                lhs: ValueExpr::Field("status".to_owned()),
                rhs: ValueExpr::Literal("ready".to_owned()),
            }],
            ..SpecCore::named("DemoSpec")
        });

        assert_eq!(obligations.len(), 2);
        assert_eq!(
            obligations[0].kind,
            ProofObligationKind::InitImpliesInvariant
        );
        assert!(obligations[0].smtlib.contains("(declare-sort Val 0)"));
        assert!(obligations[0].smtlib.contains("check-sat"));
        assert!(obligations[1].smtlib.contains("status@1"));
    }

    #[test]
    fn exports_tla_module_artifact_with_theorem_skeletons() {
        let spec = SpecCore {
            init: StateExpr::True,
            next: ActionExpr::BoxAction {
                action: Box::new(ActionExpr::Ref("NextAction".to_owned())),
                view: nirvash_ir::ViewExpr::Vars,
            },
            invariants: vec![StateExpr::Ref("TypeOk".to_owned())],
            ..SpecCore::named("DemoSpec")
        };

        let bundle = ProofBundleExporter
            .export(&spec, &[])
            .expect("supported invariant fragment should export");
        let text = bundle
            .exported_artifacts
            .iter()
            .find(|artifact| artifact.kind == ExportedArtifactKind::TlaModule)
            .expect("bundle should contain a TLA module artifact")
            .content
            .as_str();
        assert!(text.contains("MODULE DemoSpec"));
        assert!(text.contains("Init =="));
        assert!(text.contains("Next =="));
        assert!(text.contains("THEOREM TypeOk_init"));
    }

    #[test]
    fn proof_bundle_exporter_emits_supported_obligations_and_artifacts() {
        let spec = SpecCore {
            init: StateExpr::True,
            next: ActionExpr::Unchanged(vec!["vars".to_owned()]),
            vars: vec![nirvash_ir::VarDecl {
                name: "vars".to_owned(),
            }],
            defs: vec![nirvash_ir::Definition {
                name: "frontend".to_owned(),
                body: "DemoSpec".to_owned(),
            }],
            invariants: vec![StateExpr::Ref("TypeOk".to_owned())],
            fairness: Vec::new(),
            temporal_props: Vec::new(),
        };
        let reduction = vec![ProofObligation::new(
            "verified_por".to_owned(),
            ProofObligationKind::PorReduction,
            "THEOREM verified_por == PORSound".to_owned(),
            "(assert PORSound)".to_owned(),
        )];
        let bundle = ProofBundleExporter
            .export(&spec, &reduction)
            .expect("supported invariant fragment should export");
        let smt = bundle
            .exported_artifacts
            .iter()
            .find(|artifact| artifact.kind == ExportedArtifactKind::SmtlibObligation)
            .expect("bundle should include SMT obligation artifacts");

        assert!(smt.content.contains("TypeOk"));
        assert!(
            bundle
                .obligations
                .iter()
                .any(|obligation| obligation.tla_theorem.contains("verified_por"))
        );
        assert_eq!(
            bundle.obligations.last().map(|obligation| obligation.kind),
            Some(ProofObligationKind::PorReduction)
        );
    }

    #[test]
    fn proof_bundle_exporter_carries_model_case_certificates() {
        let spec = SpecCore {
            init: StateExpr::True,
            next: ActionExpr::Unchanged(vec!["vars".to_owned()]),
            vars: vec![nirvash_ir::VarDecl {
                name: "vars".to_owned(),
            }],
            defs: vec![nirvash_ir::Definition {
                name: "frontend".to_owned(),
                body: "DemoSpec".to_owned(),
            }],
            invariants: vec![StateExpr::Ref("TypeOk".to_owned())],
            fairness: Vec::new(),
            temporal_props: Vec::new(),
        };
        let certificate = ProofCertificate {
            backend: ProofBackendId::Verus,
            obligation_hash: "obligations".to_owned(),
            artifact_hash: "artifacts".to_owned(),
            artifact_path: Some("proofs/verus.json".into()),
        };
        let model_case: ModelInstance<u8, ()> = ModelInstance::new("demo")
            .with_certified_reduction(nirvash_lower::CertifiedReduction::new().with_quotient(
                nirvash_lower::Certified::new(
                    StateQuotientReduction::new("identity", |_state: &u8| "same".to_owned()),
                    certificate.clone(),
                ),
            ));

        let bundle = ProofBundleExporter
            .export_model_instance(&spec, &model_case)
            .expect("certified model instance should export");

        assert_eq!(bundle.certificates, vec![certificate]);
    }

    #[test]
    fn fake_discharger_uses_bundle_hashes() {
        let spec = SpecCore {
            init: StateExpr::True,
            next: ActionExpr::Unchanged(vec!["vars".to_owned()]),
            vars: vec![nirvash_ir::VarDecl {
                name: "vars".to_owned(),
            }],
            defs: vec![nirvash_ir::Definition {
                name: "frontend".to_owned(),
                body: "DemoSpec".to_owned(),
            }],
            invariants: vec![StateExpr::Ref("TypeOk".to_owned())],
            fairness: Vec::new(),
            temporal_props: Vec::new(),
        };
        let bundle = ProofBundleExporter
            .export(&spec, &[])
            .expect("supported invariant fragment should export");
        let certificate = FakeProofDischarger::new(ProofBackendId::Tlaps)
            .with_artifact_path("proofs/tlaps.tla")
            .discharge(&bundle)
            .expect("fake discharger should certify bundle");

        assert_eq!(certificate.obligation_hash, bundle.obligation_hash());
        assert_eq!(certificate.artifact_hash, bundle.artifact_hash());
        assert_eq!(certificate.backend, ProofBackendId::Tlaps);
    }

    #[test]
    fn importer_uses_stable_bundle_hashes() {
        let spec = SpecCore {
            init: StateExpr::True,
            next: ActionExpr::Unchanged(vec!["vars".to_owned()]),
            vars: vec![nirvash_ir::VarDecl {
                name: "vars".to_owned(),
            }],
            defs: vec![nirvash_ir::Definition {
                name: "frontend".to_owned(),
                body: "DemoSpec".to_owned(),
            }],
            invariants: vec![StateExpr::Ref("TypeOk".to_owned())],
            fairness: Vec::new(),
            temporal_props: Vec::new(),
        };
        let claimed: ClaimedReduction<u8, ()> = ClaimedReduction::new().with_quotient(
            ReductionClaim::new(StateQuotientReduction::new("identity", |_state: &u8| {
                "same".to_owned()
            }))
            .with_obligation(ProofObligation::new(
                "quotient_sound",
                ProofObligationKind::StateQuotientReduction,
                "THEOREM quotient_sound == QuotientSound",
                "(assert QuotientSound)",
            )),
        );
        let model_case: ModelInstance<u8, ()> =
            ModelInstance::new("demo").with_claimed_reduction(claimed);
        let bundle = ProofBundleExporter
            .export_model_instance(&spec, &model_case)
            .expect("claimed reduction should export");

        let verus = importer::import_verus_certificate(&bundle, "proofs/verus.json");
        let refined_rust =
            importer::import_refined_rust_certificate(&bundle, "proofs/refined_rust.json");

        assert_eq!(verus.obligation_hash, refined_rust.obligation_hash);
        assert_eq!(verus.artifact_hash, refined_rust.artifact_hash);
        assert_eq!(verus.backend, ProofBackendId::Verus);
        assert_eq!(refined_rust.backend, ProofBackendId::RefinedRust);
    }

    #[test]
    fn proof_bundle_exporter_rejects_unsupported_fragments() {
        let spec = SpecCore {
            init: StateExpr::True,
            next: ActionExpr::Unchanged(vec!["vars".to_owned()]),
            invariants: vec![StateExpr::Ref("TypeOk".to_owned())],
            temporal_props: vec![TemporalExpr::Ref("EventuallyReady".to_owned())],
            fairness: vec![FairnessDecl::WF {
                view: ViewExpr::Vars,
                action: ActionExpr::Ref("NextAction".to_owned()),
            }],
            vars: vec![nirvash_ir::VarDecl {
                name: "vars".to_owned(),
            }],
            defs: vec![nirvash_ir::Definition {
                name: "frontend".to_owned(),
                body: "DemoSpec".to_owned(),
            }],
        };

        let error = ProofBundleExporter
            .export(&spec, &[])
            .expect_err("temporal/fairness fragment is not supported");
        assert!(matches!(
            error,
            ProofExportError::UnsupportedFragment(message)
                if message.contains("temporal properties")
        ));
    }
}
