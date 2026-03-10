use std::collections::{BTreeMap, BTreeSet};

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Expr, ExprRange, Field, Fields, Ident,
    ImplItem, ItemConst, ItemFn, ItemImpl, Lit, LitStr, Path, RangeLimits, Token, Type,
    parse_macro_input,
};

#[proc_macro_derive(Signature, attributes(signature, sig, signature_invariant))]
pub fn derive_signature(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_signature_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(ActionVocabulary)]
pub fn derive_action_vocabulary(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_action_vocabulary_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(RelAtom)]
pub fn derive_rel_atom(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_rel_atom_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(RelationalState)]
pub fn derive_relational_state(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_relational_state_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn subsystem_spec(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as SpecArgs);
    let item = parse_macro_input!(item as ItemImpl);
    match expand_temporal_spec(args, item, false) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn system_spec(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as SpecArgs);
    let item = parse_macro_input!(item as ItemImpl);
    match expand_temporal_spec(args, item, true) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn formal_tests(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as TestArgs);
    let _item = parse_macro_input!(item as ItemConst);
    match expand_formal_tests(args) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn code_tests(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as CodeTestArgs);
    let _item = parse_macro_input!(item as ItemConst);
    match expand_code_tests(args) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn code_witness_tests(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as CodeTestArgs);
    let _item = parse_macro_input!(item as ItemConst);
    match expand_code_witness_tests(args) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn nirvash_runtime_contract(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as RuntimeContractArgs);
    let item = parse_macro_input!(item as ItemImpl);
    match expand_runtime_contract(args, item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn nirvash_projection_contract(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ProjectionContractArgs);
    let item = parse_macro_input!(item as ItemImpl);
    match expand_projection_contract(args, item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn contract_case(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro]
pub fn code_witness_test_main(input: TokenStream) -> TokenStream {
    let _ = parse_macro_input!(input as syn::parse::Nothing);
    quote! {
        #[doc(hidden)]
        pub fn __nirvash_code_witness_main_marker() {}

        fn main() {
            let _ = __nirvash_code_witness_main_marker as fn();
            ::nirvash_core::conformance::run_registered_code_witness_tests();
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn invariant(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::Invariant)
}

#[proc_macro_attribute]
pub fn property(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::Property)
}

#[proc_macro_attribute]
pub fn fairness(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::Fairness)
}

#[proc_macro_attribute]
pub fn state_constraint(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::StateConstraint)
}

#[proc_macro_attribute]
pub fn action_constraint(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::ActionConstraint)
}

#[proc_macro_attribute]
pub fn symmetry(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::Symmetry)
}

#[derive(Default)]
struct SignatureArgs {
    custom: bool,
    range: Option<ExprRange>,
    filter: Option<Expr>,
    bounds: BTreeMap<String, FieldSigArgs>,
    helper_invariant: Option<Expr>,
}

impl SignatureArgs {
    fn from_attrs(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut args = Self::default();
        for attr in attrs {
            if attr.path().is_ident("signature_invariant") {
                if args.helper_invariant.is_some() {
                    return Err(syn::Error::new(
                        attr.span(),
                        "duplicate #[signature_invariant(...)] attribute",
                    ));
                }
                args.helper_invariant = Some(parse_self_expr_attribute(attr)?);
                continue;
            }
            if !attr.path().is_ident("signature") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("custom") {
                    args.custom = true;
                    return Ok(());
                }
                if meta.path.is_ident("domain_fn") {
                    return Err(meta.error(
                        "#[signature(domain_fn = ...)] is removed; use #[signature(custom)] and implement the generated companion trait instead",
                    ));
                }
                if meta.path.is_ident("invariant_fn") {
                    return Err(meta.error(
                        "#[signature(invariant_fn = ...)] is removed; use #[signature(custom)] and override signature_invariant() on the generated companion trait instead",
                    ));
                }
                if meta.path.is_ident("skip_invariant") {
                    return Err(meta.error(
                        "#[signature(skip_invariant)] is removed; use #[signature(custom)] and rely on the companion trait default signature_invariant() implementation instead",
                    ));
                }
                if meta.path.is_ident("range") {
                    let lit: LitStr = meta.value()?.parse()?;
                    args.range = Some(syn::parse_str(&lit.value())?);
                    return Ok(());
                }
                if meta.path.is_ident("filter") {
                    if args.filter.is_some() {
                        return Err(meta.error("duplicate signature filter"));
                    }
                    args.filter = Some(parse_self_expr_meta(&meta)?);
                    return Ok(());
                }
                if meta.path.is_ident("bounds") {
                    parse_bounds_meta(&meta, &mut args.bounds)?;
                    return Ok(());
                }
                Err(meta.error("unsupported #[signature(...)] argument"))
            })?;
        }
        Ok(args)
    }
}

#[derive(Debug, Clone, Default)]
struct FieldSigArgs {
    range: Option<ExprRange>,
    len: Option<ExprRange>,
    optional: bool,
    domain: Option<Path>,
}

impl FieldSigArgs {
    fn from_field_attrs(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut args = Self::default();
        for attr in attrs {
            if !attr.path().is_ident("sig") {
                continue;
            }
            let parsed = attr.parse_args::<FieldSigArgs>()?;
            args.merge_from_type_level(&parsed);
        }
        Ok(args)
    }

    fn merge_from_type_level(&mut self, parent: &FieldSigArgs) {
        if self.range.is_none() {
            self.range = parent.range.clone();
        }
        if self.len.is_none() {
            self.len = parent.len.clone();
        }
        if !self.optional {
            self.optional = parent.optional;
        }
        if self.domain.is_none() {
            self.domain = parent.domain.clone();
        }
    }
}

impl Parse for FieldSigArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "range" => {
                    let _ = input.parse::<Token![=]>()?;
                    let lit: LitStr = input.parse()?;
                    args.range = Some(syn::parse_str(&lit.value())?);
                }
                "len" => {
                    let _ = input.parse::<Token![=]>()?;
                    let lit: LitStr = input.parse()?;
                    args.len = Some(syn::parse_str(&lit.value())?);
                }
                "optional" => {
                    args.optional = true;
                }
                "domain" => {
                    let _ = input.parse::<Token![=]>()?;
                    args.domain = Some(input.parse()?);
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "unsupported #[sig(...)] argument",
                    ));
                }
            }
            if input.peek(Token![,]) {
                let _ = input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

fn parse_bounds_meta(
    meta: &syn::meta::ParseNestedMeta<'_>,
    bounds: &mut BTreeMap<String, FieldSigArgs>,
) -> syn::Result<()> {
    if meta.input.is_empty() {
        return Ok(());
    }

    let content;
    syn::parenthesized!(content in meta.input);
    while !content.is_empty() {
        let field_ident: Ident = content.parse()?;
        let field_name = field_ident.to_string();
        let nested;
        syn::parenthesized!(nested in content);
        let field_args = nested.parse::<FieldSigArgs>()?;
        bounds.insert(field_name, field_args);
        if content.peek(Token![,]) {
            let _ = content.parse::<Token![,]>()?;
        }
    }
    Ok(())
}

struct SelfExprAttr {
    _self_token: Token![self],
    _arrow: Token![=>],
    expr: Expr,
}

impl Parse for SelfExprAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            _self_token: input.parse()?,
            _arrow: input.parse()?,
            expr: input.parse()?,
        })
    }
}

fn parse_self_expr_attribute(attr: &Attribute) -> syn::Result<Expr> {
    attr.parse_args::<SelfExprAttr>().map(|value| value.expr)
}

fn parse_self_expr_meta(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<Expr> {
    let content;
    syn::parenthesized!(content in meta.input);
    content.parse::<SelfExprAttr>().map(|value| value.expr)
}

struct SpecArgs {
    model_cases: Option<Path>,
    subsystems: Vec<LitStr>,
}

impl syn::parse::Parse for SpecArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self {
            model_cases: None,
            subsystems: Vec::new(),
        };

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let content;
            syn::parenthesized!(content in input);
            match ident.to_string().as_str() {
                "model_cases" => args.model_cases = Some(parse_single_path(&content)?),
                "checker_config" | "doc_graph_policy" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "checker_config/doc_graph_policy は廃止されました。#[subsystem_spec(model_cases(...))] へ移行してください",
                    ));
                }
                "subsystems" => args.subsystems = parse_string_list(&content)?,
                "invariants" | "state_constraints" | "action_constraints" | "properties"
                | "fairness" | "symmetry" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "#[invariant(SpecType)] 形式へ移行せよ",
                    ));
                }
                _ => return Err(syn::Error::new(ident.span(), "unsupported macro argument")),
            }
            if input.peek(syn::Token![,]) {
                let _ = input.parse::<syn::Token![,]>()?;
            }
        }

        Ok(args)
    }
}

struct TestArgs {
    spec: Path,
    cases: Option<Ident>,
    composition: Option<Ident>,
}

impl syn::parse::Parse for TestArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut spec = None;
        let mut cases = None;
        let mut composition = None;
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let _eq: syn::Token![=] = input.parse()?;
            match ident.to_string().as_str() {
                "spec" => spec = Some(input.parse()?),
                "init" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "init = ... is no longer supported; TransitionSystem::initial_states() is the canonical source of initial states",
                    ));
                }
                "cases" => cases = Some(input.parse()?),
                "composition" => composition = Some(input.parse()?),
                _ => return Err(syn::Error::new(ident.span(), "unsupported test argument")),
            }
            if input.peek(syn::Token![,]) {
                let _ = input.parse::<syn::Token![,]>()?;
            }
        }

        Ok(Self {
            spec: spec.ok_or_else(|| syn::Error::new(Span::call_site(), "missing spec = ..."))?,
            cases,
            composition,
        })
    }
}

struct CodeTestArgs {
    spec: Path,
    binding: Path,
    cases: Option<Ident>,
}

impl syn::parse::Parse for CodeTestArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut spec = None;
        let mut binding = None;
        let mut cases = None;
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let _eq: syn::Token![=] = input.parse()?;
            match ident.to_string().as_str() {
                "spec" => spec = Some(input.parse()?),
                "binding" => binding = Some(input.parse()?),
                "init" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "init = ... is no longer supported; TransitionSystem::initial_states() is the canonical source of initial states",
                    ));
                }
                "action" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "action = ... is no longer supported; use binding = ... and implement ProtocolRuntimeBinding<Spec>",
                    ));
                }
                "driver" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "driver = ... is no longer supported; use binding = ... and implement ProtocolRuntimeBinding<Spec>",
                    ));
                }
                "fresh" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "fresh = ... is no longer supported; use binding = ... and implement fresh_runtime() on ProtocolRuntimeBinding<Spec>",
                    ));
                }
                "context" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "context = ... is no longer supported; use binding = ... and implement context() on ProtocolRuntimeBinding<Spec>",
                    ));
                }
                "harness" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "harness = ... is no longer supported; use binding = ... and implement ProtocolRuntimeBinding<Spec>",
                    ));
                }
                "probe" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "probe = ... is no longer supported; use binding = ... and implement ProtocolRuntimeBinding<Spec>",
                    ));
                }
                "cases" => cases = Some(input.parse()?),
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "unsupported code_tests argument",
                    ));
                }
            }
            if input.peek(syn::Token![,]) {
                let _ = input.parse::<syn::Token![,]>()?;
            }
        }

        Ok(Self {
            spec: spec.ok_or_else(|| syn::Error::new(Span::call_site(), "missing spec = ..."))?,
            binding: binding
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing binding = ..."))?,
            cases,
        })
    }
}

#[derive(Default)]
struct RuntimeContractTests {
    grouped: bool,
    witness: bool,
}

struct RuntimeContractArgs {
    spec: Path,
    binding: Path,
    context_ty: Type,
    context_expr: Option<Expr>,
    runtime_ty: Option<Type>,
    fresh_runtime: Expr,
    summary_ty: Option<Type>,
    output_ty: Option<Type>,
    summary_field: Option<Ident>,
    initial_summary: Option<Expr>,
    input_ty: Option<Type>,
    session_ty: Option<Type>,
    fresh_session: Option<Expr>,
    probe_context: Option<Expr>,
    tests: RuntimeContractTests,
}

struct ProjectionContractArgs {
    probe_state_ty: Type,
    probe_output_ty: Type,
    summary_state_ty: Type,
    summary_output_ty: Type,
    summarize_state: Expr,
    summarize_output: Expr,
    abstract_state: Expr,
    abstract_output: Expr,
}

impl Parse for ProjectionContractArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut probe_state_ty = None;
        let mut probe_output_ty = None;
        let mut summary_state_ty = None;
        let mut summary_output_ty = None;
        let mut summarize_state = None;
        let mut summarize_output = None;
        let mut abstract_state = None;
        let mut abstract_output = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let _eq: Token![=] = input.parse()?;
            match ident.to_string().as_str() {
                "probe_state" => probe_state_ty = Some(input.parse()?),
                "probe_output" => probe_output_ty = Some(input.parse()?),
                "summary_state" => summary_state_ty = Some(input.parse()?),
                "summary_output" => summary_output_ty = Some(input.parse()?),
                "summarize_state" => summarize_state = Some(input.parse()?),
                "summarize_output" => summarize_output = Some(input.parse()?),
                "abstract_state" => abstract_state = Some(input.parse()?),
                "abstract_output" => abstract_output = Some(input.parse()?),
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "unsupported nirvash_projection_contract argument",
                    ));
                }
            }
            if input.peek(Token![,]) {
                let _ = input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            probe_state_ty: probe_state_ty
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing probe_state = ..."))?,
            probe_output_ty: probe_output_ty
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing probe_output = ..."))?,
            summary_state_ty: summary_state_ty
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing summary_state = ..."))?,
            summary_output_ty: summary_output_ty.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing summary_output = ...")
            })?,
            summarize_state: summarize_state.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing summarize_state = ...")
            })?,
            summarize_output: summarize_output.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing summarize_output = ...")
            })?,
            abstract_state: abstract_state.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing abstract_state = ...")
            })?,
            abstract_output: abstract_output.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing abstract_output = ...")
            })?,
        })
    }
}

impl Parse for RuntimeContractArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut spec = None;
        let mut binding = None;
        let mut context_ty = None;
        let mut context_expr = None;
        let mut runtime_ty = None;
        let mut fresh_runtime = None;
        let mut summary_ty = None;
        let mut output_ty = None;
        let mut summary_field = None;
        let mut initial_summary = None;
        let mut input_ty = None;
        let mut session_ty = None;
        let mut fresh_session = None;
        let mut probe_context = None;
        let mut tests = RuntimeContractTests::default();

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if ident == "tests" {
                let content;
                syn::parenthesized!(content in input);
                while !content.is_empty() {
                    let value: Ident = content.parse()?;
                    match value.to_string().as_str() {
                        "grouped" => tests.grouped = true,
                        "witness" => tests.witness = true,
                        _ => {
                            return Err(syn::Error::new(
                                value.span(),
                                "unsupported tests(...) entry",
                            ));
                        }
                    }
                    if content.peek(Token![,]) {
                        let _ = content.parse::<Token![,]>()?;
                    }
                }
            } else {
                let _eq: Token![=] = input.parse()?;
                match ident.to_string().as_str() {
                    "spec" => spec = Some(input.parse()?),
                    "binding" => binding = Some(input.parse()?),
                    "context" => context_ty = Some(input.parse()?),
                    "context_expr" => context_expr = Some(input.parse()?),
                    "runtime" => runtime_ty = Some(input.parse()?),
                    "fresh_runtime" => fresh_runtime = Some(input.parse()?),
                    "summary" => summary_ty = Some(input.parse()?),
                    "output" => output_ty = Some(input.parse()?),
                    "summary_field" => summary_field = Some(input.parse()?),
                    "initial_summary" => initial_summary = Some(input.parse()?),
                    "input" => input_ty = Some(input.parse()?),
                    "session" => session_ty = Some(input.parse()?),
                    "fresh_session" => fresh_session = Some(input.parse()?),
                    "probe_context" => probe_context = Some(input.parse()?),
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            "unsupported nirvash_runtime_contract argument",
                        ));
                    }
                }
            }
            if input.peek(Token![,]) {
                let _ = input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            spec: spec.ok_or_else(|| syn::Error::new(Span::call_site(), "missing spec = ..."))?,
            binding: binding
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing binding = ..."))?,
            context_ty: context_ty
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing context = ..."))?,
            context_expr,
            runtime_ty,
            fresh_runtime: fresh_runtime
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing fresh_runtime = ..."))?,
            summary_ty,
            output_ty,
            summary_field,
            initial_summary,
            input_ty,
            session_ty,
            fresh_session,
            probe_context,
            tests,
        })
    }
}

#[derive(Clone)]
struct ContractCaseArgs {
    action: Expr,
    _call: Option<Expr>,
    requires: Expr,
    updates: Vec<(Ident, Expr)>,
    _positive: Option<Path>,
    _negative: Option<Path>,
    output: Option<Expr>,
    law_output: Option<Expr>,
}

impl Parse for ContractCaseArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut action = None;
        let mut call = None;
        let mut requires = None;
        let mut updates = Vec::new();
        let mut positive = None;
        let mut negative = None;
        let mut output = None;
        let mut law_output = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if ident == "update" {
                let content;
                syn::parenthesized!(content in input);
                while !content.is_empty() {
                    let field: Ident = content.parse()?;
                    let _eq: Token![=] = content.parse()?;
                    let expr: Expr = content.parse()?;
                    updates.push((field, expr));
                    if content.peek(Token![,]) {
                        let _ = content.parse::<Token![,]>()?;
                    }
                }
            } else {
                let _eq: Token![=] = input.parse()?;
                match ident.to_string().as_str() {
                    "action" => action = Some(input.parse()?),
                    "call" => call = Some(input.parse()?),
                    "requires" => requires = Some(input.parse()?),
                    "positive" => positive = Some(input.parse()?),
                    "negative" => negative = Some(input.parse()?),
                    "output" => output = Some(input.parse()?),
                    "law_output" => law_output = Some(input.parse()?),
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            "unsupported contract_case argument",
                        ));
                    }
                }
            }
            if input.peek(Token![,]) {
                let _ = input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            action: action.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "contract_case requires action = ...")
            })?,
            _call: call,
            requires: requires.unwrap_or_else(|| syn::parse_quote!(true)),
            updates,
            _positive: positive,
            _negative: negative,
            output,
            law_output,
        })
    }
}

fn parse_string_list(input: &syn::parse::ParseBuffer<'_>) -> syn::Result<Vec<LitStr>> {
    let mut values = Vec::new();
    while !input.is_empty() {
        values.push(input.parse()?);
        if input.peek(syn::Token![,]) {
            let _ = input.parse::<syn::Token![,]>()?;
        }
    }
    Ok(values)
}

fn parse_single_path(input: &syn::parse::ParseBuffer<'_>) -> syn::Result<Path> {
    let path: Path = input.parse()?;
    if !input.is_empty() {
        return Err(syn::Error::new(
            input.span(),
            "expected exactly one function path",
        ));
    }
    Ok(path)
}

struct RegistrationArgs {
    spec: Path,
    case_labels: Option<Vec<LitStr>>,
}

impl syn::parse::Parse for RegistrationArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let spec = input.parse()?;
        let mut case_labels = None;

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            let option = input.parse::<Ident>()?;
            if option != "cases" {
                return Err(syn::Error::new(
                    option.span(),
                    "unsupported registration option; expected cases(...)",
                ));
            }
            if case_labels.is_some() {
                return Err(syn::Error::new(
                    option.span(),
                    "duplicate cases(...) registration option",
                ));
            }

            let content;
            syn::parenthesized!(content in input);
            let labels = content
                .parse_terminated(|input| input.parse::<LitStr>(), Token![,])?
                .into_iter()
                .collect::<Vec<_>>();
            let mut seen = BTreeSet::new();
            for label in &labels {
                let value = label.value();
                if !seen.insert(value.clone()) {
                    return Err(syn::Error::new(
                        label.span(),
                        format!("duplicate case label `{value}`"),
                    ));
                }
            }
            case_labels = Some(labels);
        }

        Ok(Self { spec, case_labels })
    }
}

#[derive(Clone, Copy)]
enum RegistrationKind {
    Invariant,
    Property,
    Fairness,
    StateConstraint,
    ActionConstraint,
    Symmetry,
}

impl RegistrationKind {
    fn label(self) -> &'static str {
        match self {
            Self::Invariant => "invariant",
            Self::Property => "property",
            Self::Fairness => "fairness",
            Self::StateConstraint => "state_constraint",
            Self::ActionConstraint => "action_constraint",
            Self::Symmetry => "symmetry",
        }
    }

    fn registry_ident(self) -> Ident {
        match self {
            Self::Invariant => format_ident!("RegisteredInvariant"),
            Self::Property => format_ident!("RegisteredProperty"),
            Self::Fairness => format_ident!("RegisteredFairness"),
            Self::StateConstraint => format_ident!("RegisteredStateConstraint"),
            Self::ActionConstraint => format_ident!("RegisteredActionConstraint"),
            Self::Symmetry => format_ident!("RegisteredSymmetry"),
        }
    }

    fn expected_type(self, spec: &Path) -> proc_macro2::TokenStream {
        match self {
            Self::Invariant => {
                quote! { ::nirvash_core::StatePredicate<<#spec as ::nirvash_core::TransitionSystem>::State> }
            }
            Self::Property => {
                quote! { ::nirvash_core::Ltl<<#spec as ::nirvash_core::TransitionSystem>::State, <#spec as ::nirvash_core::TransitionSystem>::Action> }
            }
            Self::Fairness => {
                quote! { ::nirvash_core::Fairness<<#spec as ::nirvash_core::TransitionSystem>::State, <#spec as ::nirvash_core::TransitionSystem>::Action> }
            }
            Self::StateConstraint => {
                quote! { ::nirvash_core::StateConstraint<<#spec as ::nirvash_core::TransitionSystem>::State> }
            }
            Self::ActionConstraint => {
                quote! { ::nirvash_core::ActionConstraint<<#spec as ::nirvash_core::TransitionSystem>::State, <#spec as ::nirvash_core::TransitionSystem>::Action> }
            }
            Self::Symmetry => {
                quote! { ::nirvash_core::SymmetryReducer<<#spec as ::nirvash_core::TransitionSystem>::State> }
            }
        }
    }
}

fn expand_registration_attr(
    attr: TokenStream,
    item: TokenStream,
    kind: RegistrationKind,
) -> TokenStream {
    if attr.is_empty() {
        return syn::Error::new(
            Span::call_site(),
            "missing target spec path; use #[invariant(SpecType)]",
        )
        .to_compile_error()
        .into();
    }
    let args = parse_macro_input!(attr as RegistrationArgs);
    let item = parse_macro_input!(item as ItemFn);
    match expand_registration(args, item, kind) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_registration(
    args: RegistrationArgs,
    item: ItemFn,
    kind: RegistrationKind,
) -> syn::Result<proc_macro2::TokenStream> {
    if !item.sig.inputs.is_empty() {
        return Err(syn::Error::new(
            item.sig.inputs.span(),
            "formal registration functions must not take parameters",
        ));
    }
    if !item.sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            item.sig.generics.span(),
            "formal registration functions must not be generic",
        ));
    }

    let fn_ident = item.sig.ident.clone();
    let RegistrationArgs { spec, case_labels } = args;
    if case_labels.is_some()
        && !matches!(
            kind,
            RegistrationKind::StateConstraint | RegistrationKind::ActionConstraint
        )
    {
        return Err(syn::Error::new(
            fn_ident.span(),
            "cases(...) is only supported on #[state_constraint(...)] and #[action_constraint(...)]",
        ));
    }
    let expected = kind.expected_type(&spec);
    let registry_ident = kind.registry_ident();
    let label = kind.label();
    let build_ident = format_ident!("__nirvash_{}_build_{}", label, fn_ident);
    let spec_id_ident = format_ident!("__nirvash_{}_spec_type_id_{}", label, fn_ident);
    let case_labels_item_ident = format_ident!("__nirvash_{}_case_labels_{}", label, fn_ident);
    let case_labels_tokens = if matches!(
        kind,
        RegistrationKind::StateConstraint | RegistrationKind::ActionConstraint
    ) {
        if let Some(case_labels) = &case_labels {
            quote! {
                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                static #case_labels_item_ident: &[&str] = &[#(#case_labels),*];
            }
        } else {
            quote! {}
        }
    } else {
        quote! {}
    };
    let case_labels_field = if matches!(
        kind,
        RegistrationKind::StateConstraint | RegistrationKind::ActionConstraint
    ) {
        if case_labels.is_some() {
            quote! { case_labels: ::std::option::Option::Some(#case_labels_item_ident), }
        } else {
            quote! { case_labels: ::std::option::Option::None, }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #item

        #[doc(hidden)]
        fn #build_ident() -> ::std::boxed::Box<dyn ::std::any::Any> {
            ::std::boxed::Box::new(#fn_ident())
        }

        #[doc(hidden)]
        fn #spec_id_ident() -> ::std::any::TypeId {
            ::std::any::TypeId::of::<#spec>()
        }

        #[doc(hidden)]
        const _: fn() -> #expected = #fn_ident;

        #case_labels_tokens

        ::nirvash_core::inventory::submit! {
            ::nirvash_core::registry::#registry_ident {
                spec_type_id: #spec_id_ident,
                name: stringify!(#fn_ident),
                #case_labels_field
                build: #build_ident,
            }
        }
    })
}

fn expand_signature_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_signature_tokens(input)
}

fn formal_runtime_guard_attrs() -> proc_macro2::TokenStream {
    quote! {
        #[cfg(any(debug_assertions, test, doc))]
    }
}

fn guard_item(tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let attrs = formal_runtime_guard_attrs();
    quote! {
        #attrs
        #tokens
    }
}

fn expand_signature_tokens(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let args = SignatureArgs::from_attrs(&input.attrs)?;
    let ident = input.ident;
    let generics = input.generics;
    let trait_ident = companion_trait_ident(&ident);
    let trait_generics = trait_generics(&generics);
    let trait_where_clause = &generics.where_clause;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let supported_data = ensure_supported_signature_data(&ident, &input.data)?;
    let action_doc_registration =
        signature_action_doc_registration(&ident, &input.data, &generics)?;

    if args.custom
        && (args.range.is_some()
            || args.filter.is_some()
            || !args.bounds.is_empty()
            || args.helper_invariant.is_some())
    {
        return Err(syn::Error::new(
            ident.span(),
            "#[signature(custom)] cannot be combined with bounds, filter, range, or #[signature_invariant(...)] helpers",
        ));
    }

    let companion_trait = quote! {
        pub trait #trait_ident #trait_generics: Sized #trait_where_clause {
            fn representatives() -> ::nirvash_core::BoundedDomain<Self>;

            fn signature_invariant(&self) -> bool {
                true
            }
        }
    };

    let auto_impl = if args.custom {
        quote! {}
    } else {
        let domain_body = signature_domain_body(&ident, &input.data, &args)?;
        let invariant_body = signature_invariant_body(&ident, &input.data, &args)?;
        quote! {
            impl #impl_generics #trait_ident #ty_generics for #ident #ty_generics #where_clause {
                fn representatives() -> ::nirvash_core::BoundedDomain<Self> {
                    #domain_body
                }

                fn signature_invariant(&self) -> bool {
                    #invariant_body
                }
            }
        }
    };

    Ok(quote! {
        #supported_data
        #companion_trait
        #auto_impl

        impl #impl_generics ::nirvash_core::Signature for #ident #ty_generics #where_clause {
            fn bounded_domain() -> ::nirvash_core::BoundedDomain<Self> {
                <Self as #trait_ident #ty_generics>::representatives()
            }

            fn invariant(&self) -> bool {
                <Self as #trait_ident #ty_generics>::signature_invariant(self)
            }
        }

        #action_doc_registration
    })
}

fn expand_action_vocabulary_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_action_vocabulary_tokens(input)
}

fn expand_action_vocabulary_tokens(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let generics = input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    match &input.data {
        Data::Enum(_) => Ok(quote! {
            impl #impl_generics ::nirvash_core::ActionVocabulary for #ident #ty_generics #where_clause {
                fn action_vocabulary() -> ::std::vec::Vec<Self> {
                    <Self as ::nirvash_core::Signature>::bounded_domain().into_vec()
                }
            }
        }),
        Data::Struct(data) => Err(syn::Error::new(
            data.struct_token.span(),
            "ActionVocabulary derive requires an enum",
        )),
        Data::Union(data) => Err(syn::Error::new(
            data.union_token.span(),
            "ActionVocabulary derive does not support unions",
        )),
    }
}

fn signature_action_doc_registration(
    ident: &Ident,
    data: &Data,
    generics: &syn::Generics,
) -> syn::Result<proc_macro2::TokenStream> {
    if !generics.params.is_empty() {
        return Ok(quote! {});
    }

    let Data::Enum(data) = data else {
        return Ok(quote! {});
    };

    let match_arms = data
        .variants
        .iter()
        .map(|variant| signature_action_doc_match_arm(ident, variant))
        .collect::<syn::Result<Vec<_>>>()?;
    let ident_snake = to_upper_snake(&ident.to_string()).to_lowercase();
    let type_id_fn_ident = format_ident!("__nirvash_action_doc_type_id_{}", ident_snake);
    let format_fn_ident = format_ident!("__nirvash_action_doc_format_{}", ident_snake);
    let type_id_item = guard_item(quote! {
        #[doc(hidden)]
        fn #type_id_fn_ident() -> ::std::any::TypeId {
            ::std::any::TypeId::of::<#ident>()
        }
    });
    let format_item = guard_item(quote! {
        #[doc(hidden)]
        fn #format_fn_ident(
            value: &dyn ::std::any::Any,
        ) -> ::std::option::Option<::std::string::String> {
            let value = value
                .downcast_ref::<#ident>()
                .expect("registered action doc downcast");
            match value {
                #(#match_arms)*
            }
        }
    });
    let inventory_item = guard_item(quote! {
        ::nirvash_core::inventory::submit! {
            ::nirvash_core::RegisteredActionDocLabel {
                value_type_id: #type_id_fn_ident,
                format: #format_fn_ident,
            }
        }
    });

    Ok(quote! {
        #type_id_item
        #format_item
        #inventory_item
    })
}

fn signature_action_doc_match_arm(
    enum_ident: &Ident,
    variant: &syn::Variant,
) -> syn::Result<proc_macro2::TokenStream> {
    let variant_ident = &variant.ident;
    if let Some(summary) = first_doc_line(&variant.attrs) {
        let pattern = variant_ignore_pattern(enum_ident, variant_ident, &variant.fields);
        return Ok(quote! {
            #pattern => ::std::option::Option::Some(#summary.to_owned()),
        });
    }

    if let Some(delegate_arm) =
        single_field_delegate_arm(enum_ident, variant_ident, &variant.fields)
    {
        return Ok(delegate_arm);
    }

    let pattern = variant_ignore_pattern(enum_ident, variant_ident, &variant.fields);
    Ok(quote! {
        #pattern => ::std::option::Option::None,
    })
}

fn first_doc_line(attrs: &[Attribute]) -> Option<LitStr> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("doc") {
            return None;
        }
        let syn::Meta::NameValue(meta) = &attr.meta else {
            return None;
        };
        let Expr::Lit(expr_lit) = &meta.value else {
            return None;
        };
        let Lit::Str(lit) = &expr_lit.lit else {
            return None;
        };
        let trimmed = lit.value().trim().to_owned();
        (!trimmed.is_empty()).then(|| LitStr::new(&trimmed, lit.span()))
    })
}

fn variant_ignore_pattern(
    enum_ident: &Ident,
    variant_ident: &Ident,
    fields: &Fields,
) -> proc_macro2::TokenStream {
    match fields {
        Fields::Unit => quote! { #enum_ident::#variant_ident },
        Fields::Unnamed(_) => quote! { #enum_ident::#variant_ident(..) },
        Fields::Named(_) => quote! { #enum_ident::#variant_ident { .. } },
    }
}

fn single_field_delegate_arm(
    enum_ident: &Ident,
    variant_ident: &Ident,
    fields: &Fields,
) -> Option<proc_macro2::TokenStream> {
    match fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            let binding = format_ident!("__nirvash_inner");
            Some(quote! {
                #enum_ident::#variant_ident(#binding) => ::std::option::Option::Some(
                    ::nirvash_core::format_doc_graph_action(#binding)
                ),
            })
        }
        Fields::Named(fields) if fields.named.len() == 1 => {
            let binding = format_ident!("__nirvash_inner");
            let field_ident = fields.named.first()?.ident.as_ref()?;
            Some(quote! {
                #enum_ident::#variant_ident { #field_ident: #binding } => ::std::option::Option::Some(
                    ::nirvash_core::format_doc_graph_action(#binding)
                ),
            })
        }
        _ => None,
    }
}

fn expand_rel_atom_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_rel_atom_tokens(input)
}

fn expand_rel_atom_tokens(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let generics = input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let supported_data = ensure_supported_signature_data(&ident, &input.data)?;

    Ok(quote! {
        #supported_data

        #[doc(hidden)]
        const _: fn() -> ::nirvash_core::BoundedDomain<#ident #ty_generics> =
            <#ident #ty_generics as ::nirvash_core::Signature>::bounded_domain;

        impl #impl_generics ::nirvash_core::RelAtom for #ident #ty_generics #where_clause {
            fn rel_index(&self) -> usize {
                <Self as ::nirvash_core::Signature>::bounded_domain()
                    .into_vec()
                    .into_iter()
                    .position(|candidate| candidate == self.clone())
                    .expect("RelAtom value must belong to Signature::bounded_domain()")
            }

            fn rel_from_index(index: usize) -> ::std::option::Option<Self> {
                <Self as ::nirvash_core::Signature>::bounded_domain()
                    .into_vec()
                    .into_iter()
                    .nth(index)
            }

            fn rel_label(&self) -> ::std::string::String {
                ::std::format!("{self:?}")
            }
        }
    })
}

fn expand_relational_state_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_relational_state_tokens(input)
}

fn expand_relational_state_tokens(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "RelationalState derive does not support generic types",
        ));
    }

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => fields.named.iter().collect::<Vec<_>>(),
            _ => {
                return Err(syn::Error::new(
                    data.fields.span(),
                    "RelationalState derive requires a named struct",
                ));
            }
        },
        Data::Enum(data) => {
            return Err(syn::Error::new(
                data.enum_token.span(),
                "RelationalState derive does not support enums",
            ));
        }
        Data::Union(data) => {
            return Err(syn::Error::new(
                data.union_token.span(),
                "RelationalState derive does not support unions",
            ));
        }
    };

    let relation_fields = fields
        .into_iter()
        .filter_map(|field| {
            relation_field_kind(&field.ty).map(|_| {
                let field_ident = field.ident.as_ref().expect("named field").clone();
                let field_name = field_ident.to_string();
                let field_ty = field.ty.clone();
                (field_ident, field_name, field_ty)
            })
        })
        .collect::<Vec<_>>();

    if relation_fields.is_empty() {
        return Err(syn::Error::new(
            ident.span(),
            "RelationalState derive requires at least one RelSet<T> or Relation2<A, B> field",
        ));
    }

    let schema_entries = relation_fields
        .iter()
        .map(|(_, field_name, field_ty)| {
            quote! {
                <#field_ty as ::nirvash_core::RelationField>::relation_schema(#field_name)
            }
        })
        .collect::<Vec<_>>();
    let summary_entries = relation_fields
        .iter()
        .map(|(field_ident, field_name, field_ty)| {
            quote! {
                <#field_ty as ::nirvash_core::RelationField>::relation_summary(&self.#field_ident, #field_name)
            }
        })
        .collect::<Vec<_>>();
    let ident_snake = to_upper_snake(&ident.to_string()).to_lowercase();
    let schema_fn_ident = format_ident!("__nirvash_relational_schema_{}", ident_snake);
    let summary_fn_ident = format_ident!("__nirvash_relational_summary_{}", ident_snake);
    let type_id_fn_ident = format_ident!("__nirvash_relational_state_type_id_{}", ident_snake);
    let schema_item = guard_item(quote! {
        #[doc(hidden)]
        fn #schema_fn_ident() -> ::std::vec::Vec<::nirvash_core::RelationFieldSchema> {
            <#ident as ::nirvash_core::RelationalState>::relation_schema()
        }
    });
    let summary_item = guard_item(quote! {
        #[doc(hidden)]
        fn #summary_fn_ident(
            value: &dyn ::std::any::Any,
        ) -> ::std::vec::Vec<::nirvash_core::RelationFieldSummary> {
            <#ident as ::nirvash_core::RelationalState>::relation_summary(
                value
                    .downcast_ref::<#ident>()
                    .expect("registered RelationalState downcast")
            )
        }
    });
    let type_id_item = guard_item(quote! {
        #[doc(hidden)]
        fn #type_id_fn_ident() -> ::std::any::TypeId {
            ::std::any::TypeId::of::<#ident>()
        }
    });
    let inventory_item = guard_item(quote! {
        ::nirvash_core::inventory::submit! {
            ::nirvash_core::RegisteredRelationalState {
                state_type_id: #type_id_fn_ident,
                relation_schema: #schema_fn_ident,
                relation_summary: #summary_fn_ident,
            }
        }
    });

    Ok(quote! {
        impl ::nirvash_core::RelationalState for #ident {
            fn relation_schema() -> ::std::vec::Vec<::nirvash_core::RelationFieldSchema> {
                ::std::vec![#(#schema_entries),*]
            }

            fn relation_summary(&self) -> ::std::vec::Vec<::nirvash_core::RelationFieldSummary> {
                ::std::vec![#(#summary_entries),*]
            }
        }

        #schema_item
        #summary_item
        #type_id_item
        #inventory_item
    })
}

fn relation_field_kind(ty: &Type) -> Option<&'static str> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    match segment.ident.to_string().as_str() {
        "RelSet" => Some("set"),
        "Relation2" => Some("binary"),
        _ => None,
    }
}

fn companion_trait_ident(ident: &Ident) -> Ident {
    format_ident!("{ident}SignatureSpec")
}

fn trait_generics(generics: &syn::Generics) -> proc_macro2::TokenStream {
    if generics.params.is_empty() {
        quote! {}
    } else {
        let params = &generics.params;
        quote! { <#params> }
    }
}

fn ensure_supported_signature_data(
    ident: &Ident,
    data: &Data,
) -> syn::Result<proc_macro2::TokenStream> {
    match data {
        Data::Enum(_) | Data::Struct(_) => Ok(quote! {}),
        Data::Union(data) => Err(syn::Error::new(
            data.union_token.span(),
            format!("Signature derive does not support unions for `{ident}`"),
        )),
    }
}

fn signature_domain_body(
    ident: &Ident,
    data: &Data,
    args: &SignatureArgs,
) -> syn::Result<proc_macro2::TokenStream> {
    if !args.bounds.is_empty() && !matches!(data, Data::Struct(_)) {
        return Err(syn::Error::new(
            ident.span(),
            "#[signature(bounds(...))] is only supported on named structs",
        ));
    }

    let domain = if let Some(range) = &args.range {
        let Data::Struct(data) = data else {
            return Err(syn::Error::new(
                ident.span(),
                "#[signature(range = ...)] is only supported on structs",
            ));
        };
        if data.fields.len() != 1 {
            return Err(syn::Error::new(
                ident.span(),
                "#[signature(range = ...)] requires a single-field newtype",
            ));
        }
        let iter = range_tokens(range)?;
        quote! {
            ::nirvash_core::BoundedDomain::new((#iter).map(Self).collect())
        }
    } else {
        match data {
            Data::Enum(data) => enum_domain_body(data)?,
            Data::Struct(data) => struct_domain_body(data, &args.bounds)?,
            Data::Union(data) => {
                return Err(syn::Error::new(
                    data.union_token.span(),
                    "Signature derive does not support unions",
                ));
            }
        }
    };

    if let Some(filter_expr) = &args.filter {
        let binding = format_ident!("__nirvash_self");
        let rewritten = rewrite_self_expr(filter_expr, &binding);
        Ok(quote! {{
            let __nirvash_domain = { #domain };
            __nirvash_domain.filter(|#binding| { #rewritten })
        }})
    } else {
        Ok(domain)
    }
}

fn signature_invariant_body(
    ident: &Ident,
    data: &Data,
    args: &SignatureArgs,
) -> syn::Result<proc_macro2::TokenStream> {
    let base = if let Some(range) = &args.range {
        let Data::Struct(data) = data else {
            return Err(syn::Error::new(
                ident.span(),
                "#[signature(range = ...)] is only supported on structs",
            ));
        };
        if data.fields.len() != 1 {
            return Err(syn::Error::new(
                ident.span(),
                "#[signature(range = ...)] requires a single-field newtype",
            ));
        }
        quote! { (#range).contains(&self.0) }
    } else {
        match data {
            Data::Enum(data) => enum_invariant_body(data)?,
            Data::Struct(data) => struct_invariant_body(data, &args.bounds)?,
            Data::Union(data) => {
                return Err(syn::Error::new(
                    data.union_token.span(),
                    "Signature derive does not support unions",
                ));
            }
        }
    };

    if let Some(invariant_expr) = &args.helper_invariant {
        let binding = format_ident!("__nirvash_self");
        let rewritten = rewrite_self_expr(invariant_expr, &binding);
        Ok(quote! {{
            let #binding = self;
            (#base) && { #rewritten }
        }})
    } else {
        Ok(base)
    }
}

fn enum_domain_body(data: &DataEnum) -> syn::Result<proc_macro2::TokenStream> {
    let mut variants = Vec::new();
    for variant in &data.variants {
        let variant_ident = &variant.ident;
        variants.push(match &variant.fields {
            Fields::Unit => quote! { values.push(Self::#variant_ident); },
            Fields::Unnamed(fields) => {
                let bindings = field_bindings(&fields.unnamed);
                let domain_exprs = fields
                    .unnamed
                    .iter()
                    .map(|field| field_domain_expr(field, None))
                    .collect::<syn::Result<Vec<_>>>()?;
                let construct = quote! {
                    Self::#variant_ident(#(#bindings.clone()),*)
                };
                nested_loops(
                    &bindings,
                    &domain_exprs,
                    quote! { values.push(#construct); },
                )
            }
            Fields::Named(fields) => {
                let bindings = named_field_bindings(&fields.named);
                let domain_exprs = fields
                    .named
                    .iter()
                    .map(|field| field_domain_expr(field, None))
                    .collect::<syn::Result<Vec<_>>>()?;
                let names = fields
                    .named
                    .iter()
                    .map(|field| field.ident.as_ref().expect("named"));
                let construct = quote! {
                    Self::#variant_ident { #(#names: #bindings.clone()),* }
                };
                nested_loops(
                    &bindings,
                    &domain_exprs,
                    quote! { values.push(#construct); },
                )
            }
        });
    }

    Ok(quote! {
        let mut values = Vec::new();
        #(#variants)*
        ::nirvash_core::BoundedDomain::new(values)
    })
}

fn struct_domain_body(
    data: &DataStruct,
    type_level_bounds: &BTreeMap<String, FieldSigArgs>,
) -> syn::Result<proc_macro2::TokenStream> {
    if !type_level_bounds.is_empty() && !matches!(data.fields, Fields::Named(_)) {
        return Err(syn::Error::new(
            data.fields.span(),
            "#[signature(bounds(...))] is only supported on named structs",
        ));
    }

    match &data.fields {
        Fields::Unit => Ok(quote! { ::nirvash_core::BoundedDomain::singleton(Self) }),
        Fields::Unnamed(fields) => {
            let bindings = field_bindings(&fields.unnamed);
            let domain_exprs = fields
                .unnamed
                .iter()
                .map(|field| field_domain_expr(field, None))
                .collect::<syn::Result<Vec<_>>>()?;
            let construct = quote! { Self(#(#bindings.clone()),*) };
            let loops = nested_loops(
                &bindings,
                &domain_exprs,
                quote! { values.push(#construct); },
            );
            Ok(quote! {
                let mut values = Vec::new();
                #loops
                ::nirvash_core::BoundedDomain::new(values)
            })
        }
        Fields::Named(fields) => {
            let bindings = named_field_bindings(&fields.named);
            let domain_exprs = fields
                .named
                .iter()
                .map(|field| {
                    let bounds = field
                        .ident
                        .as_ref()
                        .and_then(|ident| type_level_bounds.get(&ident.to_string()));
                    field_domain_expr(field, bounds)
                })
                .collect::<syn::Result<Vec<_>>>()?;
            let names = fields
                .named
                .iter()
                .map(|field| field.ident.as_ref().expect("named"));
            let construct = quote! { Self { #(#names: #bindings.clone()),* } };
            let loops = nested_loops(
                &bindings,
                &domain_exprs,
                quote! { values.push(#construct); },
            );
            Ok(quote! {
                let mut values = Vec::new();
                #loops
                ::nirvash_core::BoundedDomain::new(values)
            })
        }
    }
}

fn field_domain_expr(
    field: &Field,
    type_level_args: Option<&FieldSigArgs>,
) -> syn::Result<TokenStream2> {
    let mut args = FieldSigArgs::from_field_attrs(&field.attrs)?;
    if let Some(parent) = type_level_args {
        args.merge_from_type_level(parent);
    }

    if args.optional && option_inner_type(&field.ty).is_none() {
        return Err(syn::Error::new(
            field.ty.span(),
            "#[sig(optional)] is only supported on Option<T> fields",
        ));
    }

    if let Some(domain) = args.domain {
        return Ok(quote! { ::nirvash_core::into_bounded_domain(#domain()) });
    }

    if let Some(len) = args.len {
        let Some(element_ty) = vec_inner_type(&field.ty) else {
            return Err(syn::Error::new(
                field.ty.span(),
                "#[sig(len = ...)] is only supported on Vec<T> fields",
            ));
        };
        let iter = range_tokens(&len)?;
        return Ok(quote! {{
            let mut __nirvash_values = Vec::new();
            for __nirvash_len in #iter {
                __nirvash_values.extend(
                    ::nirvash_core::bounded_vec_domain::<#element_ty>(
                        __nirvash_len as usize,
                        __nirvash_len as usize,
                    )
                    .into_vec(),
                );
            }
            ::nirvash_core::BoundedDomain::new(__nirvash_values)
        }});
    }

    if let Some(range) = args.range {
        let iter = range_tokens(&range)?;
        return Ok(quote! { ::nirvash_core::BoundedDomain::new((#iter).collect()) });
    }

    let ty = &field.ty;
    Ok(quote! { <#ty as ::nirvash_core::Signature>::bounded_domain() })
}

fn vec_inner_type(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Vec" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    match args.args.first()? {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

fn option_inner_type(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    match args.args.first()? {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

fn rewrite_self_expr(expr: &Expr, replacement: &Ident) -> TokenStream2 {
    rewrite_self_tokens(expr.to_token_stream(), replacement)
}

fn rewrite_self_tokens(tokens: TokenStream2, replacement: &Ident) -> TokenStream2 {
    tokens
        .into_iter()
        .map(|token| match token {
            TokenTree::Group(group) => {
                let mut rewritten = proc_macro2::Group::new(
                    group.delimiter(),
                    rewrite_self_tokens(group.stream(), replacement),
                );
                rewritten.set_span(group.span());
                TokenTree::Group(rewritten)
            }
            TokenTree::Ident(ident) if ident == "self" => TokenTree::Ident(replacement.clone()),
            other => other,
        })
        .collect()
}

fn enum_invariant_body(data: &DataEnum) -> syn::Result<proc_macro2::TokenStream> {
    let arms = data.variants.iter().map(|variant| {
        let ident = &variant.ident;
        match &variant.fields {
            Fields::Unit => quote! { Self::#ident => true },
            Fields::Unnamed(fields) => {
                let bindings = field_bindings(&fields.unnamed);
                let checks = fields
                    .unnamed
                    .iter()
                    .zip(bindings.iter())
                    .map(|(field, binding)| field_invariant_expr(field, None, quote! { #binding }))
                    .collect::<syn::Result<Vec<_>>>()
                    .expect("enum invariant generation");
                quote! {
                    Self::#ident(#(#bindings),*) => true #(&& #checks)*
                }
            }
            Fields::Named(fields) => {
                let bindings = named_field_bindings(&fields.named);
                let names = fields
                    .named
                    .iter()
                    .map(|field| field.ident.as_ref().expect("named"));
                let checks = fields
                    .named
                    .iter()
                    .zip(bindings.iter())
                    .map(|(field, binding)| field_invariant_expr(field, None, quote! { #binding }))
                    .collect::<syn::Result<Vec<_>>>()
                    .expect("enum invariant generation");
                quote! {
                    Self::#ident { #(#names: #bindings),* } => true #(&& #checks)*
                }
            }
        }
    });
    Ok(quote! { match self { #(#arms),* } })
}

fn struct_invariant_body(
    data: &DataStruct,
    type_level_bounds: &BTreeMap<String, FieldSigArgs>,
) -> syn::Result<proc_macro2::TokenStream> {
    match &data.fields {
        Fields::Unit => Ok(quote! { true }),
        Fields::Unnamed(fields) => {
            let checks = fields
                .unnamed
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let access = syn::Index::from(index);
                    field_invariant_expr(field, None, quote! { self.#access })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(quote! { true #(&& #checks)* })
        }
        Fields::Named(fields) => {
            let checks = fields
                .named
                .iter()
                .map(|field| {
                    let ident = field.ident.as_ref().expect("named");
                    let bounds = type_level_bounds.get(&ident.to_string());
                    field_invariant_expr(field, bounds, quote! { self.#ident })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(quote! { true #(&& #checks)* })
        }
    }
}

fn field_invariant_expr(
    field: &Field,
    type_level_args: Option<&FieldSigArgs>,
    access: TokenStream2,
) -> syn::Result<TokenStream2> {
    let mut args = FieldSigArgs::from_field_attrs(&field.attrs)?;
    if let Some(parent) = type_level_args {
        args.merge_from_type_level(parent);
    }

    if let Some(range) = args.range {
        return Ok(quote! { (#range).contains(&#access) });
    }

    if let Some(len) = args.len {
        let Some(element_ty) = vec_inner_type(&field.ty) else {
            return Err(syn::Error::new(
                field.ty.span(),
                "#[sig(len = ...)] is only supported on Vec<T> fields",
            ));
        };
        return Ok(quote! {
            (#len).contains(&#access.len())
                && #access.iter().all(<#element_ty as ::nirvash_core::Signature>::invariant)
        });
    }

    if args.optional && option_inner_type(&field.ty).is_none() {
        return Err(syn::Error::new(
            field.ty.span(),
            "#[sig(optional)] is only supported on Option<T> fields",
        ));
    }

    let ty = &field.ty;
    Ok(quote! { <#ty as ::nirvash_core::Signature>::invariant(&#access) })
}

fn field_bindings(fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>) -> Vec<Ident> {
    fields
        .iter()
        .enumerate()
        .map(|(index, _)| format_ident!("field_{index}"))
        .collect()
}

fn named_field_bindings(
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
) -> Vec<Ident> {
    fields
        .iter()
        .map(|field| {
            field
                .ident
                .as_ref()
                .map(|ident| format_ident!("{ident}_value"))
                .expect("named field")
        })
        .collect()
}

fn nested_loops(
    bindings: &[Ident],
    domain_exprs: &[TokenStream2],
    inner: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    bindings
        .iter()
        .enumerate()
        .zip(domain_exprs.iter())
        .rev()
        .fold(inner, |acc, ((index, binding), domain_expr)| {
            let domain_ident = format_ident!("__nirvash_domain_{index}");
            quote! {
                let #domain_ident = #domain_expr;
                for #binding in &#domain_ident.into_vec() {
                    #acc
                }
            }
        })
}

fn range_tokens(range: &ExprRange) -> syn::Result<proc_macro2::TokenStream> {
    let start = range
        .start
        .as_ref()
        .ok_or_else(|| syn::Error::new(range.span(), "range start is required"))?;
    let end = range
        .end
        .as_ref()
        .ok_or_else(|| syn::Error::new(range.span(), "range end is required"))?;
    Ok(match range.limits {
        RangeLimits::Closed(_) => quote! { #start ..= #end },
        RangeLimits::HalfOpen(_) => quote! { #start .. #end },
    })
}

fn expand_temporal_spec(
    args: SpecArgs,
    item: ItemImpl,
    emit_composition: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    let self_ty = (*item.self_ty).clone();
    let doc_attrs = doc_fragment_attrs(&self_ty)?;
    let state_ty = associated_type(&item, "State")?;
    let action_ty = associated_type(&item, "Action")?;

    let model_cases = args.model_cases;
    let subsystems = args.subsystems;

    if !emit_composition && !subsystems.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "subsystems(...) is only supported on #[system_spec]",
        ));
    }

    let model_cases_expr = if let Some(model_cases) = model_cases {
        quote! { #model_cases() }
    } else {
        quote! { vec![::nirvash_core::ModelCase::default()] }
    };

    let composition_impl = if emit_composition {
        let subsystem_calls = subsystems.iter().map(|name| {
            quote! {
                composition = composition.with_subsystem(#name);
            }
        });
        let subsystem_values = subsystems.iter();
        quote! {
            impl #self_ty {
                pub const fn registered_subsystems() -> &'static [&'static str] {
                    &[#(#subsystem_values),*]
                }

                pub fn composition(&self) -> ::nirvash_core::SystemComposition<#state_ty, #action_ty> {
                    let mut composition = ::nirvash_core::SystemComposition::new(self.name());
                    #(#subsystem_calls)*
                    for invariant in <#self_ty as ::nirvash_core::TemporalSpec>::invariants(self) {
                        composition = composition.with_invariant(invariant);
                    }
                    for property in <#self_ty as ::nirvash_core::TemporalSpec>::properties(self) {
                        composition = composition.with_property(property);
                    }
                    for fairness in <#self_ty as ::nirvash_core::TemporalSpec>::fairness(self) {
                        composition = composition.with_fairness(fairness);
                    }
                    for model_case in <#self_ty as ::nirvash_core::ModelCaseSource>::model_cases(self) {
                        composition = composition.with_model_case(model_case);
                    }
                    composition
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #(#doc_attrs)*
        #item

        impl ::nirvash_core::TemporalSpec for #self_ty {
            fn invariants(&self) -> Vec<::nirvash_core::StatePredicate<Self::State>> {
                ::nirvash_core::registry::collect_invariants::<Self>()
            }

            fn properties(&self) -> Vec<::nirvash_core::Ltl<Self::State, Self::Action>> {
                ::nirvash_core::registry::collect_properties::<Self>()
            }

            fn fairness(&self) -> Vec<::nirvash_core::Fairness<Self::State, Self::Action>> {
                ::nirvash_core::registry::collect_fairness::<Self>()
            }
        }

        impl ::nirvash_core::ModelCaseSource for #self_ty {
            fn model_cases(&self) -> Vec<::nirvash_core::ModelCase<Self::State, Self::Action>> {
                let mut model_cases = #model_cases_expr;
                if model_cases.is_empty() {
                    model_cases.push(::nirvash_core::ModelCase::default());
                }
                ::nirvash_core::registry::apply_registered_model_case_metadata::<Self>(&mut model_cases);
                model_cases
            }
        }

        #composition_impl
    })
}

fn associated_type(item: &ItemImpl, name: &str) -> syn::Result<Type> {
    item.items
        .iter()
        .find_map(|impl_item| match impl_item {
            ImplItem::Type(assoc) if assoc.ident == name => Some(assoc.ty.clone()),
            _ => None,
        })
        .ok_or_else(|| syn::Error::new(item.self_ty.span(), format!("missing type {name} = ...")))
}

fn expand_formal_tests(args: TestArgs) -> syn::Result<proc_macro2::TokenStream> {
    let spec_ty = args.spec;
    let spec_tail = path_tail_ident(&spec_ty)?.clone();
    let cases_method = args.cases;
    let composition_method = args.composition;
    let module_ident = format_ident!(
        "__nirvash_generated_tests_{}",
        spec_tail.to_string().to_lowercase()
    );
    let doc_provider_ident = format_ident!("__NirvashDocGraphProvider{}", spec_tail);
    let doc_provider_build_ident = format_ident!(
        "__nirvash_doc_graph_provider_build_{}",
        spec_tail.to_string().to_lowercase()
    );
    let doc_provider_link_ident = format_ident!(
        "__nirvash_doc_graph_provider_link_{}",
        spec_tail.to_string().to_lowercase()
    );
    let spec_name = LitStr::new(&spec_tail.to_string(), spec_tail.span());

    let cases_expr = if let Some(cases_method) = cases_method {
        quote! { #cases_method() }
    } else {
        quote! { vec![<#spec_ty as ::core::default::Default>::default()] }
    };

    let composition_test = composition_method.map(|composition_method| {
        quote! {
            #[test]
            fn generated_composition_matches_temporal_spec() {
                for spec in generated_cases() {
                    let composition = spec.#composition_method();
                    let expected_invariants = <#spec_ty as ::nirvash_core::TemporalSpec>::invariants(&spec)
                        .into_iter()
                        .map(|predicate| predicate.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_properties = <#spec_ty as ::nirvash_core::TemporalSpec>::properties(&spec)
                        .into_iter()
                        .map(|property| property.describe())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_fairness = <#spec_ty as ::nirvash_core::TemporalSpec>::fairness(&spec)
                        .into_iter()
                        .map(|fairness| fairness.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_model_cases = <#spec_ty as ::nirvash_core::ModelCaseSource>::model_cases(&spec);

                    assert_eq!(composition.subsystems(), <#spec_ty>::registered_subsystems());
                    assert_eq!(composition.invariants().iter().map(|predicate| predicate.name()).collect::<::std::vec::Vec<_>>(), expected_invariants);
                    assert_eq!(composition.properties().iter().map(|property| property.describe()).collect::<::std::vec::Vec<_>>(), expected_properties);
                    assert_eq!(composition.fairness().iter().map(|fairness| fairness.name()).collect::<::std::vec::Vec<_>>(), expected_fairness);
                    assert_eq!(composition.model_cases().len(), expected_model_cases.len());
                    for (actual, expected) in composition.model_cases().iter().zip(expected_model_cases.iter()) {
                        assert_eq!(actual.label(), expected.label());
                        assert_eq!(
                            actual.state_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>(),
                            expected.state_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>()
                        );
                        assert_eq!(
                            actual.action_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>(),
                            expected.action_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>()
                        );
                        assert_eq!(actual.symmetry().map(|symmetry| symmetry.name()), expected.symmetry().map(|symmetry| symmetry.name()));
                        assert_eq!(actual.effective_checker_config(), expected.effective_checker_config());
                        assert_eq!(actual.doc_checker_config(), expected.doc_checker_config());
                        assert_eq!(actual.doc_graph_policy().reduction, expected.doc_graph_policy().reduction);
                        assert_eq!(actual.doc_graph_policy().max_edge_actions_in_label, expected.doc_graph_policy().max_edge_actions_in_label);
                        assert_eq!(
                            actual.doc_graph_policy().focus_states.iter().map(|predicate| predicate.name()).collect::<::std::vec::Vec<_>>(),
                            expected.doc_graph_policy().focus_states.iter().map(|predicate| predicate.name()).collect::<::std::vec::Vec<_>>()
                        );
                    }
                }
            }
        }
    });

    Ok(quote! {
        #[doc(hidden)]
        struct #doc_provider_ident;

        impl ::nirvash_core::DocGraphProvider for #doc_provider_ident {
            fn spec_name(&self) -> &'static str {
                #spec_name
            }

            fn cases(&self) -> ::std::vec::Vec<::nirvash_core::DocGraphCase> {
                let specs = #cases_expr;
                let multiple_cases = specs.len() > 1;
                specs
                    .into_iter()
                    .enumerate()
                    .flat_map(|(index, spec)| {
                        let model_cases = <#spec_ty as ::nirvash_core::ModelCaseSource>::model_cases(&spec);
                        let multiple_model_cases = model_cases.len() > 1;
                        model_cases
                            .into_iter()
                            .map(move |model_case| {
                                let label = match (multiple_cases, multiple_model_cases) {
                                    (false, false) => "default".to_owned(),
                                    (false, true) => model_case.label().to_owned(),
                                    (true, false) => format!("case-{index}"),
                                    (true, true) => format!("case-{index}/{}", model_case.label()),
                                };
                                let snapshot = ::nirvash_core::ModelChecker::for_case(&spec, model_case.clone())
                                    .reachable_graph_snapshot()
                                    .expect("reachable graph snapshot should build for docs");
                                let states = snapshot.states;
                                let edges = snapshot
                                    .edges
                                    .iter()
                                    .map(|outgoing| {
                                        outgoing
                                            .iter()
                                            .map(|edge| ::nirvash_core::DocGraphEdge {
                                                label: ::nirvash_core::format_doc_graph_action(&edge.action),
                                                target: edge.target,
                                            })
                                            .collect::<::std::vec::Vec<_>>()
                                    })
                                    .collect::<::std::vec::Vec<_>>();
                                let focus_indices = states
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(state_index, state)| {
                                        model_case
                                            .doc_graph_policy()
                                            .focus_states
                                            .iter()
                                            .any(|predicate| predicate.eval(state))
                                            .then_some(state_index)
                                    })
                                    .collect::<::std::vec::Vec<_>>();
                                ::nirvash_core::DocGraphCase {
                                    label,
                                    graph: ::nirvash_core::DocGraphSnapshot {
                                        states: states
                                            .into_iter()
                                            .map(|state| ::nirvash_core::summarize_doc_graph_state(&state))
                                            .collect(),
                                        edges,
                                        initial_indices: snapshot.initial_indices,
                                        deadlocks: snapshot.deadlocks,
                                        truncated: snapshot.truncated,
                                        stutter_omitted: snapshot.stutter_omitted,
                                        focus_indices,
                                        reduction: model_case.doc_graph_policy().reduction,
                                        max_edge_actions_in_label: model_case.doc_graph_policy().max_edge_actions_in_label,
                                    },
                                }
                            })
                            .collect::<::std::vec::Vec<_>>()
                    })
                    .collect()
            }
        }

        #[doc(hidden)]
        fn #doc_provider_build_ident() -> ::std::boxed::Box<dyn ::nirvash_core::DocGraphProvider> {
            ::std::boxed::Box::new(#doc_provider_ident)
        }

        #[doc(hidden)]
        pub fn #doc_provider_link_ident() {
            let _ = #doc_provider_build_ident as fn() -> ::std::boxed::Box<dyn ::nirvash_core::DocGraphProvider>;
        }

        ::nirvash_core::inventory::submit! {
            ::nirvash_core::RegisteredDocGraphProvider {
                spec_name: #spec_name,
                build: #doc_provider_build_ident,
            }
        }

        #[cfg(test)]
        mod #module_ident {
            use super::*;

            type GeneratedState = <#spec_ty as ::nirvash_core::TransitionSystem>::State;
            type GeneratedAction = <#spec_ty as ::nirvash_core::TransitionSystem>::Action;
            type GeneratedModelCase = ::nirvash_core::ModelCase<GeneratedState, GeneratedAction>;
            const GENERATED_FORMAL_CHECK_ENV: &str = "NIRVASH_FORMAL_CHECK";
            const GENERATED_FORMAL_SPEC_INDEX_ENV: &str = "NIRVASH_FORMAL_SPEC_INDEX";
            const GENERATED_FORMAL_MODEL_CASE_INDEX_ENV: &str =
                "NIRVASH_FORMAL_MODEL_CASE_INDEX";

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            fn generated_model_cases(spec: &#spec_ty) -> ::std::vec::Vec<GeneratedModelCase> {
                <#spec_ty as ::nirvash_core::ModelCaseSource>::model_cases(spec)
            }

            fn generated_snapshot(
                spec: &#spec_ty,
                model_case: GeneratedModelCase,
            ) -> ::nirvash_core::ReachableGraphSnapshot<GeneratedState, GeneratedAction> {
                ::nirvash_core::ModelChecker::for_case(spec, model_case)
                    .full_reachable_graph_snapshot()
                    .expect("reachable graph snapshot should build")
            }

            fn selected_formal_case(
                expected_check: &str,
            ) -> ::core::option::Option<(#spec_ty, GeneratedModelCase)> {
                if ::std::env::var(GENERATED_FORMAL_CHECK_ENV).ok().as_deref()
                    != ::core::option::Option::Some(expected_check)
                {
                    return ::core::option::Option::None;
                }
                let spec_index = ::std::env::var(GENERATED_FORMAL_SPEC_INDEX_ENV)
                    .expect("formal spec index env should exist")
                    .parse::<usize>()
                    .expect("formal spec index env should be usize");
                let model_case_index = ::std::env::var(GENERATED_FORMAL_MODEL_CASE_INDEX_ENV)
                    .expect("formal model case env should exist")
                    .parse::<usize>()
                    .expect("formal model case env should be usize");
                let spec = generated_cases()
                    .into_iter()
                    .nth(spec_index)
                    .expect("formal spec index should resolve");
                let model_case = generated_model_cases(&spec)
                    .into_iter()
                    .nth(model_case_index)
                    .expect("formal model case index should resolve");
                ::core::option::Option::Some((spec, model_case))
            }

            fn generated_test_filter(test_name: &str) -> ::std::string::String {
                let module_path = module_path!();
                let relative_module = module_path
                    .split_once("::")
                    .map(|(_, rest)| rest)
                    .unwrap_or(module_path);
                format!("{relative_module}::{test_name}")
            }

            fn run_formal_cases_in_subprocesses(expected_check: &str, driver_test_name: &str) {
                let current_exe =
                    ::std::env::current_exe().expect("current test binary should resolve");
                let driver_filter = generated_test_filter(driver_test_name);
                for (spec_index, spec) in generated_cases().into_iter().enumerate() {
                    for (model_case_index, model_case) in
                        generated_model_cases(&spec).into_iter().enumerate()
                    {
                        let output = ::std::process::Command::new(&current_exe)
                            .arg("--exact")
                            .arg(&driver_filter)
                            .arg("--nocapture")
                            .env(GENERATED_FORMAL_CHECK_ENV, expected_check)
                            .env(GENERATED_FORMAL_SPEC_INDEX_ENV, spec_index.to_string())
                            .env(
                                GENERATED_FORMAL_MODEL_CASE_INDEX_ENV,
                                model_case_index.to_string(),
                            )
                            .output()
                            .expect("formal case subprocess should launch");
                        assert!(
                            output.status.success(),
                            "formal case subprocess failed for {}[spec {}, model_case {}:{}]\nstdout:\n{}\nstderr:\n{}",
                            expected_check,
                            spec_index,
                            model_case_index,
                            model_case.label(),
                            ::std::string::String::from_utf8_lossy(&output.stdout),
                            ::std::string::String::from_utf8_lossy(&output.stderr),
                        );
                    }
                }
            }

            #[test]
            fn generated_initial_states_satisfy_invariants() {
                for spec in generated_cases() {
                    let invariants = <#spec_ty as ::nirvash_core::TemporalSpec>::invariants(&spec);
                    for model_case in generated_model_cases(&spec) {
                        let initial_states = <#spec_ty as ::nirvash_core::TransitionSystem>::initial_states(&spec);
                        assert!(!initial_states.is_empty(), "spec should declare at least one initial state");
                        for state in initial_states {
                            assert!(<#spec_ty as ::nirvash_core::TransitionSystem>::contains_initial(&spec, &state));
                            assert!(
                                invariants.iter().all(|predicate| predicate.eval(&state)),
                                "registered invariant failed for initial state {:?}",
                                state
                            );
                            assert!(
                                model_case.state_constraints().iter().all(|constraint| constraint.eval(&state)),
                                "state constraint failed for initial state {:?}",
                                state
                            );
                        }
                    }
                }
            }

            #[test]
            fn generated_model_checker_accepts_spec() {
                run_formal_cases_in_subprocesses(
                    "model_checker_accepts_spec",
                    "generated_model_checker_accepts_spec_case",
                );
            }

            #[test]
            fn generated_model_checker_accepts_spec_case() {
                let ::core::option::Option::Some((spec, model_case)) =
                    selected_formal_case("model_checker_accepts_spec")
                else {
                    return;
                };
                let checker = ::nirvash_core::ModelChecker::for_case(&spec, model_case);
                let result = checker.check_all().expect("model checker should run");
                assert!(result.is_ok(), "{:?}", result.violations());
            }

            #[test]
            fn generated_reachable_states_satisfy_registered_state_predicates() {
                run_formal_cases_in_subprocesses(
                    "reachable_states_satisfy_registered_state_predicates",
                    "generated_reachable_states_satisfy_registered_state_predicates_case",
                );
            }

            #[test]
            fn generated_reachable_states_satisfy_registered_state_predicates_case() {
                let ::core::option::Option::Some((spec, model_case)) =
                    selected_formal_case("reachable_states_satisfy_registered_state_predicates")
                else {
                    return;
                };
                let invariants = <#spec_ty as ::nirvash_core::TemporalSpec>::invariants(&spec);
                let snapshot = generated_snapshot(&spec, model_case.clone());
                for state in snapshot.states {
                    assert!(
                        invariants.iter().all(|predicate| predicate.eval(&state)),
                        "registered invariant failed for state {:?}",
                        state
                    );
                    assert!(
                        model_case.state_constraints().iter().all(|constraint| constraint.eval(&state)),
                        "state constraint failed for state {:?}",
                        state
                    );
                }
            }

            #[test]
            fn generated_reachable_transitions_respect_constraints() {
                run_formal_cases_in_subprocesses(
                    "reachable_transitions_respect_constraints",
                    "generated_reachable_transitions_respect_constraints_case",
                );
            }

            #[test]
            fn generated_reachable_transitions_respect_constraints_case() {
                let ::core::option::Option::Some((spec, model_case)) =
                    selected_formal_case("reachable_transitions_respect_constraints")
                else {
                    return;
                };
                let snapshot = generated_snapshot(&spec, model_case.clone());
                for (source, edges) in snapshot.edges.iter().enumerate() {
                    let prev = &snapshot.states[source];
                    for edge in edges {
                        let next = &snapshot.states[edge.target];
                        assert!(
                            model_case.state_constraints().iter().all(|constraint| constraint.eval(next)),
                            "reachable transition produced state violating state constraints: {:?}",
                            next
                        );
                        assert!(
                            model_case.action_constraints().iter().all(|constraint| constraint.eval(prev, &edge.action, next)),
                            "reachable transition violated action constraints: {:?} -- {:?} --> {:?}",
                            prev,
                            edge.action,
                            next
                        );
                    }
                }
            }

            #composition_test
        }
    })
}

fn expand_code_tests(args: CodeTestArgs) -> syn::Result<proc_macro2::TokenStream> {
    let spec_ty = args.spec;
    let binding_ty = args.binding;
    let spec_tail = path_tail_ident(&spec_ty)?.clone();
    let cases_method = args.cases;
    let module_ident = format_ident!(
        "__nirvash_code_tests_{}",
        spec_tail.to_string().to_lowercase()
    );
    let cases_expr = if let Some(cases_method) = cases_method {
        quote! { #cases_method() }
    } else {
        quote! { vec![<#spec_ty as ::core::default::Default>::default()] }
    };

    Ok(quote! {
        #[cfg(test)]
        mod #module_ident {
            use super::*;

            type GeneratedState = <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::State;
            type GeneratedAction = <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action;
            type GeneratedModelCase = ::nirvash_core::conformance::ModelCase<GeneratedState, GeneratedAction>;
            type GeneratedRuntime =
                <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::Runtime;
            type GeneratedContext =
                <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::Context;
            type GeneratedExpectedOutput =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ExpectedOutput;
            type GeneratedProbeState =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ProbeState;
            type GeneratedProbeOutput =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ProbeOutput;
            type GeneratedSummaryState =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::SummaryState;
            type GeneratedSummaryOutput =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::SummaryOutput;

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            fn generated_model_cases(spec: &#spec_ty) -> ::std::vec::Vec<GeneratedModelCase> {
                <#spec_ty as ::nirvash_core::conformance::ModelCaseSource>::model_cases(spec)
            }

            fn generated_paths(
                spec: &#spec_ty,
                model_case: GeneratedModelCase,
            ) -> (
                ::nirvash_core::conformance::ReachableGraphSnapshot<GeneratedState, GeneratedAction>,
                ::std::vec::Vec<::std::vec::Vec<GeneratedAction>>,
            ) {
                let snapshot = ::nirvash_core::conformance::ModelChecker::for_case(spec, model_case)
                    .full_reachable_graph_snapshot()
                    .expect("reachable graph snapshot should build");
                let mut paths = vec![::core::option::Option::None; snapshot.states.len()];
                let mut queue = ::std::collections::VecDeque::new();
                for &index in &snapshot.initial_indices {
                    paths[index] = ::core::option::Option::Some(::std::vec::Vec::new());
                    queue.push_back(index);
                }
                while let ::core::option::Option::Some(source) = queue.pop_front() {
                    let prefix = paths[source]
                        .clone()
                        .expect("reachable source should already have a path");
                    for edge in &snapshot.edges[source] {
                        if paths[edge.target].is_none() {
                            let mut next_path = prefix.clone();
                            next_path.push(edge.action.clone());
                            paths[edge.target] = ::core::option::Option::Some(next_path);
                            queue.push_back(edge.target);
                        }
                    }
                }
                (
                    snapshot,
                    paths.into_iter()
                        .map(|path| path.expect("reachable state should have canonical path"))
                        .collect(),
                )
            }

            async fn replay_prefix(
                spec: &#spec_ty,
                path: &[GeneratedAction],
                context: &GeneratedContext,
            ) -> GeneratedRuntime {
                let runtime =
                    <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::fresh_runtime(spec).await;
                let observed = <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                    &runtime,
                    context,
                )
                .await;
                let observed_summary =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed,
                    );
                let mut projected =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &observed_summary,
                    );
                ::nirvash_core::conformance::assert_initial_refinement(spec, &observed_summary);
                for action in path {
                    let expected_next =
                        <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::transition(
                            spec,
                            &projected,
                            action,
                        );
                    let expected_output =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::expected_output(
                            spec,
                            &projected,
                            action,
                            expected_next.as_ref(),
                        );
                    let output = <GeneratedRuntime as ::nirvash_core::conformance::ActionApplier>::execute_action(
                        &runtime,
                        context,
                        action,
                    )
                    .await;
                    let output_summary =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_output(
                            spec,
                            &output,
                        );
                    let projected_output =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_output(
                            spec,
                            &output_summary,
                        );
                    assert_eq!(projected_output, expected_output);
                    let observed_after =
                        <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                            &runtime,
                            context,
                        )
                        .await;
                    let observed_after_summary =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_state(
                            spec,
                            &observed_after,
                        );
                    let projected_after =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_state(
                            spec,
                            &observed_after_summary,
                        );
                    match expected_next {
                        ::core::option::Option::Some(next) => {
                            assert_eq!(projected_after, next);
                            projected = projected_after;
                        }
                        ::core::option::Option::None => {
                            assert_eq!(projected_after, projected);
                        }
                    }
                }
                runtime
            }

            async fn execute_from_state(
                spec: &#spec_ty,
                path: &[GeneratedAction],
                expected_state: &GeneratedState,
                action: &GeneratedAction,
                context: &GeneratedContext,
            ) -> (
                ::core::option::Option<GeneratedState>,
                GeneratedExpectedOutput,
                GeneratedState,
            ) {
                let runtime = replay_prefix(spec, path, context).await;
                let observed_before =
                    <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                        &runtime,
                        context,
                    )
                .await;
                let observed_before_summary =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed_before,
                    );
                let projected_before =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &observed_before_summary,
                    );
                assert_eq!(projected_before, *expected_state);
                let expected_next =
                    <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::transition(
                        spec,
                        &projected_before,
                        action,
                    );
                let expected_output =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::expected_output(
                        spec,
                        &projected_before,
                        action,
                        expected_next.as_ref(),
                    );
                let output = <GeneratedRuntime as ::nirvash_core::conformance::ActionApplier>::execute_action(
                    &runtime,
                    context,
                    action,
                )
                .await;
                let output_summary =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_output(
                        spec,
                        &output,
                    );
                let projected_output =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_output(
                        spec,
                        &output_summary,
                    );
                let observed_after =
                    <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                        &runtime,
                        context,
                    )
                    .await;
                let observed_after_summary =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed_after,
                    );
                let projected_after =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &observed_after_summary,
                    );
                assert_eq!(projected_output, expected_output);
                (expected_next, projected_output, projected_after)
            }

            #[test]
            fn generated_spec_is_deterministic_for_code_conformance() {
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
                        let (snapshot, _) = generated_paths(&spec, model_case);
                        for state in &snapshot.states {
                            for action in <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::actions(&spec) {
                                let next_states = <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::successors(
                                    &spec,
                                    state,
                                )
                                    .into_iter()
                                    .filter(|(candidate_action, _)| *candidate_action == action)
                                    .map(|(_, next)| next)
                                    .collect::<::std::vec::Vec<_>>();
                                assert!(
                                    next_states.len() <= 1,
                                    "spec is nondeterministic for state {:?} and action {:?}: {:?}",
                                    state,
                                    action,
                                    next_states
                                );
                                match <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::transition(
                                    &spec,
                                    state,
                                    &action,
                                ) {
                                    ::core::option::Option::Some(next) => {
                                        assert_eq!(next_states, vec![next]);
                                    }
                                    ::core::option::Option::None => {
                                        assert!(next_states.is_empty());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            #[tokio::test]
            async fn generated_real_code_accepts_allowed_actions() {
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
                        let context = <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::actions(&spec) {
                                let (expected_next, _, _) = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                        .await;
                                if expected_next.is_some() {
                                    // replay + dispatch already succeeded if we reached here
                                }
                            }
                        }
                    }
                }
            }

            #[tokio::test]
            async fn generated_real_code_rejects_disallowed_actions() {
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
                        let context = <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::actions(&spec) {
                                let (expected_next, _, observed_after) = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                        .await;
                                if expected_next.is_none() {
                                    assert_eq!(observed_after, *state);
                                }
                            }
                        }
                    }
                }
            }

            #[tokio::test]
            async fn generated_real_code_state_matches_spec() {
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
                        let context = <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::actions(&spec) {
                                let (expected_next, _, observed_after) = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                        .await;
                                match expected_next {
                                    ::core::option::Option::Some(next) => {
                                        assert_eq!(observed_after, next);
                                    }
                                    ::core::option::Option::None => {
                                        assert_eq!(observed_after, *state);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            #[tokio::test]
            async fn generated_real_code_output_matches_expected() {
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
                        let context = <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::actions(&spec) {
                                let (expected_next, output, _) = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                        .await;
                                let expected_output =
                                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::expected_output(
                                        &spec,
                                        state,
                                        &action,
                                        expected_next.as_ref(),
                                    );
                                assert_eq!(output, expected_output);
                            }
                        }
                    }
                }
            }
        }
    })
}

fn expand_code_witness_tests(args: CodeTestArgs) -> syn::Result<proc_macro2::TokenStream> {
    let spec_ty = args.spec;
    let binding_ty = args.binding;
    let spec_tail = path_tail_ident(&spec_ty)?.clone();
    let cases_method = args.cases;
    let module_ident = format_ident!(
        "__nirvash_code_witness_tests_{}",
        spec_tail.to_string().to_lowercase()
    );
    let provider_build_ident = format_ident!(
        "__nirvash_build_code_witness_tests_{}",
        spec_tail.to_string().to_lowercase()
    );
    let cases_expr = if let Some(cases_method) = cases_method {
        quote! { #cases_method() }
    } else {
        quote! { vec![<#spec_ty as ::core::default::Default>::default()] }
    };

    Ok(quote! {
        const _: fn() = crate::__nirvash_code_witness_main_marker;

        #[cfg(test)]
        mod #module_ident {
            use super::*;

            type GeneratedState = <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::State;
            type GeneratedAction = <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action;
            type GeneratedModelCase = ::nirvash_core::conformance::ModelCase<GeneratedState, GeneratedAction>;
            type GeneratedRuntime =
                <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::Runtime;
            type GeneratedContext =
                <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::Context;
            type GeneratedInput =
                <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::Input;
            type GeneratedSession =
                <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::Session;
            type GeneratedExpectedOutput =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ExpectedOutput;
            type GeneratedProbeState =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ProbeState;
            type GeneratedProbeOutput =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ProbeOutput;
            type GeneratedSummaryState =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::SummaryState;
            type GeneratedSummaryOutput =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::SummaryOutput;
            type GeneratedPositiveWitness =
                ::nirvash_core::conformance::PositiveWitness<GeneratedContext, GeneratedInput>;
            type GeneratedNegativeWitness =
                ::nirvash_core::conformance::NegativeWitness<GeneratedContext, GeneratedInput>;

            #[derive(Clone)]
            struct GeneratedSemanticCase {
                spec_case_label: ::std::string::String,
                prefix_id: usize,
                provenance: ::std::vec::Vec<::std::string::String>,
                state: GeneratedState,
                action: GeneratedAction,
                expected_next: ::core::option::Option<GeneratedState>,
                path: ::std::vec::Vec<GeneratedAction>,
            }

            #[derive(Clone)]
            struct GeneratedWitnessDescriptor {
                index: usize,
                name: ::std::string::String,
            }

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            fn generated_model_cases(spec: &#spec_ty) -> ::std::vec::Vec<GeneratedModelCase> {
                <#spec_ty as ::nirvash_core::conformance::ModelCaseSource>::model_cases(spec)
            }

            fn generated_paths(
                spec: &#spec_ty,
                model_case: GeneratedModelCase,
            ) -> (
                ::nirvash_core::conformance::ReachableGraphSnapshot<GeneratedState, GeneratedAction>,
                ::std::vec::Vec<::std::vec::Vec<GeneratedAction>>,
            ) {
                let snapshot = ::nirvash_core::conformance::ModelChecker::for_case(spec, model_case)
                    .full_reachable_graph_snapshot()
                    .expect("reachable graph snapshot should build");
                let mut paths = vec![::core::option::Option::None; snapshot.states.len()];
                let mut queue = ::std::collections::VecDeque::new();
                for &index in &snapshot.initial_indices {
                    paths[index] = ::core::option::Option::Some(::std::vec::Vec::new());
                    queue.push_back(index);
                }
                while let ::core::option::Option::Some(source) = queue.pop_front() {
                    let prefix = paths[source]
                        .clone()
                        .expect("reachable source should already have a path");
                    for edge in &snapshot.edges[source] {
                        if paths[edge.target].is_none() {
                            let mut next_path = prefix.clone();
                            next_path.push(edge.action.clone());
                            paths[edge.target] = ::core::option::Option::Some(next_path);
                            queue.push_back(edge.target);
                        }
                    }
                }
                (
                    snapshot,
                    paths.into_iter()
                        .map(|path| path.expect("reachable state should have canonical path"))
                        .collect(),
                )
            }

            fn generated_spec_case_label(index: usize, total: usize) -> ::std::string::String {
                if total > 1 {
                    format!("case-{index}")
                } else {
                    "default".to_owned()
                }
            }

            fn generated_sanitize_test_component(raw: &str) -> ::std::string::String {
                let mut sanitized = raw
                    .chars()
                    .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                    .collect::<::std::string::String>();
                while sanitized.contains("__") {
                    sanitized = sanitized.replace("__", "_");
                }
                sanitized = sanitized.trim_matches('_').to_owned();
                if sanitized.is_empty() {
                    "default".to_owned()
                } else {
                    sanitized
                }
            }

            fn generated_prefix_component(path: &[GeneratedAction]) -> ::std::string::String {
                if path.is_empty() {
                    return "from_init".to_owned();
                }
                let actions = path
                    .iter()
                    .map(|action| generated_sanitize_test_component(&format!("{action:?}")))
                    .collect::<::std::vec::Vec<_>>()
                    .join("__");
                format!("after_{actions}")
            }

            fn generated_action_component(action: &GeneratedAction) -> ::std::string::String {
                format!(
                    "when_{}",
                    generated_sanitize_test_component(&format!("{action:?}"))
                )
            }

            fn generated_prefix_id_component(prefix_id: usize) -> ::std::string::String {
                format!("via_{prefix_id:02}")
            }

            fn generated_test_name(
                semantic_case: &GeneratedSemanticCase,
                witness: &GeneratedWitnessDescriptor,
            ) -> ::std::string::String {
                let kind = if semantic_case.expected_next.is_some() {
                    "positive"
                } else {
                    "negative"
                };
                format!(
                    "code_witness/{}/{}/{}/{}/{}/{}-{}",
                    kind,
                    generated_sanitize_test_component(&semantic_case.spec_case_label),
                    generated_prefix_component(&semantic_case.path),
                    generated_action_component(&semantic_case.action),
                    generated_prefix_id_component(semantic_case.prefix_id),
                    generated_sanitize_test_component(&witness.name),
                    witness.index,
                )
            }

            fn generated_setup_failure_name(semantic_case: &GeneratedSemanticCase) -> ::std::string::String {
                let kind = if semantic_case.expected_next.is_some() {
                    "positive"
                } else {
                    "negative"
                };
                format!(
                    "code_witness/{}/{}/{}/{}/{}/setup",
                    kind,
                    generated_sanitize_test_component(&semantic_case.spec_case_label),
                    generated_prefix_component(&semantic_case.path),
                    generated_action_component(&semantic_case.action),
                    generated_prefix_id_component(semantic_case.prefix_id),
                )
            }

            fn generated_failure_prelude(
                semantic_case: &GeneratedSemanticCase,
                witness_name: &str,
            ) -> ::std::string::String {
                format!(
                    "spec case: {}\nsemantic action: {:?}\nwitness: {}\nprovenance: {:?}\ncanonical prefix path: {:?}\n",
                    semantic_case.spec_case_label,
                    semantic_case.action,
                    witness_name,
                    semantic_case.provenance,
                    semantic_case.path,
                )
            }

            fn generated_merge_provenance(
                provenance: &mut ::std::vec::Vec<::std::string::String>,
                label: ::std::string::String,
            ) {
                if !provenance.iter().any(|existing| existing == &label) {
                    provenance.push(label);
                }
            }

            fn generated_semantic_cases(
                spec_case_label: &str,
                spec: &#spec_ty,
            ) -> ::std::vec::Vec<GeneratedSemanticCase> {
                let mut cases: ::std::vec::Vec<GeneratedSemanticCase> = ::std::vec::Vec::new();
                let mut next_prefix_id = 0usize;
                for model_case in generated_model_cases(spec) {
                    let provenance_label = format!("{spec_case_label}/{}", model_case.label());
                    let (snapshot, paths) = generated_paths(spec, model_case);
                    for (index, state) in snapshot.states.iter().enumerate() {
                        for action in <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::actions(spec) {
                            let expected_next =
                                <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::transition(
                                    spec,
                                    state,
                                    &action,
                                );
                            if let ::core::option::Option::Some(existing) = cases.iter_mut().find(|existing| {
                                existing.state == *state
                                    && existing.action == action
                                    && existing.expected_next == expected_next
                                    && existing.path == paths[index]
                            }) {
                                generated_merge_provenance(
                                    &mut existing.provenance,
                                    provenance_label.clone(),
                                );
                                continue;
                            }
                            cases.push(GeneratedSemanticCase {
                                spec_case_label: spec_case_label.to_owned(),
                                prefix_id: next_prefix_id,
                                provenance: vec![provenance_label.clone()],
                                state: state.clone(),
                                action: action.clone(),
                                expected_next,
                                path: paths[index].clone(),
                            });
                            next_prefix_id += 1;
                        }
                    }
                }
                cases
            }

            fn generated_runtime() -> ::tokio::runtime::Runtime {
                ::tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("code witness tokio runtime should build")
            }

            async fn generated_observe_projected_state(
                spec: &#spec_ty,
                runtime: &GeneratedRuntime,
                session: &GeneratedSession,
            ) -> GeneratedState {
                let context =
                    <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(session);
                let observed = <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                    runtime,
                    &context,
                )
                .await;
                let observed_summary =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed,
                    );
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_state(
                    spec,
                    &observed_summary,
                )
            }

            fn generated_select_canonical_witness(
                semantic_case: &GeneratedSemanticCase,
                prev: &GeneratedState,
                action: &GeneratedAction,
                next: &GeneratedState,
                witnesses: &[GeneratedPositiveWitness],
            ) -> ::std::result::Result<GeneratedPositiveWitness, ::std::string::String> {
                let canonical = witnesses
                    .iter()
                    .filter(|witness| witness.canonical())
                    .cloned()
                    .collect::<::std::vec::Vec<_>>();
                if canonical.len() == 1 {
                    return Ok(canonical[0].clone());
                }
                Err(format!(
                    "{}expected canonical witness count = 1 for {:?} -- {:?} --> {:?}, found {} from {:?}",
                    generated_failure_prelude(semantic_case, "<canonical-prefix>"),
                    prev,
                    action,
                    next,
                    canonical.len(),
                    witnesses
                        .iter()
                        .map(|witness| format!("{}(canonical={})", witness.name(), witness.canonical()))
                        .collect::<::std::vec::Vec<_>>(),
                ))
            }

            async fn generated_replay_canonical_prefix(
                spec: &#spec_ty,
                semantic_case: &GeneratedSemanticCase,
                runtime: &GeneratedRuntime,
                session: &mut GeneratedSession,
            ) -> ::std::result::Result<(), ::std::string::String> {
                let initial_context =
                    <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(session);
                let initial_probe =
                    <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                        runtime,
                        &initial_context,
                    )
                    .await;
                let initial_summary =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &initial_probe,
                    );
                let initial_projected =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &initial_summary,
                    );
                let initial_refinement = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                    ::nirvash_core::conformance::assert_initial_refinement(spec, &initial_summary);
                }));
                if let Err(payload) = initial_refinement {
                    return Err(format!(
                        "{}{}",
                        generated_failure_prelude(semantic_case, "<initial-state>"),
                        ::nirvash_core::conformance::panic_payload_to_string(payload),
                    ));
                }
                let mut projected = initial_projected;
                for action in &semantic_case.path {
                    let expected_next =
                        <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::transition(
                            spec,
                            &projected,
                            action,
                        )
                        .ok_or_else(|| {
                            format!(
                                "{}canonical prefix action {:?} is not allowed from {:?}",
                                generated_failure_prelude(semantic_case, "<canonical-prefix>"),
                                action,
                                projected,
                            )
                        })?;
                    let witnesses =
                        <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::positive_witnesses(
                            spec,
                            session,
                            &projected,
                            action,
                            &expected_next,
                        );
                    if witnesses.is_empty() {
                        return Err(format!(
                            "{}canonical prefix for {:?} -- {:?} --> {:?} has no positive witnesses",
                            generated_failure_prelude(semantic_case, "<canonical-prefix>"),
                            projected,
                            action,
                            expected_next,
                        ));
                    }
                    let witness = generated_select_canonical_witness(
                        semantic_case,
                        &projected,
                        action,
                        &expected_next,
                        &witnesses,
                    )?;
                    let output =
                        <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::execute_input(
                            runtime,
                            session,
                            witness.context(),
                            witness.input(),
                        )
                        .await;
                    let output_summary =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_output(
                            spec,
                            &output,
                        );
                    let projected_output =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_output(
                            spec,
                            &output_summary,
                        );
                    let expected_output =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::expected_output(
                            spec,
                            &projected,
                            action,
                            ::core::option::Option::Some(&expected_next),
                        );
                    if projected_output != expected_output {
                        return Err(format!(
                            "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                            generated_failure_prelude(semantic_case, witness.name()),
                            expected_output,
                            projected_output,
                            expected_next,
                            projected,
                        ));
                    }
                    let observed_after = generated_observe_projected_state(spec, runtime, session).await;
                    if observed_after != expected_next {
                        return Err(format!(
                            "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                            generated_failure_prelude(semantic_case, witness.name()),
                            expected_output,
                            projected_output,
                            expected_next,
                            observed_after,
                        ));
                    }
                    projected = expected_next;
                }
                Ok(())
            }

            fn generated_case_witnesses(
                spec: &#spec_ty,
                semantic_case: &GeneratedSemanticCase,
            ) -> ::std::result::Result<::std::vec::Vec<GeneratedWitnessDescriptor>, ::std::string::String> {
                generated_runtime().block_on(async {
                    let runtime =
                        <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::fresh_runtime(spec).await;
                    let mut session =
                        <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::fresh_session(spec).await;
                    generated_replay_canonical_prefix(spec, semantic_case, &runtime, &mut session).await?;
                    let observed_before = generated_observe_projected_state(spec, &runtime, &session).await;
                    if observed_before != semantic_case.state {
                        return Err(format!(
                            "{}expected output: <not-executed>\nobserved output: <not-executed>\nexpected state: {:?}\nobserved state: {:?}",
                            generated_failure_prelude(semantic_case, "<probe-before-target>"),
                            semantic_case.state,
                            observed_before,
                        ));
                    }
                    if let ::core::option::Option::Some(next) = semantic_case.expected_next.as_ref() {
                        let witnesses =
                            <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::positive_witnesses(
                                spec,
                                &session,
                                &semantic_case.state,
                                &semantic_case.action,
                                next,
                            );
                        if witnesses.is_empty() {
                            return Err(format!(
                                "{}positive witnesses are empty",
                                generated_failure_prelude(semantic_case, "<metadata>"),
                            ));
                        }
                        let canonical_count =
                            witnesses.iter().filter(|witness| witness.canonical()).count();
                        if canonical_count != 1 {
                            return Err(format!(
                                "{}expected canonical witness count = 1, found {} from {:?}",
                                generated_failure_prelude(semantic_case, "<metadata>"),
                                canonical_count,
                                witnesses
                                    .iter()
                                    .map(|witness| format!("{}(canonical={})", witness.name(), witness.canonical()))
                                    .collect::<::std::vec::Vec<_>>(),
                            ));
                        }
                        Ok(witnesses
                            .into_iter()
                            .enumerate()
                            .map(|(index, witness)| GeneratedWitnessDescriptor {
                                index,
                                name: witness.name().to_owned(),
                            })
                            .collect())
                    } else {
                        let witnesses =
                            <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::negative_witnesses(
                                spec,
                                &session,
                                &semantic_case.state,
                                &semantic_case.action,
                            );
                        if witnesses.is_empty() {
                            return Err(format!(
                                "{}negative witnesses are empty",
                                generated_failure_prelude(semantic_case, "<metadata>"),
                            ));
                        }
                        Ok(witnesses
                            .into_iter()
                            .enumerate()
                            .map(|(index, witness)| GeneratedWitnessDescriptor {
                                index,
                                name: witness.name().to_owned(),
                            })
                            .collect())
                    }
                })
            }

            fn generated_run_case(
                spec: ::std::rc::Rc<#spec_ty>,
                semantic_case: GeneratedSemanticCase,
                witness_index: usize,
            ) -> ::std::result::Result<(), ::std::string::String> {
                generated_runtime().block_on(async move {
                    let runtime =
                        <#binding_ty as ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty>>::fresh_runtime(spec.as_ref()).await;
                    let mut session =
                        <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::fresh_session(spec.as_ref()).await;
                    generated_replay_canonical_prefix(spec.as_ref(), &semantic_case, &runtime, &mut session).await?;
                    let observed_before =
                        generated_observe_projected_state(spec.as_ref(), &runtime, &session).await;
                    if observed_before != semantic_case.state {
                        return Err(format!(
                            "{}expected output: <not-executed>\nobserved output: <not-executed>\nexpected state: {:?}\nobserved state: {:?}",
                            generated_failure_prelude(&semantic_case, "<probe-before-target>"),
                            semantic_case.state,
                            observed_before,
                        ));
                    }
                    match semantic_case.expected_next.as_ref() {
                        ::core::option::Option::Some(next) => {
                            let witnesses =
                                <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::positive_witnesses(
                                    spec.as_ref(),
                                    &session,
                                    &semantic_case.state,
                                    &semantic_case.action,
                                    next,
                                );
                            if witnesses.is_empty() {
                                return Err(format!(
                                    "{}positive witnesses are empty",
                                    generated_failure_prelude(&semantic_case, "<run-positive>"),
                                ));
                            }
                            let witness = witnesses.get(witness_index).ok_or_else(|| {
                                format!(
                                    "{}witness index {} is out of bounds for {:?}",
                                    generated_failure_prelude(&semantic_case, "<run-positive>"),
                                    witness_index,
                                    witnesses
                                        .iter()
                                        .map(|witness| witness.name().to_owned())
                                        .collect::<::std::vec::Vec<_>>(),
                                )
                            })?;
                            let output =
                                <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::execute_input(
                                    &runtime,
                                    &mut session,
                                    witness.context(),
                                    witness.input(),
                                )
                                .await;
                            let output_summary =
                                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_output(
                                    spec.as_ref(),
                                    &output,
                                );
                            let projected_output =
                                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_output(
                                    spec.as_ref(),
                                    &output_summary,
                                );
                            let expected_output =
                                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::expected_output(
                                    spec.as_ref(),
                                    &semantic_case.state,
                                    &semantic_case.action,
                                    ::core::option::Option::Some(next),
                                );
                            let observed_after =
                                generated_observe_projected_state(spec.as_ref(), &runtime, &session).await;
                            if projected_output != expected_output {
                                return Err(format!(
                                    "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                                    generated_failure_prelude(&semantic_case, witness.name()),
                                    expected_output,
                                    projected_output,
                                    next,
                                    observed_after,
                                ));
                            }
                            if observed_after != *next {
                                return Err(format!(
                                    "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                                    generated_failure_prelude(&semantic_case, witness.name()),
                                    expected_output,
                                    projected_output,
                                    next,
                                    observed_after,
                                ));
                            }
                            Ok(())
                        }
                        ::core::option::Option::None => {
                            let witnesses =
                                <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::negative_witnesses(
                                    spec.as_ref(),
                                    &session,
                                    &semantic_case.state,
                                    &semantic_case.action,
                                );
                            if witnesses.is_empty() {
                                return Err(format!(
                                    "{}negative witnesses are empty",
                                    generated_failure_prelude(&semantic_case, "<run-negative>"),
                                ));
                            }
                            let witness = witnesses.get(witness_index).ok_or_else(|| {
                                format!(
                                    "{}witness index {} is out of bounds for {:?}",
                                    generated_failure_prelude(&semantic_case, "<run-negative>"),
                                    witness_index,
                                    witnesses
                                        .iter()
                                        .map(|witness| witness.name().to_owned())
                                        .collect::<::std::vec::Vec<_>>(),
                                )
                            })?;
                            let output =
                                <#binding_ty as ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty>>::execute_input(
                                    &runtime,
                                    &mut session,
                                    witness.context(),
                                    witness.input(),
                                )
                                .await;
                            let output_summary =
                                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::summarize_output(
                                    spec.as_ref(),
                                    &output,
                                );
                            let projected_output =
                                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_output(
                                    spec.as_ref(),
                                    &output_summary,
                                );
                            let expected_output =
                                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::expected_output(
                                    spec.as_ref(),
                                    &semantic_case.state,
                                    &semantic_case.action,
                                    ::core::option::Option::None,
                                );
                            let observed_after =
                                generated_observe_projected_state(spec.as_ref(), &runtime, &session).await;
                            if projected_output != expected_output {
                                return Err(format!(
                                    "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                                    generated_failure_prelude(&semantic_case, witness.name()),
                                    expected_output,
                                    projected_output,
                                    semantic_case.state,
                                    observed_after,
                                ));
                            }
                            if observed_after != semantic_case.state {
                                return Err(format!(
                                    "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                                    generated_failure_prelude(&semantic_case, witness.name()),
                                    expected_output,
                                    projected_output,
                                    semantic_case.state,
                                    observed_after,
                                ));
                            }
                            Ok(())
                        }
                    }
                })
            }

            pub(super) fn generated_dynamic_tests() -> ::std::vec::Vec<::nirvash_core::conformance::DynamicTestCase> {
                let specs = generated_cases();
                let total = specs.len();
                let mut tests: ::std::vec::Vec<::nirvash_core::conformance::DynamicTestCase> =
                    ::std::vec::Vec::new();
                for (index, spec) in specs.into_iter().enumerate() {
                    let spec_case_label = generated_spec_case_label(index, total);
                    let spec = ::std::rc::Rc::new(spec);
                    for semantic_case in generated_semantic_cases(&spec_case_label, spec.as_ref()) {
                        match generated_case_witnesses(spec.as_ref(), &semantic_case) {
                            Ok(witnesses) => {
                                for witness in witnesses {
                                    let spec = spec.clone();
                                    let semantic_case = semantic_case.clone();
                                    let name = generated_test_name(&semantic_case, &witness);
                                    tests.push(::nirvash_core::conformance::DynamicTestCase::new(
                                        name,
                                        move || {
                                            generated_run_case(
                                                spec.clone(),
                                                semantic_case.clone(),
                                                witness.index,
                                            )
                                        },
                                    ));
                                }
                            }
                            Err(message) => {
                                let name = generated_setup_failure_name(&semantic_case);
                                tests.push(::nirvash_core::conformance::DynamicTestCase::new(
                                    name,
                                    move || Err(message.clone()),
                                ));
                            }
                        }
                    }
                }
                tests
            }
        }

        #[cfg(test)]
        #[doc(hidden)]
        fn #provider_build_ident() -> ::std::vec::Vec<::nirvash_core::conformance::DynamicTestCase> {
            #module_ident::generated_dynamic_tests()
        }

        #[cfg(test)]
        ::nirvash_core::inventory::submit! {
            ::nirvash_core::conformance::RegisteredCodeWitnessTestProvider {
                build: #provider_build_ident,
            }
        }
    })
}

fn expand_runtime_contract(
    args: RuntimeContractArgs,
    item: ItemImpl,
) -> syn::Result<proc_macro2::TokenStream> {
    if item.generics.params.iter().next().is_some() {
        return Err(syn::Error::new(
            item.generics.span(),
            "nirvash_runtime_contract does not support generic impl blocks",
        ));
    }

    let spec_ty = args.spec.clone();
    let binding_ty = args.binding.clone();
    let grouped_tokens = if args.tests.grouped {
        Some(expand_code_tests(CodeTestArgs {
            spec: args.spec.clone(),
            binding: args.binding.clone(),
            cases: None,
        })?)
    } else {
        None
    };
    let witness_tokens = if args.tests.witness {
        Some(expand_code_witness_tests(CodeTestArgs {
            spec: args.spec.clone(),
            binding: args.binding.clone(),
            cases: None,
        })?)
    } else {
        None
    };

    if let Some(runtime_ty) = args.runtime_ty.clone() {
        return expand_runtime_contract_binding_mode(
            args,
            item,
            spec_ty,
            binding_ty,
            runtime_ty,
            grouped_tokens,
            witness_tokens,
        );
    }

    expand_runtime_contract_runtime_mode(
        args,
        item,
        spec_ty,
        binding_ty,
        grouped_tokens,
        witness_tokens,
    )
}

fn expand_projection_contract(
    args: ProjectionContractArgs,
    mut item: ItemImpl,
) -> syn::Result<TokenStream2> {
    if item.generics.params.iter().next().is_some() {
        return Err(syn::Error::new(
            item.generics.span(),
            "nirvash_projection_contract does not support generic impl blocks",
        ));
    }

    item.items.retain(|impl_item| match impl_item {
        ImplItem::Type(ty) => !matches!(
            ty.ident.to_string().as_str(),
            "ProbeState" | "ProbeOutput" | "SummaryState" | "SummaryOutput"
        ),
        ImplItem::Fn(method) => !matches!(
            method.sig.ident.to_string().as_str(),
            "summarize_state" | "summarize_output" | "abstract_state" | "abstract_output"
        ),
        _ => true,
    });

    let probe_state_ty = args.probe_state_ty;
    let probe_output_ty = args.probe_output_ty;
    let summary_state_ty = args.summary_state_ty;
    let summary_output_ty = args.summary_output_ty;
    let summarize_state = args.summarize_state;
    let summarize_output = args.summarize_output;
    let abstract_state = args.abstract_state;
    let abstract_output = args.abstract_output;

    item.items.push(syn::parse_quote! {
        type ProbeState = #probe_state_ty;
    });
    item.items.push(syn::parse_quote! {
        type ProbeOutput = #probe_output_ty;
    });
    item.items.push(syn::parse_quote! {
        type SummaryState = #summary_state_ty;
    });
    item.items.push(syn::parse_quote! {
        type SummaryOutput = #summary_output_ty;
    });
    item.items.push(syn::parse_quote! {
        fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
            (#summarize_state)(probe)
        }
    });
    item.items.push(syn::parse_quote! {
        fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
            (#summarize_output)(probe)
        }
    });
    item.items.push(syn::parse_quote! {
        fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
            (#abstract_state)(self, summary)
        }
    });
    item.items.push(syn::parse_quote! {
        fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
            (#abstract_output)(self, summary)
        }
    });

    Ok(quote! { #item })
}

fn expand_runtime_contract_binding_mode(
    args: RuntimeContractArgs,
    item: ItemImpl,
    spec_ty: Path,
    binding_ty: Path,
    runtime_ty: Type,
    grouped_tokens: Option<TokenStream2>,
    witness_tokens: Option<TokenStream2>,
) -> syn::Result<TokenStream2> {
    let context_ty = args.context_ty;
    let context_expr = args
        .context_expr
        .unwrap_or_else(|| syn::parse_quote!(::core::default::Default::default()));
    let fresh_runtime = args.fresh_runtime;
    let input_ty: Type = args.input_ty.unwrap_or_else(
        || syn::parse_quote!(<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action),
    );
    let session_ty: Type = args.session_ty.unwrap_or_else(|| context_ty.clone());
    let fresh_session = args.fresh_session.unwrap_or_else(|| context_expr.clone());
    let probe_context = args
        .probe_context
        .unwrap_or_else(|| syn::parse_quote!(session.clone()));
    let self_ty = item.self_ty.as_ref().clone();
    let self_binding_ident = match &self_ty {
        Type::Path(type_path) if type_path.qself.is_none() => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.clone()),
        _ => None,
    };
    let binding_ident = path_tail_ident(&binding_ty)?.clone();
    if self_binding_ident.as_ref() != Some(&binding_ident) {
        return Err(syn::Error::new(
            item.self_ty.span(),
            format!(
                "binding-mode nirvash_runtime_contract must be attached to impl {}",
                binding_ident
            ),
        ));
    }

    let witness_impl = if witness_tokens.is_some() {
        quote! {
            impl ::nirvash_core::conformance::ProtocolInputWitnessBinding<#spec_ty> for #self_ty {
                type Input = #input_ty;
                type Session = #session_ty;

                async fn fresh_session(_spec: &#spec_ty) -> Self::Session {
                    #fresh_session
                }

                fn positive_witnesses(
                    _spec: &#spec_ty,
                    session: &Self::Session,
                    _prev: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::State,
                    action: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action,
                    _next: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::State,
                ) -> ::std::vec::Vec<::nirvash_core::conformance::PositiveWitness<Self::Context, Self::Input>> {
                    vec![
                        ::nirvash_core::conformance::PositiveWitness::new(
                            "principal",
                            session.clone(),
                            action.clone(),
                        ).with_canonical(true)
                    ]
                }

                fn negative_witnesses(
                    _spec: &#spec_ty,
                    session: &Self::Session,
                    _prev: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::State,
                    action: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action,
                ) -> ::std::vec::Vec<::nirvash_core::conformance::NegativeWitness<Self::Context, Self::Input>> {
                    vec![
                        ::nirvash_core::conformance::NegativeWitness::new(
                            "principal",
                            session.clone(),
                            action.clone(),
                        )
                    ]
                }

                async fn execute_input(
                    runtime: &Self::Runtime,
                    _session: &mut Self::Session,
                    context: &Self::Context,
                    input: &Self::Input,
                ) -> <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::SummaryOutput {
                    <Self::Runtime as ::nirvash_core::conformance::ActionApplier>::execute_action(
                        runtime,
                        context,
                        input,
                    )
                    .await
                }

                fn probe_context(session: &Self::Session) -> Self::Context {
                    #probe_context
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #item

        impl ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty> for #self_ty {
            type Runtime = #runtime_ty;
            type Context = #context_ty;

            async fn fresh_runtime(_spec: &#spec_ty) -> Self::Runtime {
                #fresh_runtime
            }

            fn context(_spec: &#spec_ty) -> Self::Context {
                #context_expr
            }
        }

        #witness_impl

        #grouped_tokens
        #witness_tokens
    })
}

fn expand_runtime_contract_runtime_mode(
    args: RuntimeContractArgs,
    mut item: ItemImpl,
    spec_ty: Path,
    binding_ty: Path,
    grouped_tokens: Option<TokenStream2>,
    witness_tokens: Option<TokenStream2>,
) -> syn::Result<TokenStream2> {
    if witness_tokens.is_some() {
        return Err(syn::Error::new(
            item.self_ty.span(),
            "witness generation on runtime-mode contracts is not supported yet; use binding mode for identity witnesses",
        ));
    }

    let summary_ty = args.summary_ty.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires summary = ...",
        )
    })?;
    let output_ty = args.output_ty.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires output = ...",
        )
    })?;
    let summary_field = args.summary_field.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires summary_field = ...",
        )
    })?;
    let initial_summary = args.initial_summary.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires initial_summary = ...",
        )
    })?;
    let context_ty = args.context_ty;
    let context_expr = args
        .context_expr
        .unwrap_or_else(|| syn::parse_quote!(::core::default::Default::default()));
    let fresh_runtime = args.fresh_runtime;
    let self_ty = item.self_ty.as_ref().clone();
    let binding_ident = path_tail_ident(&binding_ty)?.clone();
    let self_path_ident = match &self_ty {
        Type::Path(type_path) if type_path.qself.is_none() => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.clone()),
        _ => None,
    };
    let generate_binding_struct = self_path_ident
        .as_ref()
        .is_none_or(|ident| *ident != binding_ident);

    let mut cases = Vec::<(Ident, ContractCaseArgs, bool)>::new();
    let mut seen_actions = BTreeSet::new();
    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let mut kept_attrs = Vec::new();
        let mut contract_case = None;
        for attr in method.attrs.drain(..) {
            if attr
                .path()
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "contract_case")
            {
                let parsed = attr.parse_args::<ContractCaseArgs>()?;
                let action_key = parsed.action.to_token_stream().to_string();
                if !seen_actions.insert(action_key) {
                    return Err(syn::Error::new(
                        attr.span(),
                        "duplicate contract_case action",
                    ));
                }
                contract_case = Some(parsed);
            } else {
                kept_attrs.push(attr);
            }
        }
        method.attrs = kept_attrs;
        if let Some(contract_case) = contract_case {
            if method.sig.receiver().is_none() || method.sig.inputs.len() != 1 {
                return Err(syn::Error::new(
                    method.sig.span(),
                    "contract_case methods must take only &self or &mut self",
                ));
            }
            let returns_result = match &method.sig.output {
                syn::ReturnType::Type(_, ty) => match ty.as_ref() {
                    Type::Path(type_path) if type_path.qself.is_none() => type_path
                        .path
                        .segments
                        .last()
                        .is_some_and(|segment| segment.ident == "Result"),
                    _ => false,
                },
                syn::ReturnType::Default => false,
            };
            cases.push((method.sig.ident.clone(), contract_case, returns_result));
        }
    }

    if cases.is_empty() {
        return Err(syn::Error::new(
            item.self_ty.span(),
            "runtime-mode contract requires at least one #[contract_case(...)] method",
        ));
    }

    let execute_branches = cases.iter().map(|(method_ident, case, returns_result)| {
        let action = &case.action;
        let requires = &case.requires;
        let update_bindings = case
            .updates
            .iter()
            .enumerate()
            .map(|(index, (field, expr))| {
                let value_ident = format_ident!("__nirvash_update_value_{index}");
                quote! {
                    let #value_ident = {
                        let summary = &prev_summary;
                        #expr
                    };
                    summary.#field = #value_ident;
                }
            });
        let output_expr = if let Some(output) = &case.output {
            quote! { (#output)(&result) }
        } else {
            quote! { <#output_ty as ::core::default::Default>::default() }
        };
        let success_update = if *returns_result {
            let message = LitStr::new(
                &format!("{method_ident} should succeed for allowed contract action"),
                method_ident.span(),
            );
            quote! {
                if result.is_ok() {
                    let mut summary_guard = self.#summary_field.lock().await;
                    let summary = &mut *summary_guard;
                    let prev_summary = (*summary).clone();
                    #(#update_bindings)*
                }
                result.expect(#message);
            }
        } else {
            quote! {
                {
                    let mut summary_guard = self.#summary_field.lock().await;
                    let summary = &mut *summary_guard;
                    let prev_summary = (*summary).clone();
                    #(#update_bindings)*
                }
            }
        };
        quote! {
            if *action == #action {
                let current_summary = *self.#summary_field.lock().await;
                let summary = &current_summary;
                if !(#requires) {
                    return <#output_ty as ::core::default::Default>::default();
                }
                let result = self.#method_ident().await;
                let output = #output_expr;
                #success_update
                return output;
            }
        }
    });

    let law_allowed_branches = cases.iter().map(|(_, case, _)| {
        let action = &case.action;
        let requires = &case.requires;
        quote! {
            if *action == #action {
                let summary = summary;
                return #requires;
            }
        }
    });

    let law_advance_branches = cases.iter().map(|(_, case, _)| {
        let action = &case.action;
        let updates = case
            .updates
            .iter()
            .enumerate()
            .map(|(index, (field, expr))| {
                let value_ident = format_ident!("__nirvash_update_value_{index}");
                quote! {
                    let #value_ident = {
                        let summary = &prev_summary;
                        #expr
                    };
                    next.#field = #value_ident;
                }
            });
        quote! {
            if *action == #action {
                let prev_summary = summary.clone();
                let mut next = prev_summary.clone();
                #(#updates)*
                return next;
            }
        }
    });

    let law_output_branches = cases.iter().map(|(_, case, _)| {
        let action = &case.action;
        let law_output = case.law_output.clone().unwrap_or_else(
            || syn::parse_quote!(<#output_ty as ::core::default::Default>::default()),
        );
        quote! {
            if *action == #action {
                return #law_output;
            }
        }
    });

    let binding_struct = if generate_binding_struct {
        quote! {
            #[derive(Debug, Default, Clone, Copy)]
            struct #binding_ident;
        }
    } else {
        quote! {}
    };

    let law_test_ident = format_ident!(
        "__nirvash_summary_law_{}",
        binding_ident.to_string().to_lowercase()
    );
    let law_module_ident = format_ident!(
        "__nirvash_runtime_contract_{}",
        binding_ident.to_string().to_lowercase()
    );

    Ok(quote! {
        #item

        #binding_struct

        impl ::nirvash_core::conformance::ProtocolRuntimeBinding<#spec_ty> for #binding_ty {
            type Runtime = #self_ty;
            type Context = #context_ty;

            async fn fresh_runtime(spec: &#spec_ty) -> Self::Runtime {
                #fresh_runtime
            }

            fn context(_spec: &#spec_ty) -> Self::Context {
                #context_expr
            }
        }

        impl ::nirvash_core::conformance::ActionApplier for #self_ty {
            type Action = <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action;
            type Output = #output_ty;
            type Context = #context_ty;

            async fn execute_action(
                &self,
                _context: &Self::Context,
                action: &Self::Action,
            ) -> Self::Output {
                #(#execute_branches)*
                panic!("no contract_case registered for action {:?}", action);
            }
        }

        impl ::nirvash_core::conformance::StateObserver for #self_ty {
            type SummaryState = #summary_ty;
            type Context = #context_ty;

            async fn observe_state(&self, _context: &Self::Context) -> Self::SummaryState {
                *self.#summary_field.lock().await
            }
        }

        #[cfg(test)]
        mod #law_module_ident {
            use super::*;

            fn generated_initial_summary() -> #summary_ty {
                #initial_summary
            }

            fn generated_action_allowed(
                summary: &#summary_ty,
                action: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action,
            ) -> bool {
                #(#law_allowed_branches)*
                false
            }

            fn generated_advance_summary(
                summary: &#summary_ty,
                action: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action,
            ) -> #summary_ty {
                #(#law_advance_branches)*
                *summary
            }

            fn generated_law_output(
                action: &<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action,
            ) -> #output_ty {
                #(#law_output_branches)*
                <#output_ty as ::core::default::Default>::default()
            }

            #[test]
            fn #law_test_ident() {
                let spec = <#spec_ty as ::core::default::Default>::default();
                let actions =
                    <<#spec_ty as ::nirvash_core::conformance::TransitionSystem>::Action as ::nirvash_core::ActionVocabulary>::action_vocabulary();
                let initial_summary = generated_initial_summary();
                ::nirvash_core::conformance::assert_initial_refinement(&spec, &initial_summary);
                let mut pending = vec![initial_summary];
                let mut visited = Vec::new();

                while let Some(summary) = pending.pop() {
                    if visited.contains(&summary) {
                        continue;
                    }
                    visited.push(summary);

                    for action in &actions {
                        let allowed_by_summary = generated_action_allowed(&summary, action);
                        let prev =
                            <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::abstract_state(
                                &spec,
                                &summary,
                            );
                        let expected_next =
                            <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::transition(
                                &spec,
                                &prev,
                                action,
                            );
                        assert_eq!(
                            allowed_by_summary,
                            expected_next.is_some(),
                            "summary/state enabled mismatch for {action:?} from {summary:?}",
                        );
                        if expected_next.is_some() {
                            let next_summary = generated_advance_summary(&summary, action);
                            ::nirvash_core::conformance::assert_step_refinement(
                                &spec,
                                &summary,
                                action,
                                &next_summary,
                            );
                            ::nirvash_core::conformance::assert_output_refinement(
                                &spec,
                                &summary,
                                action,
                                &next_summary,
                                &generated_law_output(action),
                            );
                            if !visited.contains(&next_summary) {
                                pending.push(next_summary);
                            }
                        }
                    }
                }
            }
        }

        #grouped_tokens
    })
}

fn path_tail_ident(path: &Path) -> syn::Result<&Ident> {
    path.segments
        .last()
        .map(|segment| &segment.ident)
        .ok_or_else(|| syn::Error::new(path.span(), "path cannot be empty"))
}

fn doc_fragment_attrs(self_ty: &Type) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    let Some(env_key) = doc_fragment_env_key(self_ty)? else {
        return Ok(Vec::new());
    };
    let Ok(path) = ::std::env::var(&env_key) else {
        return Ok(Vec::new());
    };
    ::std::fs::metadata(&path).map_err(|error| {
        syn::Error::new(
            self_ty.span(),
            format!("failed to read nirvash doc fragment `{path}`: {error}"),
        )
    })?;
    let path = LitStr::new(&path, Span::call_site());
    Ok(vec![quote! { #[doc = include_str!(#path)] }])
}

fn doc_fragment_env_key(self_ty: &Type) -> syn::Result<Option<String>> {
    let Type::Path(type_path) = self_ty else {
        return Ok(None);
    };
    if type_path.qself.is_some() {
        return Ok(None);
    }
    let Some(segment) = type_path.path.segments.last() else {
        return Ok(None);
    };
    Ok(Some(format!(
        "NIRVASH_DOC_FRAGMENT_{}",
        to_upper_snake(&segment.ident.to_string())
    )))
}

fn to_upper_snake(input: &str) -> String {
    let mut output = String::new();
    let mut previous_is_lower = false;
    for character in input.chars() {
        if character.is_ascii_uppercase() {
            if previous_is_lower && !output.ends_with('_') {
                output.push('_');
            }
            output.push(character);
            previous_is_lower = false;
        } else if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_uppercase());
            previous_is_lower = true;
        } else {
            if !output.ends_with('_') && !output.is_empty() {
                output.push('_');
            }
            previous_is_lower = false;
        }
    }
    output
}
