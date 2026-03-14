use std::collections::{BTreeMap, BTreeSet};

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    Attribute, BinOp, Data, DataEnum, DataStruct, DeriveInput, Expr, ExprBinary, ExprCall,
    ExprField, ExprIf, ExprLit, ExprMacro, ExprMethodCall, ExprParen, ExprPath, ExprRange,
    ExprReference, ExprStruct, ExprUnary, Field, Fields, Ident, ImplItem, ImplItemFn, ItemConst,
    ItemFn, ItemImpl, Lit, LitStr, Member, Pat, Path, PathArguments, RangeLimits, Stmt, Token,
    Type, UnOp, parse_macro_input,
};

#[proc_macro_derive(
    FiniteModelDomain,
    attributes(
        finite_model_domain,
        finite_model,
        finite_model_domain_invariant,
        signature,
        sig,
        signature_invariant,
        viz
    )
)]
pub fn derive_finite_model_domain(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_finite_model_domain_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(SymbolicEncoding, attributes(symbolic_encoding))]
pub fn derive_symbolic_encoding(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_symbolic_encoding_derive(input) {
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

#[proc_macro_derive(ProtocolInputWitness, attributes(protocol_input_witness))]
pub fn derive_protocol_input_witness(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_protocol_input_witness_derive(input) {
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

#[proc_macro]
pub fn nirvash_projection_model(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as ProjectionModelArgs);
    match expand_projection_model(args) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro]
pub fn nirvash_expr(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as NamedStateExprInput);
    match expand_nirvash_expr(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro]
pub fn nirvash_step_expr(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as NamedStepExprInput);
    match expand_nirvash_step_expr(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro]
pub fn nirvash_transition_program(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TransitionProgramDsl);
    match expand_nirvash_transition_program(input) {
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
            ::nirvash_conformance::run_registered_code_witness_tests();
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

mod nirvash_dsl_kw {
    syn::custom_keyword!(rule);
    syn::custom_keyword!(when);
    syn::custom_keyword!(set);
    syn::custom_keyword!(insert);
    syn::custom_keyword!(remove);
}

struct NamedStateExprInput {
    name: Ident,
    state: Ident,
    expr: Expr,
}

impl Parse for NamedStateExprInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let content;
        syn::parenthesized!(content in input);
        let state: Ident = content.parse()?;
        if !content.is_empty() {
            return Err(content.error("expected a single state binding identifier"));
        }
        input.parse::<Token![=>]>()?;
        let expr: Expr = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after nirvash_expr! body"));
        }
        Ok(Self { name, state, expr })
    }
}

struct NamedStepExprInput {
    name: Ident,
    prev: Ident,
    action: Ident,
    next: Ident,
    expr: Expr,
}

impl Parse for NamedStepExprInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let content;
        syn::parenthesized!(content in input);
        let prev: Ident = content.parse()?;
        content.parse::<Token![,]>()?;
        let action: Ident = content.parse()?;
        content.parse::<Token![,]>()?;
        let next: Ident = content.parse()?;
        if !content.is_empty() {
            return Err(content.error("expected exactly three step binding identifiers"));
        }
        input.parse::<Token![=>]>()?;
        let expr: Expr = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after nirvash_step_expr! body"));
        }
        Ok(Self {
            name,
            prev,
            action,
            next,
            expr,
        })
    }
}

struct TransitionProgramDsl {
    rules: Vec<TransitionRuleDsl>,
}

impl Parse for TransitionProgramDsl {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut rules = Vec::new();
        while !input.is_empty() {
            rules.push(input.parse()?);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        if rules.is_empty() {
            return Err(input.error("nirvash_transition_program! requires at least one rule"));
        }
        Ok(Self { rules })
    }
}

struct TransitionRuleDsl {
    name: Ident,
    guard: Expr,
    updates: Vec<TransitionUpdateDsl>,
}

impl Parse for TransitionRuleDsl {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        input.parse::<nirvash_dsl_kw::rule>()?;
        let name: Ident = input.parse()?;
        input.parse::<nirvash_dsl_kw::when>()?;
        let guard: Expr = input.parse()?;
        input.parse::<Token![=>]>()?;
        let body;
        syn::braced!(body in input);
        let mut updates = Vec::new();
        while !body.is_empty() {
            updates.push(body.parse()?);
        }
        Ok(Self {
            name,
            guard,
            updates,
        })
    }
}

enum TransitionUpdateKind {
    Set,
    Insert,
    Remove,
}

struct TransitionUpdateDsl {
    kind: TransitionUpdateKind,
    target: TransitionTargetPath,
    value: Expr,
}

impl Parse for TransitionUpdateDsl {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let kind = if input.peek(nirvash_dsl_kw::set) {
            input.parse::<nirvash_dsl_kw::set>()?;
            TransitionUpdateKind::Set
        } else if input.peek(nirvash_dsl_kw::insert) {
            input.parse::<nirvash_dsl_kw::insert>()?;
            TransitionUpdateKind::Insert
        } else if input.peek(nirvash_dsl_kw::remove) {
            input.parse::<nirvash_dsl_kw::remove>()?;
            TransitionUpdateKind::Remove
        } else {
            return Err(input.error("expected `set`, `insert`, or `remove`"));
        };
        let target: TransitionTargetPath = input.parse()?;
        input.parse::<Token![<=]>()?;
        let value: Expr = input.parse()?;
        input.parse::<Token![;]>()?;
        Ok(Self {
            kind,
            target,
            value,
        })
    }
}

struct TransitionTargetPath {
    kind: TransitionTargetKind,
    span: Span,
}

enum TransitionTargetKind {
    WholeState,
    FieldPath(Vec<Ident>),
}

impl TransitionTargetPath {
    fn display(&self) -> String {
        match &self.kind {
            TransitionTargetKind::WholeState => "self".to_owned(),
            TransitionTargetKind::FieldPath(segments) => segments
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("."),
        }
    }

    fn access_tokens(&self) -> TokenStream2 {
        match &self.kind {
            TransitionTargetKind::WholeState => quote! { *state },
            TransitionTargetKind::FieldPath(segments) => quote! { state #( . #segments )* },
        }
    }
}

impl Parse for TransitionTargetPath {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(Token![self]) {
            let self_token: Token![self] = input.parse()?;
            return Ok(Self {
                kind: TransitionTargetKind::WholeState,
                span: self_token.span,
            });
        }
        let first: Ident = input.parse()?;
        let span = first.span();
        let mut segments = vec![first];
        while input.peek(Token![.]) {
            input.parse::<Token![.]>()?;
            segments.push(input.parse()?);
        }
        Ok(Self {
            kind: TransitionTargetKind::FieldPath(segments),
            span,
        })
    }
}

struct MatchesMacroArgs {
    value: Expr,
    pattern: TokenStream2,
}

impl Parse for MatchesMacroArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let value: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let pattern: TokenStream2 = input.parse()?;
        if pattern.is_empty() {
            return Err(input.error("expected matches! pattern"));
        }
        Ok(Self { value, pattern })
    }
}

#[derive(Clone, Copy)]
enum BoolDslKind {
    State,
    Step,
    Guard,
}

#[derive(Clone)]
enum PureCallKind {
    Builtin,
    Registered(LitStr),
}

fn is_builtin_pure_method(name: &str) -> bool {
    matches!(
        name,
        "clone"
            | "contains"
            | "difference"
            | "domain"
            | "expect"
            | "intersection"
            | "is_none"
            | "is_max"
            | "is_some"
            | "is_some_and"
            | "join"
            | "lone"
            | "no"
            | "one"
            | "range"
            | "saturating_inc"
            | "some"
            | "subset_of"
            | "transpose"
            | "union"
    )
}

fn is_builtin_pure_function(func: &Expr) -> bool {
    match strip_expr_wrappers(func) {
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Some"),
        _ => false,
    }
}

fn pure_call_kind(expr: &Expr) -> Option<PureCallKind> {
    match expr {
        Expr::MethodCall(ExprMethodCall { method, .. }) => {
            let name = method.to_string();
            if is_builtin_pure_method(&name) {
                Some(PureCallKind::Builtin)
            } else {
                Some(PureCallKind::Registered(LitStr::new(&name, method.span())))
            }
        }
        Expr::Call(ExprCall { func, .. }) => {
            if is_builtin_pure_function(func) {
                Some(PureCallKind::Builtin)
            } else {
                Some(PureCallKind::Registered(expr_source_lit(func)))
            }
        }
        _ => None,
    }
}

fn nested_registered_helper(expr: &Expr) -> Option<LitStr> {
    match expr {
        Expr::Paren(ExprParen { expr: inner, .. }) => nested_registered_helper(inner),
        Expr::Reference(ExprReference { expr: inner, .. }) => nested_registered_helper(inner),
        Expr::Unary(ExprUnary { expr: inner, .. }) => nested_registered_helper(inner),
        Expr::Binary(ExprBinary { left, right, .. }) => {
            nested_registered_helper(left).or_else(|| nested_registered_helper(right))
        }
        Expr::Field(ExprField { base, .. }) => nested_registered_helper(base),
        Expr::If(expr_if) => nested_registered_helper(&expr_if.cond)
            .or_else(|| {
                block_terminal_expr(&expr_if.then_branch).and_then(nested_registered_helper)
            })
            .or_else(|| {
                expr_if
                    .else_branch
                    .as_ref()
                    .and_then(|(_, else_expr)| match else_expr.as_ref() {
                        Expr::Block(block) => {
                            block_terminal_expr(&block.block).and_then(nested_registered_helper)
                        }
                        other => nested_registered_helper(other),
                    })
            }),
        Expr::Macro(expr_macro) if expr_macro.mac.path.is_ident("matches") => {
            let args: MatchesMacroArgs = syn::parse2(expr_macro.mac.tokens.clone()).ok()?;
            nested_registered_helper(&args.value)
        }
        Expr::Call(ExprCall { func, args, .. }) => match pure_call_kind(expr) {
            Some(PureCallKind::Registered(registration)) => Some(registration),
            Some(PureCallKind::Builtin) => nested_registered_helper(func)
                .or_else(|| args.iter().find_map(nested_registered_helper)),
            None => nested_registered_helper(func)
                .or_else(|| args.iter().find_map(nested_registered_helper)),
        },
        Expr::MethodCall(ExprMethodCall { receiver, args, .. }) => match pure_call_kind(expr) {
            Some(PureCallKind::Registered(registration)) => Some(registration),
            Some(PureCallKind::Builtin) => nested_registered_helper(receiver)
                .or_else(|| args.iter().find_map(nested_registered_helper)),
            None => nested_registered_helper(receiver)
                .or_else(|| args.iter().find_map(nested_registered_helper)),
        },
        _ => None,
    }
}

fn expr_field_segments(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Paren(ExprParen { expr: inner, .. }) => expr_field_segments(inner),
        Expr::Path(ExprPath {
            qself: None, path, ..
        }) => {
            if path.leading_colon.is_some()
                || path
                    .segments
                    .iter()
                    .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
            {
                return None;
            }
            Some(
                path.segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect(),
            )
        }
        Expr::Field(ExprField { base, member, .. }) => {
            let mut segments = expr_field_segments(base)?;
            segments.push(member.to_token_stream().to_string());
            Some(segments)
        }
        _ => None,
    }
}

fn block_terminal_expr(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last() {
        Some(Stmt::Expr(expr, _)) => Some(expr),
        _ => None,
    }
}

fn unary_method_arg<'a>(
    whole_expr: &Expr,
    args: &'a syn::punctuated::Punctuated<Expr, Token![,]>,
) -> syn::Result<&'a Expr> {
    let mut args = args.iter();
    let Some(arg) = args.next() else {
        return Err(unsupported_nirvash_expr(whole_expr));
    };
    if args.next().is_some() {
        return Err(unsupported_nirvash_expr(whole_expr));
    }
    Ok(arg)
}

fn ternary_call_args<'a>(
    whole_expr: &Expr,
    args: &'a syn::punctuated::Punctuated<Expr, Token![,]>,
) -> syn::Result<(&'a Expr, &'a Expr, &'a Expr)> {
    let mut args = args.iter();
    let Some(first) = args.next() else {
        return Err(unsupported_nirvash_expr(whole_expr));
    };
    let Some(second) = args.next() else {
        return Err(unsupported_nirvash_expr(whole_expr));
    };
    let Some(third) = args.next() else {
        return Err(unsupported_nirvash_expr(whole_expr));
    };
    if args.next().is_some() {
        return Err(unsupported_nirvash_expr(whole_expr));
    }
    Ok((first, second, third))
}

struct BoolDslContext {
    kind: BoolDslKind,
    binders: Vec<Ident>,
}

impl BoolDslContext {
    fn canonical_root_name(&self, binder_index: usize) -> &'static str {
        match self.kind {
            BoolDslKind::State => "state",
            BoolDslKind::Step => match binder_index {
                0 => "prev",
                1 => "action",
                2 => "next",
                _ => unreachable!("step expressions always bind prev, action, next"),
            },
            BoolDslKind::Guard => match binder_index {
                0 => "prev",
                1 => "action",
                _ => unreachable!("guard expressions always bind prev, action"),
            },
        }
    }

    fn builder_path(&self) -> TokenStream2 {
        match self.kind {
            BoolDslKind::State => quote!(::nirvash::BoolExpr),
            BoolDslKind::Step => quote!(::nirvash::StepExpr),
            BoolDslKind::Guard => quote!(::nirvash::GuardExpr),
        }
    }

    fn value_builder_path(&self) -> TokenStream2 {
        match self.kind {
            BoolDslKind::State => quote!(::nirvash::StateExpr),
            BoolDslKind::Step => quote!(::nirvash::StepValueExpr),
            BoolDslKind::Guard => quote!(::nirvash::GuardValueExpr),
        }
    }

    fn closure_tokens(&self, expr: &Expr) -> TokenStream2 {
        let binders = &self.binders;
        quote! {
            |#(#binders),*| {
                #(let _ = &#binders;)*
                #expr
            }
        }
    }

    fn cloned_closure_tokens(&self, expr: &Expr) -> TokenStream2 {
        let binders = &self.binders;
        quote! {
            |#(#binders),*| {
                #(let _ = &#binders;)*
                (#expr).clone()
            }
        }
    }

    fn matches_closure_tokens(&self, value: &Expr, pattern: &TokenStream2) -> TokenStream2 {
        let binders = &self.binders;
        quote! {
            |#(#binders),*| {
                #(let _ = &#binders;)*
                matches!(#value, #pattern)
            }
        }
    }

    fn bound_field_path(&self, expr: &Expr) -> Option<LitStr> {
        let segments = expr_field_segments(expr)?;
        let first = segments.first()?;
        let binder_index = self.binders.iter().position(|binder| binder == first)?;
        let mut canonical = vec![self.canonical_root_name(binder_index).to_owned()];
        canonical.extend(segments.into_iter().skip(1));
        Some(LitStr::new(&canonical.join("."), expr.span()))
    }

    fn canonical_expr_source(&self, expr: &Expr) -> LitStr {
        self.bound_field_path(expr)
            .unwrap_or_else(|| expr_source_lit(expr))
    }

    fn collect_bound_field_paths(&self, expr: &Expr, paths: &mut BTreeSet<String>) {
        if let Some(path) = self.bound_field_path(expr) {
            paths.insert(path.value());
            return;
        }
        match expr {
            Expr::Paren(ExprParen { expr: inner, .. })
            | Expr::Reference(ExprReference { expr: inner, .. })
            | Expr::Unary(ExprUnary { expr: inner, .. }) => {
                self.collect_bound_field_paths(inner, paths);
            }
            Expr::Binary(ExprBinary { left, right, .. }) => {
                self.collect_bound_field_paths(left, paths);
                self.collect_bound_field_paths(right, paths);
            }
            Expr::Field(ExprField { base, .. }) => {
                self.collect_bound_field_paths(base, paths);
            }
            Expr::If(expr_if) => {
                self.collect_bound_field_paths(&expr_if.cond, paths);
                if let Some(then_expr) = block_terminal_expr(&expr_if.then_branch) {
                    self.collect_bound_field_paths(then_expr, paths);
                }
                if let Some((_, else_expr)) = &expr_if.else_branch {
                    match else_expr.as_ref() {
                        Expr::Block(block) => {
                            if let Some(else_expr) = block_terminal_expr(&block.block) {
                                self.collect_bound_field_paths(else_expr, paths);
                            }
                        }
                        other => self.collect_bound_field_paths(other, paths),
                    }
                }
            }
            Expr::Macro(expr_macro) if expr_macro.mac.path.is_ident("matches") => {
                if let Ok(args) = syn::parse2::<MatchesMacroArgs>(expr_macro.mac.tokens.clone()) {
                    self.collect_bound_field_paths(&args.value, paths);
                }
            }
            Expr::Call(ExprCall { func, args, .. }) => {
                self.collect_bound_field_paths(func, paths);
                for arg in args {
                    self.collect_bound_field_paths(arg, paths);
                }
            }
            Expr::MethodCall(ExprMethodCall { receiver, args, .. }) => {
                self.collect_bound_field_paths(receiver, paths);
                for arg in args {
                    self.collect_bound_field_paths(arg, paths);
                }
            }
            _ => {}
        }
    }

    fn pure_call_read_paths(&self, expr: &Expr) -> TokenStream2 {
        let mut paths = BTreeSet::new();
        self.collect_bound_field_paths(expr, &mut paths);
        let paths = paths
            .into_iter()
            .map(|path| LitStr::new(&path, expr.span()))
            .collect::<Vec<_>>();
        quote! { &[#(#paths),*] }
    }

    fn lower_pure_call(
        &self,
        builder: &TokenStream2,
        name: LitStr,
        expr: &Expr,
        eval: TokenStream2,
    ) -> TokenStream2 {
        let read_paths = self.pure_call_read_paths(expr);
        match pure_call_kind(expr) {
            Some(PureCallKind::Builtin) => {
                if let Some(registration) = nested_registered_helper(expr) {
                    quote! { #builder::registered_pure_call_with_paths(#name, #registration, #read_paths, #eval) }
                } else {
                    quote! { #builder::builtin_pure_call_with_paths(#name, #read_paths, #eval) }
                }
            }
            Some(PureCallKind::Registered(registration)) => {
                quote! { #builder::registered_pure_call_with_paths(#name, #registration, #read_paths, #eval) }
            }
            None => quote! { #builder::pure_call_with_paths(#name, #read_paths, #eval) },
        }
    }

    fn lower_value_expr(
        &self,
        expr: &Expr,
        explicit_name: Option<LitStr>,
    ) -> syn::Result<TokenStream2> {
        match expr {
            Expr::Paren(ExprParen { expr: inner, .. }) => {
                self.lower_value_expr(inner, explicit_name)
            }
            Expr::Unary(ExprUnary {
                op: UnOp::Neg(_),
                expr: inner,
                ..
            }) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let inner = self.lower_value_expr(inner, None)?;
                Ok(quote! { #builder::neg(#name, #inner) })
            }
            Expr::Lit(_) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let repr = expr_source_lit(expr);
                Ok(quote! { #builder::literal_with_repr(#name, #repr, { #expr }) })
            }
            Expr::Path(_) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                if let Some(path) = self.bound_field_path(expr) {
                    let eval = self.cloned_closure_tokens(expr);
                    Ok(quote! { #builder::field(#name, #path, #eval) })
                } else {
                    let repr = expr_source_lit(expr);
                    Ok(quote! { #builder::literal_with_repr(#name, #repr, { #expr }) })
                }
            }
            Expr::Field(ExprField { .. }) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                if let Some(path) = self.bound_field_path(expr) {
                    let eval = self.cloned_closure_tokens(expr);
                    Ok(quote! { #builder::field(#name, #path, #eval) })
                } else {
                    let eval = self.closure_tokens(expr);
                    let repr = expr_source_lit(expr);
                    Ok(quote! { #builder::opaque(#name, #repr, #eval) })
                }
            }
            Expr::Binary(ExprBinary {
                op: BinOp::Add(_),
                left,
                right,
                ..
            }) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let lhs = self.lower_value_expr(left, None)?;
                let rhs = self.lower_value_expr(right, None)?;
                Ok(quote! { #builder::add(#name, #lhs, #rhs) })
            }
            Expr::Binary(ExprBinary {
                op: BinOp::Sub(_),
                left,
                right,
                ..
            }) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let lhs = self.lower_value_expr(left, None)?;
                let rhs = self.lower_value_expr(right, None)?;
                Ok(quote! { #builder::sub(#name, #lhs, #rhs) })
            }
            Expr::Binary(ExprBinary {
                op: BinOp::Mul(_),
                left,
                right,
                ..
            }) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let lhs = self.lower_value_expr(left, None)?;
                let rhs = self.lower_value_expr(right, None)?;
                Ok(quote! { #builder::mul(#name, #lhs, #rhs) })
            }
            Expr::If(expr_if) => self.lower_if_value_expr(expr, expr_if, explicit_name),
            Expr::MethodCall(method_call) => {
                if let Some(lowered) =
                    self.lower_builtin_value_method(expr, method_call, explicit_name.clone())
                {
                    return lowered;
                }
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let eval = self.closure_tokens(expr);
                Ok(self.lower_pure_call(&builder, name, expr, eval))
            }
            Expr::Call(ExprCall { .. }) => {
                let builder = self.value_builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let eval = self.closure_tokens(expr);
                Ok(self.lower_pure_call(&builder, name, expr, eval))
            }
            _ => Err(unsupported_nirvash_expr(expr)),
        }
    }

    fn lower_if_value_expr(
        &self,
        whole_expr: &Expr,
        expr_if: &ExprIf,
        explicit_name: Option<LitStr>,
    ) -> syn::Result<TokenStream2> {
        let Some(then_expr) = block_terminal_expr(&expr_if.then_branch) else {
            return Err(unsupported_nirvash_expr(whole_expr));
        };
        let Some((_, else_expr)) = &expr_if.else_branch else {
            return Err(unsupported_nirvash_expr(whole_expr));
        };
        let else_expr = match else_expr.as_ref() {
            Expr::Block(block) => block_terminal_expr(&block.block)
                .ok_or_else(|| unsupported_nirvash_expr(whole_expr))?,
            other => other,
        };
        let builder = self.value_builder_path();
        let name = explicit_name.unwrap_or_else(|| expr_source_lit(whole_expr));
        let condition = self.lower_expr(&expr_if.cond, None)?;
        let then_branch = self.lower_value_expr(then_expr, None)?;
        let else_branch = self.lower_value_expr(else_expr, None)?;
        Ok(quote! { #builder::if_else(#name, #condition, #then_branch, #else_branch) })
    }

    fn lower_builtin_value_method(
        &self,
        whole_expr: &Expr,
        method_call: &ExprMethodCall,
        explicit_name: Option<LitStr>,
    ) -> Option<syn::Result<TokenStream2>> {
        let method = method_call.method.to_string();
        let builder = self.value_builder_path();
        let name = explicit_name.unwrap_or_else(|| expr_source_lit(whole_expr));
        let eval = self.closure_tokens(whole_expr);
        let receiver = strip_expr_wrappers(&method_call.receiver);
        let arg = match method.as_str() {
            "union" | "intersection" | "difference" => {
                match unary_method_arg(whole_expr, &method_call.args) {
                    Ok(arg) => strip_expr_wrappers(arg),
                    Err(err) => return Some(Err(err)),
                }
            }
            _ => return None,
        };
        let lhs = match self.lower_value_expr(receiver, None) {
            Ok(lhs) => lhs,
            Err(err) => return Some(Err(err)),
        };
        let rhs = match self.lower_value_expr(arg, None) {
            Ok(rhs) => rhs,
            Err(err) => return Some(Err(err)),
        };
        let lowered = match method.as_str() {
            "union" => quote! { #builder::union_expr(#name, #lhs, #rhs, #eval) },
            "intersection" => {
                quote! { #builder::intersection_expr(#name, #lhs, #rhs, #eval) }
            }
            "difference" => quote! { #builder::difference_expr(#name, #lhs, #rhs, #eval) },
            _ => unreachable!("checked above"),
        };
        Some(Ok(lowered))
    }

    fn lower_expr(&self, expr: &Expr, explicit_name: Option<LitStr>) -> syn::Result<TokenStream2> {
        match expr {
            Expr::Paren(ExprParen { expr: inner, .. }) => self.lower_expr(inner, explicit_name),
            Expr::Lit(ExprLit {
                lit: Lit::Bool(value),
                ..
            }) => {
                let builder = self.builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let value = value.value;
                Ok(quote! { #builder::literal(#name, #value) })
            }
            Expr::Unary(ExprUnary {
                op: UnOp::Not(_),
                expr: inner,
                ..
            }) => {
                let builder = self.builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let inner = self.lower_expr(inner, None)?;
                Ok(quote! { #builder::not(#name, #inner) })
            }
            Expr::Binary(binary) => self.lower_binary(expr, binary, explicit_name),
            Expr::Macro(expr_macro) if expr_macro.mac.path.is_ident("matches") => {
                self.lower_matches(expr, expr_macro, explicit_name)
            }
            Expr::Field(ExprField { .. }) => {
                let builder = self.builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let path = expr_source_lit(expr);
                let eval = self.closure_tokens(expr);
                Ok(quote! { #builder::field(#name, #path, #eval) })
            }
            Expr::MethodCall(method_call) => {
                if let Some(lowered) =
                    self.lower_builtin_predicate_method(expr, method_call, explicit_name.clone())
                {
                    return lowered;
                }
                let builder = self.builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let eval = self.closure_tokens(expr);
                Ok(self.lower_pure_call(&builder, name, expr, eval))
            }
            Expr::Call(ExprCall { .. }) => {
                let builder = self.builder_path();
                let name = explicit_name.unwrap_or_else(|| expr_source_lit(expr));
                let eval = self.closure_tokens(expr);
                Ok(self.lower_pure_call(&builder, name, expr, eval))
            }
            _ => Err(unsupported_nirvash_expr(expr)),
        }
    }

    fn lower_binary(
        &self,
        whole_expr: &Expr,
        binary: &ExprBinary,
        explicit_name: Option<LitStr>,
    ) -> syn::Result<TokenStream2> {
        let builder = self.builder_path();
        let name = explicit_name.unwrap_or_else(|| expr_source_lit(whole_expr));
        match &binary.op {
            BinOp::And(_) => {
                let lhs = self.lower_expr(&binary.left, None)?;
                let rhs = self.lower_expr(&binary.right, None)?;
                Ok(quote! { #builder::and(#name, vec![#lhs, #rhs]) })
            }
            BinOp::Or(_) => {
                let lhs = self.lower_expr(&binary.left, None)?;
                let rhs = self.lower_expr(&binary.right, None)?;
                Ok(quote! { #builder::or(#name, vec![#lhs, #rhs]) })
            }
            BinOp::Eq(_) => self.lower_comparison("eq", name, &binary.left, &binary.right),
            BinOp::Ne(_) => self.lower_comparison("ne", name, &binary.left, &binary.right),
            BinOp::Lt(_) => self.lower_comparison("lt", name, &binary.left, &binary.right),
            BinOp::Le(_) => self.lower_comparison("le", name, &binary.left, &binary.right),
            BinOp::Gt(_) => self.lower_comparison("gt", name, &binary.left, &binary.right),
            BinOp::Ge(_) => self.lower_comparison("ge", name, &binary.left, &binary.right),
            _ => Err(unsupported_nirvash_expr(whole_expr)),
        }
    }

    fn lower_comparison(
        &self,
        method: &str,
        name: LitStr,
        lhs: &Expr,
        rhs: &Expr,
    ) -> syn::Result<TokenStream2> {
        let builder = self.builder_path();
        let method = format_ident!("{}_expr", method);
        let lhs = self.lower_value_expr(lhs, None)?;
        let rhs = self.lower_value_expr(rhs, None)?;
        Ok(quote! { #builder::#method(#name, #lhs, #rhs) })
    }

    fn lower_matches(
        &self,
        whole_expr: &Expr,
        expr_macro: &ExprMacro,
        explicit_name: Option<LitStr>,
    ) -> syn::Result<TokenStream2> {
        let builder = self.builder_path();
        let name = explicit_name.unwrap_or_else(|| expr_source_lit(whole_expr));
        let args: MatchesMacroArgs = syn::parse2(expr_macro.mac.tokens.clone())?;
        let value = self.canonical_expr_source(&args.value);
        let pattern = token_stream_source_lit(&args.pattern, whole_expr.span());
        let eval = self.matches_closure_tokens(&args.value, &args.pattern);
        Ok(quote! { #builder::matches_variant(#name, #value, #pattern, #eval) })
    }

    fn lower_builtin_predicate_method(
        &self,
        whole_expr: &Expr,
        method_call: &ExprMethodCall,
        explicit_name: Option<LitStr>,
    ) -> Option<syn::Result<TokenStream2>> {
        let method = method_call.method.to_string();
        let builder = self.builder_path();
        let name = explicit_name.unwrap_or_else(|| expr_source_lit(whole_expr));
        let eval = self.closure_tokens(whole_expr);
        let receiver = strip_expr_wrappers(&method_call.receiver);
        let arg = match method.as_str() {
            "contains" | "subset_of" => match unary_method_arg(whole_expr, &method_call.args) {
                Ok(arg) => strip_expr_wrappers(arg),
                Err(err) => return Some(Err(err)),
            },
            _ => return None,
        };
        let lhs = match self.lower_value_expr(receiver, None) {
            Ok(lhs) => lhs,
            Err(err) => return Some(Err(err)),
        };
        let rhs = match self.lower_value_expr(arg, None) {
            Ok(rhs) => rhs,
            Err(err) => return Some(Err(err)),
        };
        let lowered = match method.as_str() {
            "contains" => quote! { #builder::contains_expr(#name, #lhs, #rhs, #eval) },
            "subset_of" => quote! { #builder::subset_of_expr(#name, #lhs, #rhs, #eval) },
            _ => unreachable!("checked above"),
        };
        Some(Ok(lowered))
    }
}

fn expand_nirvash_expr(input: NamedStateExprInput) -> syn::Result<TokenStream2> {
    let context = BoolDslContext {
        kind: BoolDslKind::State,
        binders: vec![input.state],
    };
    let name = LitStr::new(&input.name.to_string(), input.name.span());
    context.lower_expr(&input.expr, Some(name))
}

fn expand_nirvash_step_expr(input: NamedStepExprInput) -> syn::Result<TokenStream2> {
    let context = BoolDslContext {
        kind: BoolDslKind::Step,
        binders: vec![input.prev, input.action, input.next],
    };
    let name = LitStr::new(&input.name.to_string(), input.name.span());
    context.lower_expr(&input.expr, Some(name))
}

fn expand_nirvash_transition_program(input: TransitionProgramDsl) -> syn::Result<TokenStream2> {
    let guard_context = BoolDslContext {
        kind: BoolDslKind::Guard,
        binders: vec![
            Ident::new("prev", Span::call_site()),
            Ident::new("action", Span::call_site()),
        ],
    };
    let rules = input
        .rules
        .into_iter()
        .map(|rule| lower_transition_rule(rule, &guard_context))
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(quote! {
        ::nirvash::TransitionProgram::new(vec![#(#rules),*])
    })
}

fn lower_transition_rule(
    rule: TransitionRuleDsl,
    guard_context: &BoolDslContext,
) -> syn::Result<TokenStream2> {
    let name = LitStr::new(&rule.name.to_string(), rule.name.span());
    let guard = guard_context.lower_expr(&rule.guard, Some(name.clone()))?;
    let ops = rule
        .updates
        .into_iter()
        .map(lower_transition_update)
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(quote! {
        ::nirvash::TransitionRule::ast(
            #name,
            #guard,
            ::nirvash::UpdateProgram::ast(#name, vec![#(#ops),*]),
        )
    })
}

fn bound_update_field_path(expr: &Expr) -> Option<LitStr> {
    let segments = expr_field_segments(expr)?;
    matches!(
        segments.first().map(String::as_str),
        Some("prev" | "state" | "action")
    )
    .then(|| LitStr::new(&segments.join("."), expr.span()))
}

fn collect_bound_update_field_paths(expr: &Expr, paths: &mut BTreeSet<String>) {
    if let Some(path) = bound_update_field_path(expr) {
        paths.insert(path.value());
        return;
    }
    match expr {
        Expr::Paren(ExprParen { expr: inner, .. })
        | Expr::Reference(ExprReference { expr: inner, .. })
        | Expr::Unary(ExprUnary { expr: inner, .. }) => {
            collect_bound_update_field_paths(inner, paths);
        }
        Expr::Binary(ExprBinary { left, right, .. }) => {
            collect_bound_update_field_paths(left, paths);
            collect_bound_update_field_paths(right, paths);
        }
        Expr::Field(ExprField { base, .. }) => {
            collect_bound_update_field_paths(base, paths);
        }
        Expr::Struct(ExprStruct { fields, rest, .. }) => {
            for field in fields {
                collect_bound_update_field_paths(&field.expr, paths);
            }
            if let Some(rest) = rest {
                collect_bound_update_field_paths(rest, paths);
            }
        }
        Expr::If(expr_if) => {
            collect_bound_update_field_paths(&expr_if.cond, paths);
            if let Some(then_expr) = block_terminal_expr(&expr_if.then_branch) {
                collect_bound_update_field_paths(then_expr, paths);
            }
            if let Some((_, else_expr)) = &expr_if.else_branch {
                match else_expr.as_ref() {
                    Expr::Block(block) => {
                        if let Some(else_expr) = block_terminal_expr(&block.block) {
                            collect_bound_update_field_paths(else_expr, paths);
                        }
                    }
                    other => collect_bound_update_field_paths(other, paths),
                }
            }
        }
        Expr::Macro(expr_macro) if expr_macro.mac.path.is_ident("matches") => {
            if let Ok(args) = syn::parse2::<MatchesMacroArgs>(expr_macro.mac.tokens.clone()) {
                collect_bound_update_field_paths(&args.value, paths);
            }
        }
        Expr::Call(ExprCall { func, args, .. }) => {
            collect_bound_update_field_paths(func, paths);
            for arg in args {
                collect_bound_update_field_paths(arg, paths);
            }
        }
        Expr::MethodCall(ExprMethodCall { receiver, args, .. }) => {
            collect_bound_update_field_paths(receiver, paths);
            for arg in args {
                collect_bound_update_field_paths(arg, paths);
            }
        }
        _ => {}
    }
}

fn update_pure_call_read_paths(expr: &Expr) -> TokenStream2 {
    let mut paths = BTreeSet::new();
    collect_bound_update_field_paths(expr, &mut paths);
    let paths = paths
        .into_iter()
        .map(|path| LitStr::new(&path, expr.span()))
        .collect::<Vec<_>>();
    quote! { &[#(#paths),*] }
}

fn lower_update_value_expr(expr: &Expr) -> syn::Result<TokenStream2> {
    match expr {
        Expr::Paren(ExprParen { expr: inner, .. }) => lower_update_value_expr(inner),
        Expr::Unary(ExprUnary {
            op: UnOp::Neg(_),
            expr: inner,
            ..
        }) => {
            let inner = lower_update_value_expr(inner)?;
            Ok(quote! { ::nirvash::UpdateValueExprAst::neg(#inner) })
        }
        Expr::Lit(_) => {
            let repr = expr_source_lit(expr);
            Ok(quote! { ::nirvash::UpdateValueExprAst::literal(#repr) })
        }
        Expr::Path(_) => {
            if let Some(path) = bound_update_field_path(expr) {
                Ok(quote! { ::nirvash::UpdateValueExprAst::field(#path) })
            } else {
                let repr = expr_source_lit(expr);
                Ok(quote! { ::nirvash::UpdateValueExprAst::literal(#repr) })
            }
        }
        Expr::Field(ExprField { .. }) => {
            if let Some(path) = bound_update_field_path(expr) {
                Ok(quote! { ::nirvash::UpdateValueExprAst::field(#path) })
            } else {
                let repr = expr_source_lit(expr);
                Ok(quote! { ::nirvash::UpdateValueExprAst::opaque(#repr) })
            }
        }
        Expr::Struct(expr_struct) => lower_struct_update_expr(expr, expr_struct),
        Expr::Binary(ExprBinary {
            op: BinOp::Add(_),
            left,
            right,
            ..
        }) => {
            let lhs = lower_update_value_expr(left)?;
            let rhs = lower_update_value_expr(right)?;
            Ok(quote! { ::nirvash::UpdateValueExprAst::add(#lhs, #rhs) })
        }
        Expr::Binary(ExprBinary {
            op: BinOp::Sub(_),
            left,
            right,
            ..
        }) => {
            let lhs = lower_update_value_expr(left)?;
            let rhs = lower_update_value_expr(right)?;
            Ok(quote! { ::nirvash::UpdateValueExprAst::sub(#lhs, #rhs) })
        }
        Expr::Binary(ExprBinary {
            op: BinOp::Mul(_),
            left,
            right,
            ..
        }) => {
            let lhs = lower_update_value_expr(left)?;
            let rhs = lower_update_value_expr(right)?;
            Ok(quote! { ::nirvash::UpdateValueExprAst::mul(#lhs, #rhs) })
        }
        Expr::If(expr_if) => {
            let Some(then_expr) = block_terminal_expr(&expr_if.then_branch) else {
                return Err(unsupported_nirvash_expr(expr));
            };
            let Some((_, else_expr)) = &expr_if.else_branch else {
                return Err(unsupported_nirvash_expr(expr));
            };
            let else_expr = match else_expr.as_ref() {
                Expr::Block(block) => block_terminal_expr(&block.block)
                    .ok_or_else(|| unsupported_nirvash_expr(expr))?,
                other => other,
            };
            let guard_context = BoolDslContext {
                kind: BoolDslKind::Guard,
                binders: vec![
                    Ident::new("prev", Span::call_site()),
                    Ident::new("action", Span::call_site()),
                ],
            };
            let condition = guard_context.lower_expr(&expr_if.cond, None)?;
            let then_branch = lower_update_value_expr(then_expr)?;
            let else_branch = lower_update_value_expr(else_expr)?;
            Ok(
                quote! { ::nirvash::UpdateValueExprAst::if_else(#condition, #then_branch, #else_branch) },
            )
        }
        Expr::MethodCall(method_call) => {
            if let Some(lowered) = lower_builtin_update_method(expr, method_call) {
                return lowered;
            }
            let read_paths = update_pure_call_read_paths(expr);
            match pure_call_kind(expr) {
                Some(PureCallKind::Builtin) => {
                    let name = expr_source_lit(expr);
                    if let Some(registration) = nested_registered_helper(expr) {
                        Ok(quote! {
                            ::nirvash::UpdateValueExprAst::registered_pure_call_with_paths(#name, #registration, #read_paths)
                        })
                    } else {
                        Ok(
                            quote! { ::nirvash::UpdateValueExprAst::builtin_pure_call_with_paths(#name, #read_paths) },
                        )
                    }
                }
                Some(PureCallKind::Registered(registration)) => {
                    let name = expr_source_lit(expr);
                    Ok(quote! {
                        ::nirvash::UpdateValueExprAst::registered_pure_call_with_paths(#name, #registration, #read_paths)
                    })
                }
                None => {
                    let repr = expr_source_lit(expr);
                    Ok(quote! { ::nirvash::UpdateValueExprAst::opaque(#repr) })
                }
            }
        }
        Expr::Call(ExprCall { .. }) => {
            if let Some(lowered) = lower_builtin_update_call(expr) {
                return lowered;
            }
            let read_paths = update_pure_call_read_paths(expr);
            match pure_call_kind(expr) {
                Some(PureCallKind::Builtin) => {
                    let name = expr_source_lit(expr);
                    if let Some(registration) = nested_registered_helper(expr) {
                        Ok(quote! {
                            ::nirvash::UpdateValueExprAst::registered_pure_call_with_paths(#name, #registration, #read_paths)
                        })
                    } else {
                        Ok(
                            quote! { ::nirvash::UpdateValueExprAst::builtin_pure_call_with_paths(#name, #read_paths) },
                        )
                    }
                }
                Some(PureCallKind::Registered(registration)) => {
                    let name = expr_source_lit(expr);
                    Ok(quote! {
                        ::nirvash::UpdateValueExprAst::registered_pure_call_with_paths(#name, #registration, #read_paths)
                    })
                }
                None => {
                    let repr = expr_source_lit(expr);
                    Ok(quote! { ::nirvash::UpdateValueExprAst::opaque(#repr) })
                }
            }
        }
        _ => Err(unsupported_nirvash_expr(expr)),
    }
}

fn lower_struct_update_expr(
    whole_expr: &Expr,
    expr_struct: &ExprStruct,
) -> syn::Result<TokenStream2> {
    let Some(rest) = &expr_struct.rest else {
        return Err(unsupported_nirvash_expr(whole_expr));
    };
    let mut lowered = lower_update_value_expr(rest)?;
    for field in &expr_struct.fields {
        let Member::Named(ident) = &field.member else {
            return Err(unsupported_nirvash_expr(whole_expr));
        };
        let field_name = LitStr::new(&ident.to_string(), ident.span());
        let value = lower_update_value_expr(&field.expr)?;
        lowered = quote! {
            ::nirvash::UpdateValueExprAst::record_update(#lowered, #field_name, #value)
        };
    }
    Ok(lowered)
}

fn lower_builtin_update_call(whole_expr: &Expr) -> Option<syn::Result<TokenStream2>> {
    let Expr::Call(ExprCall { func, args, .. }) = whole_expr else {
        return None;
    };
    let Expr::Path(ExprPath { path, .. }) = strip_expr_wrappers(func) else {
        return None;
    };
    let mut segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string());
    let root = segments.next()?;
    let name = segments.next()?;
    if root != "nirvash" || segments.next().is_some() {
        return None;
    }
    let (base, key, value) = match ternary_call_args(whole_expr, args) {
        Ok(args) => args,
        Err(err) => return Some(Err(err)),
    };
    let base = match lower_update_value_expr(base) {
        Ok(base) => base,
        Err(err) => return Some(Err(err)),
    };
    let key = match lower_update_value_expr(key) {
        Ok(key) => key,
        Err(err) => return Some(Err(err)),
    };
    let value = match lower_update_value_expr(value) {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    let lowered = match name.as_str() {
        "sequence_update" => {
            quote! { ::nirvash::UpdateValueExprAst::sequence_update(#base, #key, #value) }
        }
        "function_update" => {
            quote! { ::nirvash::UpdateValueExprAst::function_update(#base, #key, #value) }
        }
        _ => return None,
    };
    Some(Ok(lowered))
}

fn lower_builtin_update_method(
    whole_expr: &Expr,
    method_call: &ExprMethodCall,
) -> Option<syn::Result<TokenStream2>> {
    let arg = match method_call.method.to_string().as_str() {
        "union" | "intersection" | "difference" => {
            match unary_method_arg(whole_expr, &method_call.args) {
                Ok(arg) => strip_expr_wrappers(arg),
                Err(err) => return Some(Err(err)),
            }
        }
        _ => return None,
    };
    let lhs = match lower_update_value_expr(strip_expr_wrappers(&method_call.receiver)) {
        Ok(lhs) => lhs,
        Err(err) => return Some(Err(err)),
    };
    let rhs = match lower_update_value_expr(arg) {
        Ok(rhs) => rhs,
        Err(err) => return Some(Err(err)),
    };
    let lowered = match method_call.method.to_string().as_str() {
        "union" => quote! { ::nirvash::UpdateValueExprAst::union(#lhs, #rhs) },
        "intersection" => quote! { ::nirvash::UpdateValueExprAst::intersection(#lhs, #rhs) },
        "difference" => quote! { ::nirvash::UpdateValueExprAst::difference(#lhs, #rhs) },
        _ => unreachable!("checked above"),
    };
    Some(Ok(lowered))
}

fn lower_transition_update(update: TransitionUpdateDsl) -> syn::Result<TokenStream2> {
    let target = LitStr::new(&update.target.display(), update.target.span);
    let value_ast = lower_update_value_expr(&update.value)?;
    let access = update.target.access_tokens();
    let rhs = &update.value;
    let tokens = match update.kind {
        TransitionUpdateKind::Set => quote! {
            ::nirvash::UpdateOp::assign_ast(#target, #value_ast, |prev, state, action| {
                let __nirvash_value = { #rhs };
                let _ = (&prev, &action);
                #access = __nirvash_value;
            })
        },
        TransitionUpdateKind::Insert => quote! {
            ::nirvash::UpdateOp::set_insert_ast(#target, #value_ast, |prev, state, action| {
                let __nirvash_item = { #rhs };
                let _ = (&prev, &action);
                #access.insert(__nirvash_item);
            })
        },
        TransitionUpdateKind::Remove => quote! {
            ::nirvash::UpdateOp::set_remove_ast(#target, #value_ast, |prev, state, action| {
                let __nirvash_item = { #rhs };
                let _ = (&prev, &action);
                #access.remove(&__nirvash_item);
            })
        },
    };
    Ok(tokens)
}

fn expr_source_lit(expr: &Expr) -> LitStr {
    LitStr::new(&expr.to_token_stream().to_string(), expr.span())
}

fn token_stream_source_lit(tokens: &TokenStream2, span: Span) -> LitStr {
    LitStr::new(&tokens.to_string(), span)
}

fn strip_expr_wrappers(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(ExprParen { expr: inner, .. })
        | Expr::Reference(ExprReference { expr: inner, .. }) => strip_expr_wrappers(inner),
        _ => expr,
    }
}

fn unsupported_nirvash_expr(expr: &Expr) -> syn::Error {
    syn::Error::new_spanned(
        expr,
        "unsupported nirvash expression; supported forms are `!`, unary `-`, `&&`, `||`, `==`, `!=`, `<`, `<=`, `>`, `>=`, `+`, binary `-`, `*`, `if/else`, `matches!(..)`, field reads, struct update, function/method calls, and parentheses",
    )
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
            if attr.path().is_ident("finite_model_domain_invariant")
                || attr.path().is_ident("signature_invariant")
            {
                if args.helper_invariant.is_some() {
                    return Err(syn::Error::new(
                        attr.span(),
                        "duplicate #[finite_model_domain_invariant(...)] attribute",
                    ));
                }
                args.helper_invariant = Some(parse_self_expr_attribute(attr)?);
                continue;
            }
            if !(attr.path().is_ident("finite_model_domain") || attr.path().is_ident("signature")) {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("custom") {
                    args.custom = true;
                    return Ok(());
                }
                if meta.path.is_ident("domain_fn") {
                    return Err(meta.error(
                        "#[finite_model_domain(domain_fn = ...)] is removed; use #[finite_model_domain(custom)] and implement the generated companion trait instead",
                    ));
                }
                if meta.path.is_ident("invariant_fn") {
                    return Err(meta.error(
                        "#[finite_model_domain(invariant_fn = ...)] is removed; use #[finite_model_domain(custom)] and override the generated companion trait invariant hook instead",
                    ));
                }
                if meta.path.is_ident("skip_invariant") {
                    return Err(meta.error(
                        "#[finite_model_domain(skip_invariant)] is removed; use #[finite_model_domain(custom)] and rely on the companion trait default invariant hook instead",
                    ));
                }
                if meta.path.is_ident("range") {
                    let lit: LitStr = meta.value()?.parse()?;
                    args.range = Some(syn::parse_str(&lit.value())?);
                    return Ok(());
                }
                if meta.path.is_ident("filter") {
                    if args.filter.is_some() {
                        return Err(meta.error("duplicate finite_model_domain filter"));
                    }
                    args.filter = Some(parse_self_expr_meta(&meta)?);
                    return Ok(());
                }
                if meta.path.is_ident("bounds") {
                    parse_bounds_meta(&meta, &mut args.bounds)?;
                    return Ok(());
                }
                Err(meta.error("unsupported #[finite_model_domain(...)] argument"))
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
            if !(attr.path().is_ident("finite_model") || attr.path().is_ident("sig")) {
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
                        "unsupported #[finite_model(...)] argument",
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
    subsystems: Vec<Path>,
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
                "subsystems" => args.subsystems = parse_path_list(&content)?,
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
                        "init = ... is no longer supported; FrontendSpec::initial_states() is the canonical source of initial states",
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
                        "init = ... is no longer supported; FrontendSpec::initial_states() is the canonical source of initial states",
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
    probe_state_ty: Option<Type>,
    probe_output_ty: Option<Type>,
    observe_state: Option<Expr>,
    observe_output: Option<Expr>,
    dispatch_input: Option<Expr>,
    input_codec: Option<Path>,
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

struct ProjectionModelFieldAssign {
    target: Ident,
    value: Expr,
}

struct ProjectionModelStateAssign {
    target: TokenStream2,
    value: Expr,
}

enum ProjectionModelOutputValue {
    Drop,
    Expr(Expr),
}

struct ProjectionModelOutputArm {
    pattern: Pat,
    value: ProjectionModelOutputValue,
}

struct ProjectionModelArgs {
    probe_state_ty: Type,
    probe_output_ty: Type,
    summary_state_ty: Type,
    summary_output_ty: Type,
    abstract_state_ty: Type,
    expected_output_ty: Type,
    probe_state_domain: Option<Path>,
    summary_output_domain: Option<Path>,
    state_seed: Expr,
    state_summary: Vec<ProjectionModelFieldAssign>,
    output_summary: Vec<ProjectionModelFieldAssign>,
    state_abstract: Vec<ProjectionModelStateAssign>,
    output_abstract: Vec<ProjectionModelOutputArm>,
    item: ItemImpl,
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

impl Parse for ProjectionModelFieldAssign {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            target: input.parse()?,
            value: {
                let _le: Token![<=] = input.parse()?;
                input.parse()?
            },
        })
    }
}

impl Parse for ProjectionModelStateAssign {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut target = TokenStream2::new();
        while !input.peek(Token![<=]) {
            let token: TokenTree = input.parse()?;
            target.extend(std::iter::once(token));
        }
        Ok(Self {
            target,
            value: {
                let _le: Token![<=] = input.parse()?;
                input.parse()?
            },
        })
    }
}

impl Parse for ProjectionModelOutputArm {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let pattern = input.call(Pat::parse_single)?;
        let _fat_arrow: Token![=>] = input.parse()?;
        let value = if input.peek(Ident) {
            let fork = input.fork();
            let ident: Ident = fork.parse()?;
            if ident == "drop" {
                let _drop: Ident = input.parse()?;
                ProjectionModelOutputValue::Drop
            } else {
                ProjectionModelOutputValue::Expr(input.parse()?)
            }
        } else {
            ProjectionModelOutputValue::Expr(input.parse()?)
        };
        Ok(Self { pattern, value })
    }
}

impl Parse for ProjectionModelArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut probe_state_ty = None;
        let mut probe_output_ty = None;
        let mut summary_state_ty = None;
        let mut summary_output_ty = None;
        let mut abstract_state_ty = None;
        let mut expected_output_ty = None;
        let mut probe_state_domain = None;
        let mut summary_output_domain = None;
        let mut state_seed = None;
        let mut state_summary = None;
        let mut output_summary = None;
        let mut state_abstract = None;
        let mut output_abstract = None;
        let mut item = None;

        while !input.is_empty() {
            if input.peek(Token![impl]) {
                item = Some(input.parse()?);
                break;
            }
            let ident: Ident = input.parse()?;
            if input.peek(Token![=]) {
                let _eq: Token![=] = input.parse()?;
                match ident.to_string().as_str() {
                    "probe_state" => probe_state_ty = Some(input.parse()?),
                    "probe_output" => probe_output_ty = Some(input.parse()?),
                    "summary_state" => summary_state_ty = Some(input.parse()?),
                    "summary_output" => summary_output_ty = Some(input.parse()?),
                    "abstract_state" => abstract_state_ty = Some(input.parse()?),
                    "expected_output" => expected_output_ty = Some(input.parse()?),
                    "probe_state_domain" => probe_state_domain = Some(input.parse()?),
                    "summary_output_domain" => summary_output_domain = Some(input.parse()?),
                    "state_seed" => state_seed = Some(input.parse()?),
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            "unsupported nirvash_projection_model argument",
                        ));
                    }
                }
            } else {
                let content;
                syn::braced!(content in input);
                match ident.to_string().as_str() {
                    "state_summary" => {
                        let mut entries = Vec::new();
                        while !content.is_empty() {
                            entries.push(content.parse()?);
                            if content.peek(Token![,]) {
                                let _ = content.parse::<Token![,]>()?;
                            }
                        }
                        state_summary = Some(entries);
                    }
                    "output_summary" => {
                        let mut entries = Vec::new();
                        while !content.is_empty() {
                            entries.push(content.parse()?);
                            if content.peek(Token![,]) {
                                let _ = content.parse::<Token![,]>()?;
                            }
                        }
                        output_summary = Some(entries);
                    }
                    "state_abstract" => {
                        let mut entries = Vec::new();
                        while !content.is_empty() {
                            entries.push(content.parse()?);
                            if content.peek(Token![,]) {
                                let _ = content.parse::<Token![,]>()?;
                            }
                        }
                        state_abstract = Some(entries);
                    }
                    "output_abstract" => {
                        let mut arms = Vec::new();
                        while !content.is_empty() {
                            arms.push(content.parse()?);
                            if content.peek(Token![,]) {
                                let _ = content.parse::<Token![,]>()?;
                            }
                        }
                        output_abstract = Some(arms);
                    }
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            "unsupported nirvash_projection_model block",
                        ));
                    }
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
            abstract_state_ty: abstract_state_ty.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing abstract_state = ...")
            })?,
            expected_output_ty: expected_output_ty.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing expected_output = ...")
            })?,
            probe_state_domain,
            summary_output_domain,
            state_seed: state_seed
                .ok_or_else(|| syn::Error::new(Span::call_site(), "missing state_seed = ..."))?,
            state_summary: state_summary.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing state_summary { ... }")
            })?,
            output_summary: output_summary.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing output_summary { ... }")
            })?,
            state_abstract: state_abstract.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing state_abstract { ... }")
            })?,
            output_abstract: output_abstract.ok_or_else(|| {
                syn::Error::new(Span::call_site(), "missing output_abstract { ... }")
            })?,
            item: item.ok_or_else(|| {
                syn::Error::new(
                    Span::call_site(),
                    "missing impl ProtocolConformanceSpec for ... { ... }",
                )
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
        let mut probe_state_ty = None;
        let mut probe_output_ty = None;
        let mut observe_state = None;
        let mut observe_output = None;
        let mut dispatch_input = None;
        let mut input_codec = None;
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
                    "probe_state" => probe_state_ty = Some(input.parse()?),
                    "probe_output" => probe_output_ty = Some(input.parse()?),
                    "observe_state" => observe_state = Some(input.parse()?),
                    "observe_output" | "output" => observe_output = Some(input.parse()?),
                    "dispatch_input" => dispatch_input = Some(input.parse()?),
                    "input_codec" => input_codec = Some(input.parse()?),
                    "input" => input_ty = Some(input.parse()?),
                    "session" => session_ty = Some(input.parse()?),
                    "fresh_session" => fresh_session = Some(input.parse()?),
                    "probe_context" => probe_context = Some(input.parse()?),
                    "summary" | "summary_field" | "initial_summary" => {
                        return Err(syn::Error::new(
                            ident.span(),
                            "runtime-mode nirvash_runtime_contract no longer accepts summary/summary_field/initial_summary; use probe_state/probe_output/observe_state/observe_output",
                        ));
                    }
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
            probe_state_ty,
            probe_output_ty,
            observe_state,
            observe_output,
            dispatch_input,
            input_codec,
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
    call: Option<Expr>,
    positive: Option<Path>,
    negative: Option<Path>,
}

impl Parse for ContractCaseArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut action = None;
        let mut call = None;
        let mut positive = None;
        let mut negative = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let _eq: Token![=] = input.parse()?;
            match ident.to_string().as_str() {
                "action" => action = Some(input.parse()?),
                "call" => call = Some(input.parse()?),
                "positive" => positive = Some(input.parse()?),
                "negative" => negative = Some(input.parse()?),
                "requires" | "output" | "law_output" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "probe-first contract_case no longer accepts requires/output/law_output; use observe_state/observe_output on nirvash_runtime_contract",
                    ));
                }
                "update" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "probe-first contract_case no longer accepts update(...)",
                    ));
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "unsupported contract_case argument",
                    ));
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
            call,
            positive,
            negative,
        })
    }
}

fn parse_path_list(input: &syn::parse::ParseBuffer<'_>) -> syn::Result<Vec<Path>> {
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
            Self::Fairness => format_ident!("RegisteredExecutableFairness"),
            Self::StateConstraint => format_ident!("RegisteredStateConstraint"),
            Self::ActionConstraint => format_ident!("RegisteredActionConstraint"),
            Self::Symmetry => format_ident!("RegisteredSymmetry"),
        }
    }

    fn expected_type(self, spec: &Path) -> proc_macro2::TokenStream {
        match self {
            Self::Invariant => {
                quote! { ::nirvash::BoolExpr<<#spec as ::nirvash_lower::FrontendSpec>::State> }
            }
            Self::Property => {
                quote! { ::nirvash::Ltl<<#spec as ::nirvash_lower::FrontendSpec>::State, <#spec as ::nirvash_lower::FrontendSpec>::Action> }
            }
            Self::Fairness => {
                quote! { ::nirvash::Fairness<<#spec as ::nirvash_lower::FrontendSpec>::State, <#spec as ::nirvash_lower::FrontendSpec>::Action> }
            }
            Self::StateConstraint => {
                quote! { ::nirvash::BoolExpr<<#spec as ::nirvash_lower::FrontendSpec>::State> }
            }
            Self::ActionConstraint => {
                quote! { ::nirvash::StepExpr<<#spec as ::nirvash_lower::FrontendSpec>::State, <#spec as ::nirvash_lower::FrontendSpec>::Action> }
            }
            Self::Symmetry => {
                quote! { ::nirvash_lower::ReductionClaim<::nirvash_lower::SymmetryReduction<<#spec as ::nirvash_lower::FrontendSpec>::State>> }
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

    if let Some(message) = ast_native_registration_builder_error(kind, &item) {
        return Err(syn::Error::new(item.block.span(), message));
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
    let ast_native_message = match kind {
        RegistrationKind::Invariant | RegistrationKind::StateConstraint => {
            format!(
                "registered {label} `{{}}` for spec `{{}}` must be AST-native; use nirvash_expr! instead of BoolExpr::new(...)"
            )
        }
        RegistrationKind::ActionConstraint | RegistrationKind::Fairness => {
            format!(
                "registered {label} `{{}}` for spec `{{}}` must be AST-native; use nirvash_step_expr! instead of StepExpr::new(...)"
            )
        }
        RegistrationKind::Property => format!(
            "registered {label} `{{}}` for spec `{{}}` must be built from AST-native nirvash_expr!/nirvash_step_expr! nodes"
        ),
        RegistrationKind::Symmetry => String::new(),
    };
    let ast_native_message = LitStr::new(&ast_native_message, Span::call_site());
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
    let ast_native_assert = match kind {
        RegistrationKind::Invariant | RegistrationKind::StateConstraint => quote! {
            assert!(
                value.is_ast_native(),
                #ast_native_message,
                stringify!(#fn_ident),
                ::std::any::type_name::<#spec>(),
            );
        },
        RegistrationKind::ActionConstraint => quote! {
            assert!(
                value.is_ast_native(),
                #ast_native_message,
                stringify!(#fn_ident),
                ::std::any::type_name::<#spec>(),
            );
        },
        RegistrationKind::Property => quote! {
            assert!(
                value.is_ast_native(),
                #ast_native_message,
                stringify!(#fn_ident),
                ::std::any::type_name::<#spec>(),
            );
        },
        RegistrationKind::Fairness => quote! {
            assert!(
                value.is_ast_native(),
                #ast_native_message,
                stringify!(#fn_ident),
                ::std::any::type_name::<#spec>(),
            );
        },
        RegistrationKind::Symmetry => quote! {},
    };

    let registry_submit = match kind {
        RegistrationKind::Fairness => quote! {
            ::nirvash::inventory::submit! {
                ::nirvash::registry::RegisteredCoreFairness {
                    spec_type_id: #spec_id_ident,
                    name: stringify!(#fn_ident),
                    build: #build_ident,
                }
            }

            ::nirvash::inventory::submit! {
                ::nirvash::registry::RegisteredExecutableFairness {
                    spec_type_id: #spec_id_ident,
                    name: stringify!(#fn_ident),
                    build: #build_ident,
                }
            }
        },
        _ => quote! {
            ::nirvash::inventory::submit! {
                ::nirvash::registry::#registry_ident {
                    spec_type_id: #spec_id_ident,
                    name: stringify!(#fn_ident),
                    #case_labels_field
                    build: #build_ident,
                }
            }
        },
    };

    Ok(quote! {
        #item

        #[doc(hidden)]
        fn #build_ident() -> ::std::boxed::Box<dyn ::std::any::Any> {
            let value = #fn_ident();
            #ast_native_assert
            ::std::boxed::Box::new(value)
        }

        #[doc(hidden)]
        fn #spec_id_ident() -> ::std::any::TypeId {
            ::std::any::TypeId::of::<#spec>()
        }

        #[doc(hidden)]
        const _: fn() -> #expected = #fn_ident;

        #case_labels_tokens

        #registry_submit
    })
}

fn ast_native_registration_builder_error(
    kind: RegistrationKind,
    item: &ItemFn,
) -> Option<&'static str> {
    let body = item.block.to_token_stream().to_string();
    let contains = |needle: &str| body.contains(needle);
    let uses_bool_new = contains("BoolExpr :: new") || contains("BoolExpr::new");
    let uses_step_new = contains("StepExpr :: new") || contains("StepExpr::new");
    match kind {
        RegistrationKind::Invariant | RegistrationKind::StateConstraint => uses_bool_new.then_some(
            "registered state predicates must be AST-native; use nirvash_expr! instead of BoolExpr::new(...)",
        ),
        RegistrationKind::ActionConstraint | RegistrationKind::Fairness => uses_step_new.then_some(
            "registered step predicates must be AST-native; use nirvash_step_expr! instead of StepExpr::new(...)",
        ),
        RegistrationKind::Property => (uses_bool_new || uses_step_new).then_some(
            "registered properties must be built from AST-native nirvash_expr!/nirvash_step_expr! nodes",
        ),
        RegistrationKind::Symmetry => None,
    }
}

fn expand_finite_model_domain_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_finite_model_domain_tokens(input)
}

fn expand_symbolic_encoding_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_symbolic_encoding_tokens(input)
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

fn expand_finite_model_domain_tokens(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let args = SignatureArgs::from_attrs(&input.attrs)?;
    let ident = input.ident;
    let generics = input.generics;
    let trait_ident = finite_model_domain_trait_ident(&ident);
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
            "#[finite_model_domain(custom)] cannot be combined with bounds, filter, range, or #[finite_model_domain_invariant(...)] helpers",
        ));
    }

    let companion_trait = quote! {
        pub trait #trait_ident #trait_generics: Sized #trait_where_clause {
            fn finite_domain() -> ::nirvash_lower::BoundedDomain<Self>;

            fn value_invariant(&self) -> bool {
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
                fn finite_domain() -> ::nirvash_lower::BoundedDomain<Self> {
                    #domain_body
                }

                fn value_invariant(&self) -> bool {
                    #invariant_body
                }
            }
        }
    };

    Ok(quote! {
        #supported_data
        #companion_trait
        #auto_impl

        impl #impl_generics ::nirvash_lower::FiniteModelDomain for #ident #ty_generics #where_clause {
            fn finite_domain() -> ::nirvash::BoundedDomain<Self> {
                <Self as #trait_ident #ty_generics>::finite_domain()
            }

            fn value_invariant(&self) -> bool {
                <Self as #trait_ident #ty_generics>::value_invariant(self)
            }
        }

        #action_doc_registration
    })
}

fn expand_symbolic_encoding_tokens(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let custom = input.attrs.iter().any(|attr| {
        if !attr.path().is_ident("symbolic_encoding") {
            return false;
        }
        attr.parse_args::<Ident>()
            .map(|ident| ident == "custom")
            .unwrap_or(false)
    });
    signature_symbolic_state_registration(&input.ident, &input.data, &input.generics, custom)
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
            impl #impl_generics ::nirvash::ActionVocabulary for #ident #ty_generics #where_clause {
                fn action_vocabulary() -> ::std::vec::Vec<Self> {
                    <Self as ::nirvash_lower::FiniteModelDomain>::finite_domain().into_vec()
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
    let presentation_fn_ident = format_ident!("__nirvash_action_doc_presentation_{}", ident_snake);
    let type_id_item = guard_item(quote! {
        #[doc(hidden)]
        fn #type_id_fn_ident() -> ::std::any::TypeId {
            ::std::any::TypeId::of::<#ident>()
        }
    });
    let presentation_item = guard_item(quote! {
        #[doc(hidden)]
        fn #presentation_fn_ident(
            value: &dyn ::std::any::Any,
        ) -> ::std::option::Option<::nirvash::DocGraphActionPresentation> {
            let value = value
                .downcast_ref::<#ident>()
                .expect("registered action doc downcast");
            match value {
                #(#match_arms)*
            }
        }
    });
    let format_item = guard_item(quote! {
        #[doc(hidden)]
        fn #format_fn_ident(
            value: &dyn ::std::any::Any,
        ) -> ::std::option::Option<::std::string::String> {
            #presentation_fn_ident(value).map(|presentation| presentation.label)
        }
    });
    let inventory_item = guard_item(quote! {
        ::nirvash::inventory::submit! {
            ::nirvash::RegisteredActionDocLabel {
                value_type_id: #type_id_fn_ident,
                format: #format_fn_ident,
            }
        }
    });
    let presentation_inventory_item = guard_item(quote! {
        ::nirvash::inventory::submit! {
            ::nirvash::RegisteredActionDocPresentation {
                value_type_id: #type_id_fn_ident,
                format: #presentation_fn_ident,
            }
        }
    });

    Ok(quote! {
        #type_id_item
        #presentation_item
        #format_item
        #inventory_item
        #presentation_inventory_item
    })
}

fn signature_symbolic_state_registration(
    ident: &Ident,
    data: &Data,
    generics: &syn::Generics,
    custom: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    if custom || !signature_data_supports_symbolic_state_schema(data) {
        return Ok(quote! {});
    }

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let schema_body = signature_symbolic_state_schema_body(ident, data)?;
    let sort_body = signature_symbolic_sort_body(ident, data)?;
    let registration = if generics.params.is_empty() {
        let ident_snake = to_upper_snake(&ident.to_string()).to_lowercase();
        let type_id_fn_ident = format_ident!("__nirvash_symbolic_state_type_id_{}", ident_snake);
        let build_fn_ident = format_ident!("__nirvash_build_symbolic_state_schema_{}", ident_snake);
        quote! {
            #[doc(hidden)]
            fn #type_id_fn_ident() -> ::std::any::TypeId {
                ::std::any::TypeId::of::<#ident>()
            }

            #[doc(hidden)]
            fn #build_fn_ident() -> ::std::boxed::Box<dyn ::std::any::Any> {
                ::std::boxed::Box::new(
                    <#ident as ::nirvash_lower::SymbolicEncoding>::symbolic_state_schema()
                        .expect("symbolic state schema should be available")
                )
            }

            ::nirvash::inventory::submit! {
                ::nirvash::registry::RegisteredSymbolicStateSchema {
                    state_type_id: #type_id_fn_ident,
                    build: #build_fn_ident,
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl #impl_generics ::nirvash_lower::SymbolicEncoding for #ident #ty_generics #where_clause {
            fn symbolic_sort() -> ::nirvash_lower::SymbolicSort {
                #sort_body
            }

            fn symbolic_state_schema() -> ::core::option::Option<::nirvash_lower::SymbolicStateSchema<Self>> {
                ::core::option::Option::Some(#schema_body)
            }
        }

        #registration
    })
}

fn signature_data_supports_symbolic_state_schema(data: &Data) -> bool {
    match data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => fields
                .named
                .iter()
                .all(|field| type_supports_symbolic_state_schema(&field.ty)),
            Fields::Unnamed(fields) => fields
                .unnamed
                .iter()
                .all(|field| type_supports_symbolic_state_schema(&field.ty)),
            Fields::Unit => true,
        },
        Data::Enum(_) => true,
        Data::Union(_) => false,
    }
}

fn type_supports_symbolic_state_schema(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    let Some(segment) = type_path.path.segments.last() else {
        return false;
    };

    match &segment.arguments {
        syn::PathArguments::None => !matches!(
            segment.ident.to_string().as_str(),
            "u8" | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "f32"
                | "f64"
                | "char"
                | "str"
                | "String"
                | "Vec"
        ),
        syn::PathArguments::AngleBracketed(args) => match segment.ident.to_string().as_str() {
            "Option" | "RelSet" => args
                .args
                .iter()
                .filter_map(angle_bracketed_type_argument)
                .next()
                .is_some_and(type_supports_symbolic_state_schema),
            "Relation2" => {
                let mut args = args.args.iter().filter_map(angle_bracketed_type_argument);
                match (args.next(), args.next(), args.next()) {
                    (Some(lhs), Some(rhs), None) => {
                        type_supports_symbolic_state_schema(lhs)
                            && type_supports_symbolic_state_schema(rhs)
                    }
                    _ => false,
                }
            }
            _ => false,
        },
        syn::PathArguments::Parenthesized(_) => false,
    }
}

fn angle_bracketed_type_argument(arg: &syn::GenericArgument) -> Option<&Type> {
    match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    }
}

fn signature_symbolic_state_schema_body(
    ident: &Ident,
    data: &Data,
) -> syn::Result<proc_macro2::TokenStream> {
    match data {
        Data::Struct(data) => signature_struct_symbolic_state_schema_body(ident, data),
        Data::Enum(_) => Ok(quote! {
            ::nirvash::SymbolicStateSchema::new(
                vec![::nirvash::symbolic_leaf_field(
                    "self",
                    |state: &Self| state,
                    |state: &mut Self, value: Self| {
                        *state = value;
                    },
                )],
                || ::nirvash::symbolic_seed_value::<Self>(),
            )
        }),
        Data::Union(data) => Err(syn::Error::new(
            data.union_token.span(),
            "FiniteModelDomain derive does not support unions",
        )),
    }
}

fn signature_symbolic_sort_body(
    ident: &Ident,
    data: &Data,
) -> syn::Result<proc_macro2::TokenStream> {
    match data {
        Data::Enum(_) => Ok(quote! {
            ::nirvash::SymbolicSort::finite::<Self>()
        }),
        Data::Struct(data) => signature_struct_symbolic_sort_body(ident, data),
        Data::Union(data) => Err(syn::Error::new(
            data.union_token.span(),
            "FiniteModelDomain derive does not support unions",
        )),
    }
}

fn signature_struct_symbolic_sort_body(
    _ident: &Ident,
    data: &syn::DataStruct,
) -> syn::Result<proc_macro2::TokenStream> {
    match &data.fields {
        Fields::Named(fields) => {
            let field_sorts = fields.named.iter().map(|field| {
                let field_ident = field
                    .ident
                    .as_ref()
                    .expect("named struct fields should have identifiers");
                let field_ty = &field.ty;
                quote! {
                    ::nirvash::SymbolicSortField::new(
                        stringify!(#field_ident),
                        <#field_ty as ::nirvash_lower::SymbolicEncoding>::symbolic_sort(),
                    )
                }
            });
            Ok(quote! {
                ::nirvash::SymbolicSort::composite::<Self>(vec![#(#field_sorts),*])
            })
        }
        Fields::Unnamed(fields) => {
            let field_sorts = fields.unnamed.iter().enumerate().map(|(index, field)| {
                let field_index = LitStr::new(&index.to_string(), field.span());
                let field_ty = &field.ty;
                quote! {
                    ::nirvash::SymbolicSortField::new(
                        #field_index,
                        <#field_ty as ::nirvash_lower::SymbolicEncoding>::symbolic_sort(),
                    )
                }
            });
            Ok(quote! {
                ::nirvash::SymbolicSort::composite::<Self>(vec![#(#field_sorts),*])
            })
        }
        Fields::Unit => Ok(quote! {
            ::nirvash::SymbolicSort::composite::<Self>(vec![])
        }),
    }
}

fn signature_struct_symbolic_state_schema_body(
    ident: &Ident,
    data: &syn::DataStruct,
) -> syn::Result<proc_macro2::TokenStream> {
    match &data.fields {
        Fields::Named(fields) => {
            let mut registrations = Vec::new();
            let mut seed_fields = Vec::new();
            for field in &fields.named {
                let field_ident = field
                    .ident
                    .as_ref()
                    .expect("named struct fields should have identifiers");
                let field_ty = &field.ty;
                registrations.push(quote! {
                    __nirvash_fields.extend(::nirvash::symbolic_state_fields::<Self, #field_ty, _, _>(
                        stringify!(#field_ident),
                        |state: &Self| &state.#field_ident,
                        |state: &mut Self, value: #field_ty| {
                            state.#field_ident = value;
                        },
                    ));
                });
                seed_fields.push(quote! {
                    #field_ident: ::nirvash::symbolic_seed_value::<#field_ty>()
                });
            }
            Ok(quote! {
                {
                    let mut __nirvash_fields = ::std::vec::Vec::new();
                    #(#registrations)*
                    ::nirvash::SymbolicStateSchema::new(__nirvash_fields, || Self {
                        #(#seed_fields),*
                    })
                }
            })
        }
        Fields::Unnamed(fields) => {
            let mut registrations = Vec::new();
            let mut seed_fields = Vec::new();
            for (index, field) in fields.unnamed.iter().enumerate() {
                let field_index = syn::Index::from(index);
                let field_path = LitStr::new(&index.to_string(), field.span());
                let field_ty = &field.ty;
                registrations.push(quote! {
                    __nirvash_fields.extend(::nirvash::symbolic_state_fields::<Self, #field_ty, _, _>(
                        #field_path,
                        |state: &Self| &state.#field_index,
                        |state: &mut Self, value: #field_ty| {
                            state.#field_index = value;
                        },
                    ));
                });
                seed_fields.push(quote! {
                    ::nirvash::symbolic_seed_value::<#field_ty>()
                });
            }
            Ok(quote! {
                {
                    let mut __nirvash_fields = ::std::vec::Vec::new();
                    #(#registrations)*
                    ::nirvash::SymbolicStateSchema::new(
                        __nirvash_fields,
                        || #ident(#(#seed_fields),*),
                    )
                }
            })
        }
        Fields::Unit => Ok(quote! {
            ::nirvash::SymbolicStateSchema::new(
                vec![::nirvash::symbolic_leaf_field(
                    "self",
                    |state: &Self| state,
                    |state: &mut Self, value: Self| {
                        *state = value;
                    },
                )],
                || Self,
            )
        }),
    }
}

fn signature_action_doc_match_arm(
    enum_ident: &Ident,
    variant: &syn::Variant,
) -> syn::Result<proc_macro2::TokenStream> {
    let variant_ident = &variant.ident;
    let (compact_label, scenario_priority) = parse_viz_metadata(&variant.attrs)?;
    if let Some(summary) = first_doc_line(&variant.attrs) {
        let pattern = variant_ignore_pattern(enum_ident, variant_ident, &variant.fields);
        let compact_label = compact_label
            .as_ref()
            .map(|label| {
                quote! {
                    presentation = presentation.with_compact_label(#label.to_owned());
                }
            })
            .unwrap_or_default();
        let scenario_priority = scenario_priority
            .map(|priority| {
                quote! {
                    presentation = presentation.with_scenario_priority(#priority);
                }
            })
            .unwrap_or_default();
        return Ok(quote! {
            #pattern => {
                let mut presentation =
                    ::nirvash::DocGraphActionPresentation::new(#summary.to_owned());
                #compact_label
                #scenario_priority
                ::std::option::Option::Some(presentation)
            },
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

fn parse_viz_metadata(attrs: &[Attribute]) -> syn::Result<(Option<LitStr>, Option<i32>)> {
    let mut compact_label = None;
    let mut scenario_priority = None;

    for attr in attrs {
        if !attr.path().is_ident("viz") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("compact_label") {
                compact_label = Some(meta.value()?.parse()?);
                return Ok(());
            }
            if meta.path.is_ident("scenario_priority") {
                let value: syn::LitInt = meta.value()?.parse()?;
                scenario_priority = Some(value.base10_parse()?);
                return Ok(());
            }
            Err(meta.error("unsupported viz option"))
        })?;
    }

    Ok((compact_label, scenario_priority))
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
                    ::nirvash::describe_doc_graph_action(#binding)
                ),
            })
        }
        Fields::Named(fields) if fields.named.len() == 1 => {
            let binding = format_ident!("__nirvash_inner");
            let field_ident = fields.named.first()?.ident.as_ref()?;
            Some(quote! {
                #enum_ident::#variant_ident { #field_ident: #binding } => ::std::option::Option::Some(
                    ::nirvash::describe_doc_graph_action(#binding)
                ),
            })
        }
        _ => None,
    }
}

#[derive(Default)]
struct ProtocolInputWitnessArgs {
    action_ty: Option<Type>,
    action_field: Option<Ident>,
}

impl ProtocolInputWitnessArgs {
    fn from_attrs(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut args = Self::default();
        for attr in attrs {
            if !attr.path().is_ident("protocol_input_witness") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("action") {
                    args.action_ty = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                if meta.path.is_ident("field") {
                    args.action_field = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                Err(meta.error("unsupported #[protocol_input_witness(...)] option"))
            })?;
        }
        Ok(args)
    }
}

fn expand_protocol_input_witness_derive(
    input: DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "ProtocolInputWitness derive does not support generics",
        ));
    }

    let args = ProtocolInputWitnessArgs::from_attrs(&input.attrs)?;
    let ident = input.ident;

    match input.data {
        Data::Struct(data) => expand_protocol_input_witness_struct(&ident, data, args),
        Data::Enum(data) => expand_protocol_input_witness_enum(&ident, data, args),
        Data::Union(data) => Err(syn::Error::new(
            data.union_token.span(),
            "ProtocolInputWitness derive does not support unions",
        )),
    }
}

fn expand_protocol_input_witness_struct(
    ident: &Ident,
    data: DataStruct,
    args: ProtocolInputWitnessArgs,
) -> syn::Result<proc_macro2::TokenStream> {
    match data.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            let action_ty = fields
                .unnamed
                .first()
                .map(|field| field.ty.clone())
                .ok_or_else(|| syn::Error::new(fields.span(), "missing newtype field"))?;
            Ok(quote! {
                impl ::nirvash_conformance::ProtocolInputWitnessCodec<#action_ty> for #ident {
                    fn canonical_positive(action: &#action_ty) -> Self {
                        Self(action.clone())
                    }
                }
            })
        }
        Fields::Named(fields) => {
            let action_ty = args.action_ty.ok_or_else(|| {
                syn::Error::new(
                    fields.span(),
                    "named struct ProtocolInputWitness derive requires #[protocol_input_witness(action = ...)]",
                )
            })?;
            let action_field = if let Some(action_field) = args.action_field {
                action_field
            } else {
                let matching_fields = fields
                    .named
                    .iter()
                    .filter(|field| {
                        field.ty.to_token_stream().to_string()
                            == action_ty.to_token_stream().to_string()
                    })
                    .collect::<Vec<_>>();
                if matching_fields.len() == 1 {
                    matching_fields[0]
                        .ident
                        .clone()
                        .expect("named field should have ident")
                } else {
                    return Err(syn::Error::new(
                        fields.span(),
                        "named struct ProtocolInputWitness derive requires #[protocol_input_witness(field = ...)] when the action field is ambiguous",
                    ));
                }
            };
            Ok(quote! {
                impl ::nirvash_conformance::ProtocolInputWitnessCodec<#action_ty> for #ident {
                    fn canonical_positive(action: &#action_ty) -> Self {
                        let mut input = <Self as ::core::default::Default>::default();
                        input.#action_field = action.clone();
                        input
                    }
                }
            })
        }
        Fields::Unit => Err(syn::Error::new(
            ident.span(),
            "ProtocolInputWitness derive does not support unit structs",
        )),
        Fields::Unnamed(fields) => Err(syn::Error::new(
            fields.span(),
            "ProtocolInputWitness derive only supports newtype tuple structs",
        )),
    }
}

fn expand_protocol_input_witness_enum(
    ident: &Ident,
    data: DataEnum,
    args: ProtocolInputWitnessArgs,
) -> syn::Result<proc_macro2::TokenStream> {
    let action_ty = args.action_ty.ok_or_else(|| {
        syn::Error::new(
            ident.span(),
            "enum ProtocolInputWitness derive requires #[protocol_input_witness(action = ...)]",
        )
    })?;
    let arms = data
        .variants
        .iter()
        .map(|variant| {
            let variant_ident = &variant.ident;
            let constructor = match &variant.fields {
                Fields::Unit => quote! { Self::#variant_ident },
                Fields::Unnamed(fields) => {
                    let defaults = fields
                        .unnamed
                        .iter()
                        .map(|_| quote!(::core::default::Default::default()));
                    quote! { Self::#variant_ident(#(#defaults),*) }
                }
                Fields::Named(fields) => {
                    let defaults = fields.named.iter().map(|field| {
                        let field_ident =
                            field.ident.as_ref().expect("named field should have ident");
                        quote!(#field_ident: ::core::default::Default::default())
                    });
                    quote! { Self::#variant_ident { #(#defaults),* } }
                }
            };
            Ok(quote! {
                #action_ty::#variant_ident => #constructor,
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(quote! {
        impl ::nirvash_conformance::ProtocolInputWitnessCodec<#action_ty> for #ident {
            fn canonical_positive(action: &#action_ty) -> Self {
                match action {
                    #(#arms)*
                }
            }
        }
    })
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
        const _: fn() -> ::nirvash::BoundedDomain<#ident #ty_generics> =
            <#ident #ty_generics as ::nirvash_lower::FiniteModelDomain>::finite_domain;

        impl #impl_generics ::nirvash::RelAtom for #ident #ty_generics #where_clause {
            fn rel_index(&self) -> usize {
                <Self as ::nirvash_lower::FiniteModelDomain>::finite_domain()
                    .into_vec()
                    .into_iter()
                    .position(|candidate| candidate == self.clone())
                    .expect("RelAtom value must belong to FiniteModelDomain::finite_domain()")
            }

            fn rel_from_index(index: usize) -> ::std::option::Option<Self> {
                <Self as ::nirvash_lower::FiniteModelDomain>::finite_domain()
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
                <#field_ty as ::nirvash::RelationField>::relation_schema(#field_name)
            }
        })
        .collect::<Vec<_>>();
    let summary_entries = relation_fields
        .iter()
        .map(|(field_ident, field_name, field_ty)| {
            quote! {
                <#field_ty as ::nirvash::RelationField>::relation_summary(&self.#field_ident, #field_name)
            }
        })
        .collect::<Vec<_>>();
    let ident_snake = to_upper_snake(&ident.to_string()).to_lowercase();
    let schema_fn_ident = format_ident!("__nirvash_relational_schema_{}", ident_snake);
    let summary_fn_ident = format_ident!("__nirvash_relational_summary_{}", ident_snake);
    let type_id_fn_ident = format_ident!("__nirvash_relational_state_type_id_{}", ident_snake);
    let schema_item = guard_item(quote! {
        #[doc(hidden)]
        fn #schema_fn_ident() -> ::std::vec::Vec<::nirvash::RelationFieldSchema> {
            <#ident as ::nirvash::RelationalState>::relation_schema()
        }
    });
    let summary_item = guard_item(quote! {
        #[doc(hidden)]
        fn #summary_fn_ident(
            value: &dyn ::std::any::Any,
        ) -> ::std::vec::Vec<::nirvash::RelationFieldSummary> {
            <#ident as ::nirvash::RelationalState>::relation_summary(
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
        ::nirvash::inventory::submit! {
            ::nirvash::RegisteredRelationalState {
                state_type_id: #type_id_fn_ident,
                relation_schema: #schema_fn_ident,
                relation_summary: #summary_fn_ident,
            }
        }
    });

    Ok(quote! {
        impl ::nirvash::RelationalState for #ident {
            fn relation_schema() -> ::std::vec::Vec<::nirvash::RelationFieldSchema> {
                ::std::vec![#(#schema_entries),*]
            }

            fn relation_summary(&self) -> ::std::vec::Vec<::nirvash::RelationFieldSummary> {
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

fn finite_model_domain_trait_ident(ident: &Ident) -> Ident {
    format_ident!("{ident}FiniteModelDomainSpec")
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
            format!("FiniteModelDomain derive does not support unions for `{ident}`"),
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
            "#[finite_model_domain(bounds(...))] is only supported on named structs",
        ));
    }

    let domain = if let Some(range) = &args.range {
        let Data::Struct(data) = data else {
            return Err(syn::Error::new(
                ident.span(),
                "#[finite_model_domain(range = ...)] is only supported on structs",
            ));
        };
        if data.fields.len() != 1 {
            return Err(syn::Error::new(
                ident.span(),
                "#[finite_model_domain(range = ...)] requires a single-field newtype",
            ));
        }
        let iter = range_tokens(range)?;
        quote! {
            ::nirvash::BoundedDomain::new((#iter).map(Self).collect())
        }
    } else {
        match data {
            Data::Enum(data) => enum_domain_body(data)?,
            Data::Struct(data) => struct_domain_body(data, &args.bounds)?,
            Data::Union(data) => {
                return Err(syn::Error::new(
                    data.union_token.span(),
                    "FiniteModelDomain derive does not support unions",
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
                "#[finite_model_domain(range = ...)] is only supported on structs",
            ));
        };
        if data.fields.len() != 1 {
            return Err(syn::Error::new(
                ident.span(),
                "#[finite_model_domain(range = ...)] requires a single-field newtype",
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
                    "FiniteModelDomain derive does not support unions",
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
        ::nirvash::BoundedDomain::new(values)
    })
}

fn struct_domain_body(
    data: &DataStruct,
    type_level_bounds: &BTreeMap<String, FieldSigArgs>,
) -> syn::Result<proc_macro2::TokenStream> {
    if !type_level_bounds.is_empty() && !matches!(data.fields, Fields::Named(_)) {
        return Err(syn::Error::new(
            data.fields.span(),
            "#[finite_model_domain(bounds(...))] is only supported on named structs",
        ));
    }

    match &data.fields {
        Fields::Unit => Ok(quote! { ::nirvash::BoundedDomain::singleton(Self) }),
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
                ::nirvash::BoundedDomain::new(values)
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
                ::nirvash::BoundedDomain::new(values)
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
            "#[finite_model(optional)] is only supported on Option<T> fields",
        ));
    }

    if let Some(domain) = args.domain {
        return Ok(quote! { ::nirvash::into_bounded_domain(#domain()) });
    }

    if let Some(len) = args.len {
        let Some(element_ty) = vec_inner_type(&field.ty) else {
            return Err(syn::Error::new(
                field.ty.span(),
                "#[finite_model(len = ...)] is only supported on Vec<T> fields",
            ));
        };
        let iter = range_tokens(&len)?;
        return Ok(quote! {{
            let mut __nirvash_values = Vec::new();
            for __nirvash_len in #iter {
                __nirvash_values.extend(
                    ::nirvash::bounded_vec_domain::<#element_ty>(
                        __nirvash_len as usize,
                        __nirvash_len as usize,
                    )
                    .into_vec(),
                );
            }
            ::nirvash::BoundedDomain::new(__nirvash_values)
        }});
    }

    if let Some(range) = args.range {
        let iter = range_tokens(&range)?;
        return Ok(quote! { ::nirvash::BoundedDomain::new((#iter).collect()) });
    }

    let ty = &field.ty;
    Ok(quote! { <#ty as ::nirvash_lower::FiniteModelDomain>::finite_domain() })
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
                "#[finite_model(len = ...)] is only supported on Vec<T> fields",
            ));
        };
        return Ok(quote! {
            (#len).contains(&#access.len())
                && #access.iter().all(<#element_ty as ::nirvash_lower::FiniteModelDomain>::value_invariant)
        });
    }

    if args.optional && option_inner_type(&field.ty).is_none() {
        return Err(syn::Error::new(
            field.ty.span(),
            "#[finite_model(optional)] is only supported on Option<T> fields",
        ));
    }

    let ty = &field.ty;
    Ok(quote! { <#ty as ::nirvash_lower::FiniteModelDomain>::value_invariant(&#access) })
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
    mut item: ItemImpl,
    emit_composition: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    let self_ty = (*item.self_ty).clone();
    let spec_path = match &self_ty {
        Type::Path(type_path) if type_path.qself.is_none() => type_path.path.clone(),
        _ => {
            return Err(syn::Error::new(
                self_ty.span(),
                "#[system_spec]/#[subsystem_spec] requires a simple path type",
            ));
        }
    };
    let spec_tail = path_tail_ident(&spec_path)?.clone();
    let spec_name = LitStr::new(&spec_tail.to_string(), spec_tail.span());
    let spec_viz_provider_ident = format_ident!("__NirvashSpecVizMetadataProvider{}", spec_tail);
    let spec_viz_provider_build_ident = format_ident!(
        "__nirvash_spec_viz_metadata_provider_build_{}",
        spec_tail.to_string().to_lowercase()
    );
    let doc_attrs = doc_fragment_attrs(&self_ty)?;
    let state_ty = associated_type(&item, "State")?;
    let action_ty = associated_type(&item, "Action")?;
    if let Some(transition_fn) = impl_method(&item, "transition") {
        return Err(syn::Error::new(
            transition_fn.sig.ident.span(),
            "#[system_spec]/#[subsystem_spec] does not allow user-defined fn transition(...); implement transition_program() instead",
        ));
    }
    let Some(transition_program_fn) = impl_method(&item, "transition_program") else {
        return Err(syn::Error::new(
            self_ty.span(),
            "#[system_spec]/#[subsystem_spec] requires fn transition_program(&self) -> Option<TransitionProgram<...>>",
        ));
    };
    if let Some(message) = ast_native_transition_program_builder_error(transition_program_fn) {
        return Err(syn::Error::new(transition_program_fn.block.span(), message));
    }

    let model_cases = args.model_cases;
    let subsystems = args.subsystems;
    let model_cases_name_expr = if let Some(model_cases) = &model_cases {
        quote! { ::std::option::Option::Some(::core::stringify!(#model_cases)) }
    } else {
        quote! { ::std::option::Option::None }
    };
    if let Some(model_cases) = &model_cases {
        item.items.push(syn::parse_quote! {
            fn model_instances(&self) -> Vec<::nirvash_lower::ModelInstance<Self::State, Self::Action>> {
                let mut model_instances = #model_cases();
                ::nirvash_lower::registry::apply_registered_model_case_metadata_for::<Self, Self::State, Self::Action>(&mut model_instances);
                model_instances
            }
        });
    }

    if !emit_composition && !subsystems.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "subsystems(...) is only supported on #[system_spec]",
        ));
    }

    let composition_impl = if emit_composition {
        let subsystem_calls = subsystems.iter().map(|path| {
            let spec_id = path_to_string_syn(path).expect("subsystem path should stringify");
            let label = path_tail_ident(path)
                .expect("subsystem path should have a tail ident")
                .to_string();
            let spec_id = LitStr::new(&spec_id, path.span());
            let label = LitStr::new(&label, path.span());
            quote! {
                composition = composition.with_subsystem(::nirvash::RegisteredSubsystemSpec::new(
                    #spec_id,
                    #label,
                ));
            }
        });
        let subsystem_values = subsystems.iter().map(|path| {
            let spec_id = path_to_string_syn(path).expect("subsystem path should stringify");
            let label = path_tail_ident(path)
                .expect("subsystem path should have a tail ident")
                .to_string();
            let spec_id = LitStr::new(&spec_id, path.span());
            let label = LitStr::new(&label, path.span());
            quote! { ::nirvash::RegisteredSubsystemSpec::new(#spec_id, #label) }
        });
        quote! {
            impl #self_ty {
                pub const fn spec_kind() -> ::nirvash::SpecVizKind {
                    ::nirvash::SpecVizKind::System
                }

                pub const fn model_cases_name() -> ::std::option::Option<&'static str> {
                    #model_cases_name_expr
                }

                pub const REGISTERED_SUBSYSTEMS: &'static [::nirvash::RegisteredSubsystemSpec] =
                    &[#(#subsystem_values),*];

                pub const fn registered_subsystems() -> &'static [::nirvash::RegisteredSubsystemSpec] {
                    Self::REGISTERED_SUBSYSTEMS
                }

                pub fn composition(&self) -> ::nirvash_lower::SystemComposition<#state_ty, #action_ty> {
                    let mut composition = ::nirvash_lower::SystemComposition::new(self.frontend_name());
                    #(#subsystem_calls)*
                    for invariant in <#self_ty as ::nirvash_lower::TemporalSpec>::invariants(self) {
                        composition = composition.with_invariant(invariant);
                    }
                    for property in <#self_ty as ::nirvash_lower::TemporalSpec>::properties(self) {
                        composition = composition.with_property(property);
                    }
                    for fairness in <#self_ty as ::nirvash_lower::TemporalSpec>::core_fairness(self) {
                        composition = composition.with_core_fairness(fairness);
                    }
                    for model_case in <#self_ty as ::nirvash_lower::FrontendSpec>::model_instances(self) {
                        composition = composition.with_model_instance(model_case);
                    }
                    composition
                }
            }
        }
    } else {
        quote! {
            impl #self_ty {
                pub const fn spec_kind() -> ::nirvash::SpecVizKind {
                    ::nirvash::SpecVizKind::Subsystem
                }

                pub const fn model_cases_name() -> ::std::option::Option<&'static str> {
                    #model_cases_name_expr
                }

                pub const REGISTERED_SUBSYSTEMS: &'static [::nirvash::RegisteredSubsystemSpec] = &[];

                pub const fn registered_subsystems() -> &'static [::nirvash::RegisteredSubsystemSpec] {
                    Self::REGISTERED_SUBSYSTEMS
                }
            }
        }
    };

    Ok(quote! {
        #(#doc_attrs)*
        #item

        impl ::nirvash_lower::TemporalSpec for #self_ty {
            fn invariants(&self) -> Vec<::nirvash::BoolExpr<Self::State>> {
                ::nirvash_lower::registry::collect_invariants_for::<Self, Self::State>()
            }

            fn properties(&self) -> Vec<::nirvash::Ltl<Self::State, Self::Action>> {
                ::nirvash_lower::registry::collect_properties_for::<Self, Self::State, Self::Action>()
            }

            fn core_fairness(&self) -> Vec<::nirvash_lower::FairnessDecl> {
                ::nirvash_lower::registry::collect_core_fairness_for::<Self, Self::State, Self::Action>()
            }

            fn executable_fairness(&self) -> Vec<::nirvash::Fairness<Self::State, Self::Action>> {
                ::nirvash_lower::registry::collect_executable_fairness_for::<Self, Self::State, Self::Action>()
            }
        }

        #[doc(hidden)]
        struct #spec_viz_provider_ident;

        impl ::nirvash::SpecVizProvider for #spec_viz_provider_ident {
            fn spec_name(&self) -> &'static str {
                #spec_name
            }

            fn bundle(&self) -> ::nirvash::SpecVizBundle {
                let metadata = ::nirvash::SpecVizMetadata {
                    spec_id: ::core::stringify!(#self_ty).to_owned(),
                    kind: ::std::option::Option::Some(<#self_ty>::spec_kind()),
                    state_ty: ::std::any::type_name::<#state_ty>().to_owned(),
                    action_ty: ::std::any::type_name::<#action_ty>().to_owned(),
                    model_cases: <#self_ty>::model_cases_name().map(|name| name.to_owned()),
                    subsystems: <#self_ty>::registered_subsystems()
                        .iter()
                        .map(|subsystem| ::nirvash::SpecVizSubsystem::from_registered(*subsystem))
                        .collect::<::std::vec::Vec<_>>(),
                    registrations: ::nirvash_lower::registry::collect_spec_viz_registrations_for::<#self_ty, #state_ty, #action_ty>(),
                    policy: ::nirvash::VizPolicy::default(),
                };
                ::nirvash::SpecVizBundle::from_doc_graph_spec(
                    #spec_name,
                    metadata,
                    ::std::vec::Vec::new(),
                )
            }
        }

        #[doc(hidden)]
        fn #spec_viz_provider_build_ident() -> ::std::boxed::Box<dyn ::nirvash::SpecVizProvider> {
            ::std::boxed::Box::new(#spec_viz_provider_ident)
        }

        ::nirvash::inventory::submit! {
            ::nirvash::RegisteredSpecVizProvider {
                spec_name: #spec_name,
                build: #spec_viz_provider_build_ident,
                kind: ::nirvash::SpecVizProviderKind::MetadataOnly,
            }
        }

        #composition_impl
    })
}

fn ast_native_transition_program_builder_error(item: &ImplItemFn) -> Option<&'static str> {
    let body = item.block.to_token_stream().to_string();
    let contains = |needle: &str| body.contains(needle);
    let uses_transition_rule_new =
        contains("TransitionRule :: new") || contains("TransitionRule::new");
    let uses_update_program_new =
        contains("UpdateProgram :: new") || contains("UpdateProgram::new");
    let uses_guard_expr_new = contains("GuardExpr :: new") || contains("GuardExpr::new");
    (uses_transition_rule_new || uses_update_program_new || uses_guard_expr_new).then_some(
        "#[system_spec]/#[subsystem_spec] transition_program() must be AST-native; use nirvash_transition_program! instead of TransitionRule::new/UpdateProgram::new",
    )
}

fn impl_method<'a>(item: &'a ItemImpl, name: &str) -> Option<&'a syn::ImplItemFn> {
    item.items.iter().find_map(|impl_item| match impl_item {
        ImplItem::Fn(method) if method.sig.ident == name => Some(method),
        _ => None,
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
    let spec_viz_provider_ident = format_ident!("__NirvashSpecVizProvider{}", spec_tail);
    let spec_viz_provider_build_ident = format_ident!(
        "__nirvash_spec_viz_provider_build_{}",
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
                    let expected_invariants = <#spec_ty as ::nirvash_lower::TemporalSpec>::invariants(&spec)
                        .into_iter()
                        .map(|predicate| predicate.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_properties = <#spec_ty as ::nirvash_lower::TemporalSpec>::properties(&spec)
                        .into_iter()
                        .map(|property| property.describe())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_fairness = <#spec_ty as ::nirvash_lower::TemporalSpec>::core_fairness(&spec)
                        .into_iter()
                        .map(|fairness| fairness.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_model_instances = <#spec_ty as ::nirvash_lower::FrontendSpec>::model_instances(&spec);

                    assert_eq!(composition.subsystems(), <#spec_ty>::registered_subsystems());
                    assert_eq!(composition.invariants().iter().map(|predicate| predicate.name()).collect::<::std::vec::Vec<_>>(), expected_invariants);
                    assert_eq!(composition.properties().iter().map(|property| property.describe()).collect::<::std::vec::Vec<_>>(), expected_properties);
                    assert_eq!(composition.core_fairness().iter().map(|fairness| fairness.name()).collect::<::std::vec::Vec<_>>(), expected_fairness);
                    assert_eq!(composition.model_instances().len(), expected_model_instances.len());
                    for (actual, expected) in composition.model_instances().iter().zip(expected_model_instances.iter()) {
                        assert_eq!(actual.label(), expected.label());
                        assert_eq!(
                            actual.state_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>(),
                            expected.state_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>()
                        );
                        assert_eq!(
                            actual.action_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>(),
                            expected.action_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>()
                        );
                        assert_eq!(
                            actual
                                .claimed_reduction()
                                .and_then(|reduction| reduction.symmetry().map(|claim| claim.value()))
                                .map(|symmetry| symmetry.name()),
                            expected
                                .claimed_reduction()
                                .and_then(|reduction| reduction.symmetry().map(|claim| claim.value()))
                                .map(|symmetry| symmetry.name())
                        );
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

        impl ::nirvash::DocGraphProvider for #doc_provider_ident {
            fn spec_name(&self) -> &'static str {
                #spec_name
            }

            fn cases(&self) -> ::std::vec::Vec<::nirvash::DocGraphCase> {
                let specs = #cases_expr;
                let multiple_cases = specs.len() > 1;
                specs
                    .into_iter()
                    .enumerate()
                    .flat_map(|(index, spec)| {
                        let mut lowering_cx = ::nirvash_lower::LoweringCx;
                        let lowered =
                            <#spec_ty as ::nirvash_lower::FrontendSpec>::lower(&spec, &mut lowering_cx)
                                .expect("spec should lower for docs");
                        let model_cases = lowered.model_instances();
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
                                let resolved_model_case = model_case
                                    .clone()
                                    .with_resolved_backend(
                                        lowered
                                            .default_model_backend()
                                            .unwrap_or(::nirvash::ModelBackend::Explicit),
                                    );
                                let backend = resolved_model_case
                                    .doc_checker_config()
                                    .and_then(|config| config.backend)
                                    .unwrap_or_else(|| {
                                        resolved_model_case
                                            .effective_checker_config()
                                            .backend
                                            .unwrap_or(::nirvash::ModelBackend::Explicit)
                                    });
                                let snapshot = match backend {
                                    ::nirvash::ModelBackend::Explicit => {
                                        ::nirvash_check::ExplicitModelChecker::for_case(
                                            &lowered,
                                            resolved_model_case.clone(),
                                        )
                                        .reachable_graph_snapshot()
                                    }
                                    ::nirvash::ModelBackend::Symbolic => {
                                        ::nirvash_check::SymbolicModelChecker::for_case(
                                            &lowered,
                                            resolved_model_case.clone(),
                                        )
                                        .reachable_graph_snapshot()
                                    }
                                }
                                .expect("reachable graph snapshot should build for docs");
                                let states = snapshot.states;
                                let edges = snapshot
                                    .edges
                                    .iter()
                                    .map(|outgoing| {
                                        outgoing
                                            .iter()
                                            .map(|edge| {
                                                let presentation =
                                                    ::nirvash::describe_doc_graph_action(
                                                        &edge.action,
                                                    );
                                                ::nirvash::DocGraphEdge {
                                                    label: presentation.label,
                                                    compact_label: presentation.compact_label,
                                                    scenario_priority: presentation
                                                        .scenario_priority,
                                                    interaction_steps: presentation
                                                        .interaction_steps,
                                                    process_steps: presentation.process_steps,
                                                    target: edge.target,
                                                }
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
                                let doc_surface = model_case.doc_surface().map(::std::borrow::ToOwned::to_owned);
                                let doc_projection = model_case
                                    .doc_state_projection()
                                    .map(|projection| projection.label.to_owned());
                                ::nirvash::DocGraphCase {
                                    label,
                                    surface: doc_surface,
                                    projection: doc_projection,
                                    backend,
                                    trust_tier: snapshot.trust_tier,
                                    graph: ::nirvash::DocGraphSnapshot {
                                        states: states
                                            .into_iter()
                                            .map(|state| {
                                                model_case
                                                    .doc_state_projection()
                                                    .map(|projection| projection.summarize(&state))
                                                    .unwrap_or_else(|| ::nirvash::summarize_doc_graph_state(&state))
                                            })
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
        struct #spec_viz_provider_ident;

        impl ::nirvash::SpecVizProvider for #spec_viz_provider_ident {
            fn spec_name(&self) -> &'static str {
                #spec_name
            }

            fn bundle(&self) -> ::nirvash::SpecVizBundle {
                let doc_cases =
                    <#doc_provider_ident as ::nirvash::DocGraphProvider>::cases(&#doc_provider_ident);
                let metadata = ::nirvash::SpecVizMetadata {
                    spec_id: ::core::stringify!(#spec_ty).to_owned(),
                    kind: ::std::option::Option::Some(<#spec_ty>::spec_kind()),
                    state_ty: ::std::any::type_name::<<#spec_ty as ::nirvash_lower::FrontendSpec>::State>().to_owned(),
                    action_ty: ::std::any::type_name::<<#spec_ty as ::nirvash_lower::FrontendSpec>::Action>().to_owned(),
                    model_cases: <#spec_ty>::model_cases_name().map(|name| name.to_owned()),
                    subsystems: <#spec_ty>::registered_subsystems()
                        .iter()
                        .map(|subsystem| ::nirvash::SpecVizSubsystem::from_registered(*subsystem))
                        .collect::<::std::vec::Vec<_>>(),
                    registrations: ::nirvash_lower::registry::collect_spec_viz_registrations_for::<#spec_ty, <#spec_ty as ::nirvash_lower::FrontendSpec>::State, <#spec_ty as ::nirvash_lower::FrontendSpec>::Action>(),
                    policy: ::nirvash::VizPolicy::default(),
                };
                ::nirvash::SpecVizBundle::from_doc_graph_spec(#spec_name, metadata, doc_cases)
            }
        }

        #[doc(hidden)]
        fn #doc_provider_build_ident() -> ::std::boxed::Box<dyn ::nirvash::DocGraphProvider> {
            ::std::boxed::Box::new(#doc_provider_ident)
        }

        #[doc(hidden)]
        fn #spec_viz_provider_build_ident() -> ::std::boxed::Box<dyn ::nirvash::SpecVizProvider> {
            ::std::boxed::Box::new(#spec_viz_provider_ident)
        }

        #[doc(hidden)]
        pub fn #doc_provider_link_ident() {
            let _ = #doc_provider_build_ident as fn() -> ::std::boxed::Box<dyn ::nirvash::DocGraphProvider>;
            let _ = #spec_viz_provider_build_ident as fn() -> ::std::boxed::Box<dyn ::nirvash::SpecVizProvider>;
        }

        ::nirvash::inventory::submit! {
            ::nirvash::RegisteredDocGraphProvider {
                spec_name: #spec_name,
                build: #doc_provider_build_ident,
            }
        }

        ::nirvash::inventory::submit! {
            ::nirvash::RegisteredSpecVizProvider {
                spec_name: #spec_name,
                build: #spec_viz_provider_build_ident,
                kind: ::nirvash::SpecVizProviderKind::RuntimeGraph,
            }
        }

        #[cfg(test)]
        mod #module_ident {
            use super::*;

            type GeneratedState = <#spec_ty as ::nirvash_lower::FrontendSpec>::State;
            type GeneratedAction = <#spec_ty as ::nirvash_lower::FrontendSpec>::Action;
            type GeneratedModelCase = ::nirvash_lower::ModelInstance<GeneratedState, GeneratedAction>;
            const GENERATED_FORMAL_CHECK_ENV: &str = "NIRVASH_FORMAL_CHECK";
            const GENERATED_FORMAL_SPEC_INDEX_ENV: &str = "NIRVASH_FORMAL_SPEC_INDEX";
            const GENERATED_FORMAL_MODEL_CASE_INDEX_ENV: &str =
                "NIRVASH_FORMAL_MODEL_CASE_INDEX";

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            fn generated_lowered(
                spec: &#spec_ty,
            ) -> ::nirvash_lower::LoweredSpec<'_, GeneratedState, GeneratedAction> {
                let mut lowering_cx = ::nirvash_lower::LoweringCx;
                <#spec_ty as ::nirvash_lower::FrontendSpec>::lower(spec, &mut lowering_cx)
                    .expect("spec should lower")
            }

            fn generated_model_cases(spec: &#spec_ty) -> ::std::vec::Vec<GeneratedModelCase> {
                generated_lowered(spec).model_instances()
            }

            fn generated_explicit_fallback_model_case(
                model_case: GeneratedModelCase,
            ) -> GeneratedModelCase {
                let checker_config = model_case.effective_checker_config();
                let doc_checker_config = model_case
                    .doc_checker_config()
                    .unwrap_or_else(|| checker_config.clone());
                model_case
                    .with_checker_config(::nirvash::ModelCheckConfig {
                        backend: ::core::option::Option::Some(::nirvash::ModelBackend::Explicit),
                        ..checker_config.clone()
                    })
                    .with_doc_checker_config(::nirvash::ModelCheckConfig {
                        backend: ::core::option::Option::Some(::nirvash::ModelBackend::Explicit),
                        ..doc_checker_config
                    })
            }

            fn generated_snapshot(
                spec: &#spec_ty,
                model_case: GeneratedModelCase,
            ) -> ::nirvash::ReachableGraphSnapshot<GeneratedState, GeneratedAction> {
                let lowered = generated_lowered(spec);
                ::nirvash_check::ExplicitModelChecker::for_case(
                    &lowered,
                    generated_explicit_fallback_model_case(model_case),
                )
                .full_reachable_graph_snapshot()
                .expect("reachable graph snapshot should build with explicit fallback")
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
                    let lowered = generated_lowered(&spec);
                    let invariants = lowered.invariants();
                    for model_case in generated_model_cases(&spec) {
                        let initial_states = lowered.initial_states();
                        assert!(!initial_states.is_empty(), "spec should declare at least one initial state");
                        for state in initial_states {
                            assert!(lowered.contains_initial(&state));
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
                let lowered = generated_lowered(&spec);
                let resolved_model_case = model_case
                    .clone()
                    .with_resolved_backend(
                        lowered
                            .default_model_backend()
                            .unwrap_or(::nirvash::ModelBackend::Explicit),
                    );
                let backend = resolved_model_case
                    .effective_checker_config()
                    .backend
                    .unwrap_or(::nirvash::ModelBackend::Explicit);
                let result = match match backend {
                    ::nirvash::ModelBackend::Explicit => {
                        ::nirvash_check::ExplicitModelChecker::for_case(
                            &lowered,
                            resolved_model_case.clone(),
                        )
                        .check_all()
                    }
                    ::nirvash::ModelBackend::Symbolic => {
                        ::nirvash_check::SymbolicModelChecker::for_case(
                            &lowered,
                            resolved_model_case.clone(),
                        )
                        .check_all()
                    }
                } {
                    ::core::result::Result::Ok(result) => result,
                    ::core::result::Result::Err(::nirvash::ModelCheckError::UnsupportedConfiguration(_))
                        if backend == ::nirvash::ModelBackend::Symbolic =>
                    {
                        let lowered = generated_lowered(&spec);
                        ::nirvash_check::ExplicitModelChecker::for_case(
                            &lowered,
                            generated_explicit_fallback_model_case(model_case),
                        )
                        .check_all()
                        .expect("model checker should run with explicit fallback")
                    }
                    ::core::result::Result::Err(error) => {
                        panic!("model checker should run: {error:?}");
                    }
                };
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
                let invariants = <#spec_ty as ::nirvash_lower::TemporalSpec>::invariants(&spec);
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

            type GeneratedState = <#spec_ty as ::nirvash_lower::FrontendSpec>::State;
            type GeneratedAction = <#spec_ty as ::nirvash_lower::FrontendSpec>::Action;
            type GeneratedModelCase = ::nirvash_lower::ModelInstance<GeneratedState, GeneratedAction>;
            type GeneratedRuntime =
                <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::Runtime;
            type GeneratedContext =
                <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::Context;
            type GeneratedExpectedOutput =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ExpectedOutput;
            type GeneratedProbeState =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ProbeState;
            type GeneratedProbeOutput =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ProbeOutput;
            type GeneratedSummaryState =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState;
            type GeneratedSummaryOutput =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryOutput;

            #[derive(Clone, Debug, PartialEq, Eq)]
            struct GeneratedPrefixStep {
                action: GeneratedAction,
                next_state: GeneratedState,
            }

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            fn generated_lowered(
                spec: &#spec_ty,
            ) -> ::nirvash_lower::LoweredSpec<'_, GeneratedState, GeneratedAction> {
                let mut lowering_cx = ::nirvash_lower::LoweringCx;
                <#spec_ty as ::nirvash_lower::FrontendSpec>::lower(spec, &mut lowering_cx)
                    .expect("spec should lower")
            }

            fn generated_model_cases(spec: &#spec_ty) -> ::std::vec::Vec<GeneratedModelCase> {
                generated_lowered(spec).model_instances()
            }

            fn generated_paths(
                spec: &#spec_ty,
                model_case: GeneratedModelCase,
            ) -> (
                ::nirvash_conformance::ReachableGraphSnapshot<GeneratedState, GeneratedAction>,
                ::std::vec::Vec<::std::vec::Vec<GeneratedPrefixStep>>,
            ) {
                let lowered = generated_lowered(spec);
                let snapshot = ::nirvash_check::ExplicitModelChecker::for_case(&lowered, model_case)
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
                            next_path.push(GeneratedPrefixStep {
                                action: edge.action.clone(),
                                next_state: snapshot.states[edge.target].clone(),
                            });
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

            struct GeneratedReplayHistory {
                runtime: GeneratedRuntime,
                summary_states: ::std::vec::Vec<
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState,
                >,
                action_events: ::std::vec::Vec<(
                    GeneratedAction,
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryOutput,
                )>,
            }

            fn generated_initial_summary_is_valid(
                spec: &#spec_ty,
                summary: &<#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState,
            ) -> bool {
                let projected =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        summary,
                    );
                <#spec_ty as ::nirvash_lower::FrontendSpec>::contains_initial(spec, &projected)
            }

            fn generated_action_enabled(
                spec: &#spec_ty,
                summary: &<#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState,
                action: &GeneratedAction,
            ) -> bool {
                let projected =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        summary,
                    );
                !<#spec_ty as ::nirvash_lower::FrontendSpec>::transition_relation(spec, &projected, action)
                    .is_empty()
            }

            fn generated_refine_terminal_trace(
                spec: &#spec_ty,
                summary_states: ::std::vec::Vec<
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState,
                >,
                action_events: ::std::vec::Vec<(
                    GeneratedAction,
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryOutput,
                )>,
            ) -> ::std::result::Result<
                ::nirvash_conformance::TraceRefinementWitness<GeneratedState, GeneratedAction>,
                ::nirvash_conformance::TraceRefinementError<GeneratedState, GeneratedAction>,
            > {
                let observed = ::nirvash_conformance::ObservedTrace::terminal(summary_states, action_events);
                ::nirvash_conformance::constrained_trace_refines(
                    spec,
                    &::nirvash_conformance::ProtocolSummaryTraceMap(spec),
                    ::nirvash_lower::ModelInstance::new("generated_runtime"),
                    &observed,
                    ::nirvash_conformance::TraceRefinementConfig {
                        engine: ::nirvash_conformance::TraceRefinementEngine::ExplicitConstrained,
                        require_total_observation: false,
                        allow_lasso: false,
                        ..::nirvash_conformance::TraceRefinementConfig::default()
                    },
                )
            }

            async fn replay_prefix(
                spec: &#spec_ty,
                path: &[GeneratedPrefixStep],
                context: &GeneratedContext,
            ) -> GeneratedReplayHistory {
                let runtime =
                    <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::fresh_runtime(spec).await;
                let observed = <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                    &runtime,
                    context,
                )
                .await;
                let mut observed_summary =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed,
                    );
                let mut projected =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &observed_summary,
                    );
                let mut summary_states = ::std::vec::Vec::from([observed_summary.clone()]);
                let mut action_events = ::std::vec::Vec::new();
                assert!(
                    generated_initial_summary_is_valid(spec, &observed_summary),
                    "runtime initial state {:?} must be one of the declared initial states {:?}",
                    projected,
                    <#spec_ty as ::nirvash_lower::FrontendSpec>::initial_states(spec),
                );
                for prefix_step in path {
                    let action = &prefix_step.action;
                    let expected_next = &prefix_step.next_state;
                    let action_enabled = generated_action_enabled(spec, &observed_summary, action);
                    let output = <GeneratedRuntime as ::nirvash_conformance::ActionApplier>::execute_action(
                        &runtime,
                        context,
                        action,
                    )
                    .await;
                    let output_summary =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_output(
                            spec,
                            &output,
                        );
                    let projected_output =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_output(
                            spec,
                            &output_summary,
                        );
                    let observed_after =
                        <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                            &runtime,
                            context,
                        )
                        .await;
                    let observed_after_summary =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                            spec,
                            &observed_after,
                        );
                    let projected_after =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                            spec,
                            &observed_after_summary,
                        );
                    action_events.push((action.clone(), output_summary.clone()));
                    summary_states.push(observed_after_summary.clone());
                    if action_enabled {
                        let expected_output =
                            <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::expected_output(
                                spec,
                                &projected,
                                action,
                                ::core::option::Option::Some(expected_next),
                            );
                        assert_eq!(projected_output, expected_output);
                        assert_eq!(projected_after, *expected_next);
                        projected = projected_after;
                    } else {
                        action_events.pop();
                        summary_states.pop();
                        let expected_output =
                            <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::expected_output(
                                spec,
                                &projected,
                                action,
                                ::core::option::Option::None,
                            );
                        assert_eq!(projected_output, expected_output);
                        assert_eq!(projected_after, projected);
                    }
                    observed_summary = observed_after_summary;
                }
                GeneratedReplayHistory {
                    runtime,
                    summary_states,
                    action_events,
                }
            }

            async fn execute_from_state(
                spec: &#spec_ty,
                path: &[GeneratedPrefixStep],
                expected_state: &GeneratedState,
                action: &GeneratedAction,
                context: &GeneratedContext,
            ) -> (
                GeneratedState,
                ::core::option::Option<
                    ::nirvash_conformance::TraceStepRefinementWitness<GeneratedState, GeneratedAction>,
                >,
                GeneratedExpectedOutput,
                GeneratedState,
            ) {
                let replay = replay_prefix(spec, path, context).await;
                let GeneratedReplayHistory {
                    runtime,
                    mut summary_states,
                    mut action_events,
                } = replay;
                let observed_before =
                    <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                        &runtime,
                        context,
                    )
                .await;
                let observed_before_summary =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed_before,
                    );
                let projected_before =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &observed_before_summary,
                    );
                let action_enabled = generated_action_enabled(spec, &observed_before_summary, action);
                let output = <GeneratedRuntime as ::nirvash_conformance::ActionApplier>::execute_action(
                    &runtime,
                    context,
                    action,
                )
                .await;
                let output_summary =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_output(
                        spec,
                        &output,
                    );
                let projected_output =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_output(
                        spec,
                        &output_summary,
                    );
                let observed_after =
                    <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                        &runtime,
                        context,
                    )
                    .await;
                let observed_after_summary =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed_after,
                    );
                let projected_after =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &observed_after_summary,
                    );
                action_events.push((action.clone(), output_summary.clone()));
                summary_states.push(observed_after_summary.clone());
                let step_refinement =
                    generated_refine_terminal_trace(spec, summary_states, action_events);
                match (action_enabled, step_refinement) {
                        (true, Ok(witness)) => {
                        let action_witness = witness
                            .steps
                            .last()
                            .expect("step witness should exist")
                            .clone();
                        let expected_output =
                            <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::expected_output(
                                spec,
                                &projected_before,
                                action,
                                ::core::option::Option::Some(&action_witness.abstract_after),
                            );
                        assert_eq!(projected_output, expected_output);
                        (
                            projected_before,
                            ::core::option::Option::Some(action_witness),
                            projected_output,
                            projected_after,
                        )
                    }
                    (true, Err(error)) => {
                        panic!(
                            "state {:?} replayed as {:?} but step refinement failed for {:?}: {}",
                            expected_state,
                            projected_before,
                            action,
                            error,
                        );
                    }
                    (false, Ok(witness)) => {
                        panic!(
                            "state {:?} replayed as {:?} but action {:?} unexpectedly refined to {:?}",
                            expected_state,
                            projected_before,
                            action,
                            witness.abstract_after,
                        );
                    }
                    (false, Err(_)) => {
                        let expected_output =
                            <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::expected_output(
                                spec,
                                &projected_before,
                                action,
                                ::core::option::Option::None,
                            );
                        assert_eq!(projected_output, expected_output);
                        (
                            projected_before,
                            ::core::option::Option::None,
                            projected_output,
                            projected_after,
                        )
                    }
                }
            }

            #[test]
            fn generated_relation_sanity_matches_successor_projection() {
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
                        let (snapshot, _) = generated_paths(&spec, model_case);
                        for state in &snapshot.states {
                            for action in <#spec_ty as ::nirvash_lower::FrontendSpec>::actions(&spec) {
                                let next_states = <#spec_ty as ::nirvash_lower::FrontendSpec>::successors(
                                    &spec,
                                    state,
                                )
                                    .into_iter()
                                    .filter(|(candidate_action, _)| *candidate_action == action)
                                    .map(|(_, next)| next)
                                    .collect::<::std::vec::Vec<_>>();
                                let relation_next_states =
                                    <#spec_ty as ::nirvash_lower::FrontendSpec>::transition_relation(
                                        &spec,
                                        state,
                                        &action,
                                    );
                                assert_eq!(next_states, relation_next_states);
                                match <#spec_ty as ::nirvash_lower::FrontendSpec>::transition(
                                    &spec,
                                    state,
                                    &action,
                                ) {
                                    ::core::option::Option::Some(next) => {
                                        assert_eq!(next_states, vec![next]);
                                    }
                                    ::core::option::Option::None => {}
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
                        let context = <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_lower::FrontendSpec>::actions(&spec) {
                                let (_, expected_step, _, _) = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                        .await;
                                if expected_step.is_some() {
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
                        let context = <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_lower::FrontendSpec>::actions(&spec) {
                                let (observed_before, expected_step, _, observed_after) = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                        .await;
                                if expected_step.is_none() {
                                    assert_eq!(observed_after, observed_before);
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
                        let context = <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_lower::FrontendSpec>::actions(&spec) {
                                let (observed_before, expected_step, _, observed_after) = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                        .await;
                                match expected_step {
                                    ::core::option::Option::Some(witness) => {
                                        assert_eq!(observed_after, witness.abstract_after);
                                    }
                                    ::core::option::Option::None => {
                                        assert_eq!(observed_after, observed_before);
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
                        let context = <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::context(&spec);
                        let (snapshot, paths) = generated_paths(&spec, model_case);
                        for (index, state) in snapshot.states.iter().enumerate() {
                            for action in <#spec_ty as ::nirvash_lower::FrontendSpec>::actions(&spec) {
                                let _ = execute_from_state(
                                    &spec,
                                    &paths[index],
                                    state,
                                    &action,
                                    &context,
                                )
                                .await;
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

            type GeneratedState = <#spec_ty as ::nirvash_lower::FrontendSpec>::State;
            type GeneratedAction = <#spec_ty as ::nirvash_lower::FrontendSpec>::Action;
            type GeneratedModelCase = ::nirvash_lower::ModelInstance<GeneratedState, GeneratedAction>;
            type GeneratedRuntime =
                <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::Runtime;
            type GeneratedContext =
                <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::Context;
            type GeneratedInput =
                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::Input;
            type GeneratedSession =
                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::Session;
            type GeneratedExpectedOutput =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ExpectedOutput;
            type GeneratedProbeState =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ProbeState;
            type GeneratedProbeOutput =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ProbeOutput;
            type GeneratedSummaryState =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState;
            type GeneratedSummaryOutput =
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryOutput;
            type GeneratedPositiveWitness =
                ::nirvash_conformance::PositiveWitness<GeneratedContext, GeneratedInput>;
            type GeneratedNegativeWitness =
                ::nirvash_conformance::NegativeWitness<GeneratedContext, GeneratedInput>;

            #[derive(Clone)]
            struct GeneratedSemanticCase {
                spec_case_label: ::std::string::String,
                prefix_id: usize,
                provenance: ::std::vec::Vec<::std::string::String>,
                state: GeneratedState,
                action: GeneratedAction,
                expected_next: ::core::option::Option<GeneratedState>,
                path: ::std::vec::Vec<GeneratedPrefixStep>,
            }

            #[derive(Clone, Debug, PartialEq, Eq)]
            struct GeneratedPrefixStep {
                action: GeneratedAction,
                next_state: GeneratedState,
            }

            #[derive(Clone)]
            struct GeneratedWitnessDescriptor {
                index: usize,
                name: ::std::string::String,
            }

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            fn generated_lowered(
                spec: &#spec_ty,
            ) -> ::nirvash_lower::LoweredSpec<'_, GeneratedState, GeneratedAction> {
                let mut lowering_cx = ::nirvash_lower::LoweringCx;
                <#spec_ty as ::nirvash_lower::FrontendSpec>::lower(spec, &mut lowering_cx)
                    .expect("spec should lower")
            }

            fn generated_model_cases(spec: &#spec_ty) -> ::std::vec::Vec<GeneratedModelCase> {
                generated_lowered(spec).model_instances()
            }

            fn generated_paths(
                spec: &#spec_ty,
                model_case: GeneratedModelCase,
            ) -> (
                ::nirvash_conformance::ReachableGraphSnapshot<GeneratedState, GeneratedAction>,
                ::std::vec::Vec<::std::vec::Vec<GeneratedPrefixStep>>,
            ) {
                let lowered = generated_lowered(spec);
                let snapshot = ::nirvash_check::ExplicitModelChecker::for_case(&lowered, model_case)
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
                            next_path.push(GeneratedPrefixStep {
                                action: edge.action.clone(),
                                next_state: snapshot.states[edge.target].clone(),
                            });
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

            fn generated_prefix_component(path: &[GeneratedPrefixStep]) -> ::std::string::String {
                if path.is_empty() {
                    return "from_init".to_owned();
                }
                let actions = path
                    .iter()
                    .map(|step| generated_sanitize_test_component(&format!("{:?}", step.action)))
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
                        for action in <#spec_ty as ::nirvash_lower::FrontendSpec>::actions(spec) {
                            let successors =
                                <#spec_ty as ::nirvash_lower::FrontendSpec>::transition_relation(
                                    spec,
                                    state,
                                    &action,
                                );
                            if successors.is_empty() {
                                if let ::core::option::Option::Some(existing) = cases.iter_mut().find(|existing| {
                                    existing.state == *state
                                        && existing.action == action
                                        && existing.expected_next.is_none()
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
                                    expected_next: ::core::option::Option::None,
                                    path: paths[index].clone(),
                                });
                                next_prefix_id += 1;
                                continue;
                            }

                            for expected_next in successors {
                                if let ::core::option::Option::Some(existing) = cases.iter_mut().find(|existing| {
                                    existing.state == *state
                                        && existing.action == action
                                        && existing.expected_next == ::core::option::Option::Some(expected_next.clone())
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
                                    expected_next: ::core::option::Option::Some(expected_next),
                                    path: paths[index].clone(),
                                });
                                next_prefix_id += 1;
                            }
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
                    <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(session);
                let observed = <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                    runtime,
                    &context,
                )
                .await;
                let observed_summary =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &observed,
                    );
                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
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

            struct GeneratedObservedPrefix {
                summary_states: ::std::vec::Vec<
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState,
                >,
                action_events: ::std::vec::Vec<(
                    GeneratedAction,
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryOutput,
                )>,
            }

            fn generated_initial_summary_is_valid(
                spec: &#spec_ty,
                summary: &<#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState,
            ) -> bool {
                let projected =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        summary,
                    );
                <#spec_ty as ::nirvash_lower::FrontendSpec>::contains_initial(spec, &projected)
            }

            fn generated_refine_terminal_trace(
                spec: &#spec_ty,
                summary_states: ::std::vec::Vec<
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryState,
                >,
                action_events: ::std::vec::Vec<(
                    GeneratedAction,
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::SummaryOutput,
                )>,
            ) -> ::std::result::Result<
                ::nirvash_conformance::TraceRefinementWitness<GeneratedState, GeneratedAction>,
                ::nirvash_conformance::TraceRefinementError<GeneratedState, GeneratedAction>,
            > {
                let observed = ::nirvash_conformance::ObservedTrace::terminal(summary_states, action_events);
                ::nirvash_conformance::constrained_trace_refines(
                    spec,
                    &::nirvash_conformance::ProtocolSummaryTraceMap(spec),
                    ::nirvash_lower::ModelInstance::new("generated_canonical_prefix"),
                    &observed,
                    ::nirvash_conformance::TraceRefinementConfig {
                        engine: ::nirvash_conformance::TraceRefinementEngine::ExplicitConstrained,
                        require_total_observation: false,
                        allow_lasso: false,
                        ..::nirvash_conformance::TraceRefinementConfig::default()
                    },
                )
            }

            async fn generated_replay_canonical_prefix(
                spec: &#spec_ty,
                semantic_case: &GeneratedSemanticCase,
                runtime: &GeneratedRuntime,
                session: &mut GeneratedSession,
            ) -> ::std::result::Result<GeneratedObservedPrefix, ::std::string::String> {
                let initial_context =
                    <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(session);
                let initial_probe =
                    <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                        runtime,
                        &initial_context,
                    )
                    .await;
                let initial_summary =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                        spec,
                        &initial_probe,
                    );
                let initial_projected =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        spec,
                        &initial_summary,
                    );
                let mut current_summary = initial_summary;
                let initial_refinement =
                    ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                        generated_initial_summary_is_valid(spec, &current_summary)
                    }));
                let Ok(initial_matches) = initial_refinement else {
                    return Err(format!(
                        "{}{}",
                        generated_failure_prelude(semantic_case, "<initial-state>"),
                        "initial summary projection panicked",
                    ));
                };
                if !initial_matches {
                    return Err(format!(
                        "{}projected initial state {:?} is not declared as an initial state",
                        generated_failure_prelude(semantic_case, "<initial-state>"),
                        initial_projected,
                    ));
                }
                let mut projected = initial_projected;
                let mut summary_states = ::std::vec::Vec::from([current_summary.clone()]);
                let mut action_events = ::std::vec::Vec::new();
                for prefix_step in &semantic_case.path {
                    let action = &prefix_step.action;
                    let expected_next = &prefix_step.next_state;
                    let witnesses =
                        <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::positive_witnesses(
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
                        <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::execute_input(
                            runtime,
                            session,
                            witness.context(),
                            witness.input(),
                        )
                        .await;
                    let output_summary =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_output(
                            spec,
                            &output,
                        );
                    let projected_output =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_output(
                            spec,
                            &output_summary,
                        );
                    let expected_output =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::expected_output(
                            spec,
                            &projected,
                            action,
                            ::core::option::Option::Some(expected_next),
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
                    let observed_after =
                        {
                            let probe_after_context =
                                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(session);
                            <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                                runtime,
                                &probe_after_context,
                            )
                            .await
                        };
                    let observed_after_summary =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                            spec,
                            &observed_after,
                        );
                    let observed_after_projected =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                            spec,
                            &observed_after_summary,
                        );
                    action_events.push((action.clone(), output_summary.clone()));
                    summary_states.push(observed_after_summary.clone());
                    if observed_after_projected != *expected_next {
                        return Err(format!(
                            "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                            generated_failure_prelude(semantic_case, witness.name()),
                            expected_output,
                            projected_output,
                            expected_next,
                            observed_after_projected,
                        ));
                    }
                    current_summary = observed_after_summary;
                    projected = expected_next.clone();
                }
                Ok(GeneratedObservedPrefix {
                    summary_states,
                    action_events,
                })
            }

            fn generated_case_witnesses(
                spec: &#spec_ty,
                semantic_case: &GeneratedSemanticCase,
            ) -> ::std::result::Result<::std::vec::Vec<GeneratedWitnessDescriptor>, ::std::string::String> {
                generated_runtime().block_on(async {
                    let runtime =
                        <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::fresh_runtime(spec).await;
                    let mut session =
                        <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::fresh_session(spec).await;
                    let _replay =
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
                            <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::positive_witnesses(
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
                            <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::negative_witnesses(
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
                        <#binding_ty as ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty>>::fresh_runtime(spec.as_ref()).await;
                    let mut session =
                        <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::fresh_session(spec.as_ref()).await;
                    let replay =
                        generated_replay_canonical_prefix(spec.as_ref(), &semantic_case, &runtime, &mut session).await?;
                    let probe_before_context =
                        <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(&session);
                    let observed_before_probe =
                        <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                            &runtime,
                            &probe_before_context,
                        )
                        .await;
                    let observed_before_summary =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                            spec.as_ref(),
                            &observed_before_probe,
                        );
                    let observed_before =
                        <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                            spec.as_ref(),
                            &observed_before_summary,
                        );
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
                                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::positive_witnesses(
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
                                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::execute_input(
                                    &runtime,
                                    &mut session,
                                    witness.context(),
                                    witness.input(),
                                )
                                .await;
                            let output_summary =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_output(
                                    spec.as_ref(),
                                    &output,
                                );
                            let projected_output =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_output(
                                    spec.as_ref(),
                                    &output_summary,
                                );
                            let expected_output =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::expected_output(
                                    spec.as_ref(),
                                    &semantic_case.state,
                                    &semantic_case.action,
                                    ::core::option::Option::Some(next),
                                );
                            let probe_after_context =
                                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(&session);
                            let observed_after_probe =
                                <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                                    &runtime,
                                    &probe_after_context,
                                )
                                .await;
                            let observed_after_summary =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                                    spec.as_ref(),
                                    &observed_after_probe,
                                );
                            let observed_after =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                                    spec.as_ref(),
                                    &observed_after_summary,
                                );
                            let mut action_events = replay.action_events.clone();
                            let mut summary_states = replay.summary_states.clone();
                            action_events.push((semantic_case.action.clone(), output_summary.clone()));
                            summary_states.push(observed_after_summary.clone());
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
                            if replay.action_events.is_empty() {
                                let step_refinement =
                                    generated_refine_terminal_trace(
                                        spec.as_ref(),
                                        summary_states,
                                        action_events,
                                    )
                                    .map_err(|error| {
                                        format!(
                                            "{}step refinement failed: {}",
                                            generated_failure_prelude(&semantic_case, witness.name()),
                                            error,
                                        )
                                    })?;
                                let action_witness = step_refinement
                                    .steps
                                    .last()
                                    .expect("run-case action witness should exist");
                                if action_witness.abstract_after != *next || observed_after != *next {
                                    return Err(format!(
                                        "{}expected output: {:?}\nobserved output: {:?}\nexpected state: {:?}\nobserved state: {:?}",
                                        generated_failure_prelude(&semantic_case, witness.name()),
                                        expected_output,
                                        projected_output,
                                        next,
                                        observed_after,
                                    ));
                                }
                            } else if observed_after != *next {
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
                                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::negative_witnesses(
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
                                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::execute_input(
                                    &runtime,
                                    &mut session,
                                    witness.context(),
                                    witness.input(),
                                )
                                .await;
                            let output_summary =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_output(
                                    spec.as_ref(),
                                    &output,
                                );
                            let projected_output =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_output(
                                    spec.as_ref(),
                                    &output_summary,
                                );
                            let expected_output =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::expected_output(
                                    spec.as_ref(),
                                    &semantic_case.state,
                                    &semantic_case.action,
                                    ::core::option::Option::None,
                                );
                            let probe_after_context =
                                <#binding_ty as ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty>>::probe_context(&session);
                            let observed_after_probe =
                                <GeneratedRuntime as ::nirvash_conformance::StateObserver>::observe_state(
                                    &runtime,
                                    &probe_after_context,
                                )
                                .await;
                            let observed_after_summary =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                                    spec.as_ref(),
                                    &observed_after_probe,
                                );
                            let observed_after =
                                <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                                    spec.as_ref(),
                                    &observed_after_summary,
                                );
                            let mut action_events = replay.action_events.clone();
                            let mut summary_states = replay.summary_states.clone();
                            action_events.push((semantic_case.action.clone(), output_summary.clone()));
                            summary_states.push(observed_after_summary.clone());
                            if generated_refine_terminal_trace(
                                spec.as_ref(),
                                summary_states,
                                action_events,
                            )
                            .is_ok() {
                                return Err(format!(
                                    "{}negative witness unexpectedly refined to {:?}",
                                    generated_failure_prelude(&semantic_case, witness.name()),
                                    observed_after,
                                ));
                            }
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

            pub(super) fn generated_dynamic_tests() -> ::std::vec::Vec<::nirvash_conformance::DynamicTestCase> {
                let specs = generated_cases();
                let total = specs.len();
                let mut tests: ::std::vec::Vec<::nirvash_conformance::DynamicTestCase> =
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
                                    tests.push(::nirvash_conformance::DynamicTestCase::new(
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
                                tests.push(::nirvash_conformance::DynamicTestCase::new(
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
        fn #provider_build_ident() -> ::std::vec::Vec<::nirvash_conformance::DynamicTestCase> {
            #module_ident::generated_dynamic_tests()
        }

        #[cfg(test)]
        ::nirvash::inventory::submit! {
            ::nirvash_conformance::RegisteredCodeWitnessTestProvider {
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

fn expand_projection_model(args: ProjectionModelArgs) -> syn::Result<TokenStream2> {
    let ProjectionModelArgs {
        probe_state_ty,
        probe_output_ty,
        summary_state_ty,
        summary_output_ty,
        abstract_state_ty,
        expected_output_ty,
        probe_state_domain,
        summary_output_domain,
        state_seed,
        state_summary,
        output_summary,
        state_abstract,
        output_abstract,
        item,
    } = args;

    let self_ty = item.self_ty.as_ref().clone();
    let spec_ident = match &self_ty {
        Type::Path(type_path) if type_path.qself.is_none() => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.clone())
            .ok_or_else(|| {
                syn::Error::new(
                    type_path.span(),
                    "projection model requires a named self type",
                )
            })?,
        _ => {
            return Err(syn::Error::new(
                self_ty.span(),
                "projection model requires a named self type",
            ));
        }
    };
    let snake = to_upper_snake(&spec_ident.to_string()).to_lowercase();
    let summarize_state_ident =
        format_ident!("__nirvash_projection_model_{}_summarize_state", snake);
    let summarize_output_ident =
        format_ident!("__nirvash_projection_model_{}_summarize_output", snake);
    let abstract_state_ident = format_ident!("__nirvash_projection_model_{}_abstract_state", snake);
    let abstract_output_ident =
        format_ident!("__nirvash_projection_model_{}_abstract_output", snake);
    let tests_mod_ident = format_ident!("__nirvash_projection_model_{}_laws", snake);

    let state_summary_fields = state_summary
        .iter()
        .map(|entry| {
            let target = &entry.target;
            let value = &entry.value;
            quote! { #target: #value }
        })
        .collect::<Vec<_>>();
    let output_summary_fields = output_summary
        .iter()
        .map(|entry| {
            let target = &entry.target;
            let value = &entry.value;
            quote! { #target: #value }
        })
        .collect::<Vec<_>>();
    let state_abstract_assignments = state_abstract
        .iter()
        .map(|entry| {
            let target = &entry.target;
            let value = &entry.value;
            quote! { #target = #value; }
        })
        .collect::<Vec<_>>();
    let output_match_arms = output_abstract
        .iter()
        .map(|arm| {
            let pattern = &arm.pattern;
            match &arm.value {
                ProjectionModelOutputValue::Drop => quote! {
                    #pattern => {}
                },
                ProjectionModelOutputValue::Expr(expr) => quote! {
                    #pattern => {
                        output.push(#expr);
                    }
                },
            }
        })
        .collect::<Vec<_>>();

    let output_test_samples = output_abstract
        .iter()
        .map(|arm| {
            let sample_expr = projection_model_sample_expr(&arm.pattern)?;
            let pattern = &arm.pattern;
            let expected = match &arm.value {
                ProjectionModelOutputValue::Drop => quote! {
                    match &sample_effect {
                        #pattern => {}
                        _ => unreachable!("projection-model sample should match declared arm"),
                    }
                },
                ProjectionModelOutputValue::Expr(expr) => quote! {
                    match &sample_effect {
                        #pattern => expected_output.push(#expr),
                        _ => unreachable!("projection-model sample should match declared arm"),
                    }
                },
            };
            Ok(quote! {
                let sample_effect = #sample_expr;
                sample_effects.push(sample_effect.clone());
                #expected
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let state_projection_test = if let Some(domain) = probe_state_domain {
        quote! {
            #[test]
            #[allow(unused_assignments)]
            fn declared_state_projection_matches_model() {
                let spec = <#self_ty as ::core::default::Default>::default();
                ::nirvash_conformance::assert_projection_exhaustive(
                    "declared state projection",
                    (#domain)(),
                    |probe: &#probe_state_ty| {
                        let summary =
                            <#self_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                                &spec,
                                probe,
                            );
                        let projected =
                            <#self_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                                &spec,
                                &summary,
                            );
                        (summary, projected)
                    },
                    |probe: &#probe_state_ty| {
                        let probe = probe;
                        let summary = #summary_state_ty {
                            #(#state_summary_fields),*
                        };
                        let projected = {
                            let spec = &spec;
                            let summary = &summary;
                            let mut state: #abstract_state_ty = #state_seed;
                            #(#state_abstract_assignments)*
                            state
                        };
                        (summary, projected)
                    },
                );
            }
        }
    } else {
        quote! {
            #[test]
            #[allow(unused_assignments)]
            fn declared_state_projection_matches_model() {
                let spec = <#self_ty as ::core::default::Default>::default();
                let probe = <#probe_state_ty as ::core::default::Default>::default();
                let summary =
                    <#self_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                        &spec,
                        &probe,
                    );
                let expected_summary = {
                    let probe = &probe;
                    #summary_state_ty {
                        #(#state_summary_fields),*
                    }
                };
                let projected =
                    <#self_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        &spec,
                        &summary,
                    );
                let expected_state: #abstract_state_ty = {
                    let spec = &spec;
                    let summary = &summary;
                    let mut state: #abstract_state_ty = #state_seed;
                    #(#state_abstract_assignments)*
                    state
                };
                ::nirvash_conformance::assert_declared_state_projection(
                    &summary,
                    &expected_summary,
                    &projected,
                    &expected_state,
                );
            }
        }
    };
    let output_projection_test = if let Some(domain) = summary_output_domain {
        quote! {
            #[test]
            fn declared_output_projection_matches_model() {
                let spec = <#self_ty as ::core::default::Default>::default();
                ::nirvash_conformance::assert_projection_exhaustive(
                    "declared output projection",
                    (#domain)(),
                    |summary: &#summary_output_ty| {
                        <#self_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_output(
                            &spec,
                            summary,
                        )
                    },
                    |summary: &#summary_output_ty| {
                        let mut output: #expected_output_ty = ::core::default::Default::default();
                        for effect in &summary.effects {
                            match effect {
                                #(#output_match_arms,)*
                                _ => panic!("projection model encountered undeclared summary effect: {:?}", effect),
                            }
                        }
                        output
                    },
                );
            }
        }
    } else {
        quote! {
            #[test]
            fn declared_output_projection_matches_model() {
                let spec = <#self_ty as ::core::default::Default>::default();
                let mut sample_effects = ::std::vec::Vec::new();
                let mut expected_output: #expected_output_ty = ::core::default::Default::default();
                #(#output_test_samples)*
                let summary = #summary_output_ty {
                    effects: sample_effects,
                    ..<#summary_output_ty as ::core::default::Default>::default()
                };
                let projected =
                    <#self_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_output(
                        &spec,
                        &summary,
                    );
                ::nirvash_conformance::assert_declared_output_projection(
                    &projected,
                    &expected_output,
                );
            }
        }
    };

    let projection_tokens = expand_projection_contract(
        ProjectionContractArgs {
            probe_state_ty: probe_state_ty.clone(),
            probe_output_ty: probe_output_ty.clone(),
            summary_state_ty: summary_state_ty.clone(),
            summary_output_ty: summary_output_ty.clone(),
            summarize_state: syn::parse_quote!(#summarize_state_ident),
            summarize_output: syn::parse_quote!(#summarize_output_ident),
            abstract_state: syn::parse_quote!(#abstract_state_ident),
            abstract_output: syn::parse_quote!(#abstract_output_ident),
        },
        item,
    )?;

    Ok(quote! {
        #[doc(hidden)]
        fn #summarize_state_ident(probe: &#probe_state_ty) -> #summary_state_ty {
            #summary_state_ty {
                #(#state_summary_fields),*
            }
        }

        #[doc(hidden)]
        fn #summarize_output_ident(probe: &#probe_output_ty) -> #summary_output_ty {
            #summary_output_ty {
                #(#output_summary_fields),*
            }
        }

        #[doc(hidden)]
        #[allow(unused_assignments)]
        fn #abstract_state_ident(spec: &#self_ty, summary: &#summary_state_ty) -> #abstract_state_ty {
            let spec = spec;
            let mut state: #abstract_state_ty = #state_seed;
            #(#state_abstract_assignments)*
            state
        }

        #[doc(hidden)]
        fn #abstract_output_ident(
            _spec: &#self_ty,
            summary: &#summary_output_ty,
        ) -> #expected_output_ty {
            let mut output: #expected_output_ty = ::core::default::Default::default();
            for effect in &summary.effects {
                match effect {
                    #(#output_match_arms,)*
                    _ => panic!("projection model encountered undeclared summary effect: {:?}", effect),
                }
            }
            output
        }

        #projection_tokens

        #[cfg(test)]
        #[allow(clippy::needless_update)]
        mod #tests_mod_ident {
            use super::*;

            #state_projection_test

            #output_projection_test
        }
    })
}

fn projection_model_sample_expr(pattern: &Pat) -> syn::Result<Expr> {
    match pattern {
        Pat::Ident(ident) => {
            if let Some((_, subpat)) = &ident.subpat {
                projection_model_sample_expr(subpat)
            } else {
                Err(syn::Error::new(
                    ident.span(),
                    "nirvash_projection_model output_abstract identifier patterns require `name @ Variant(...)`",
                ))
            }
        }
        Pat::Path(path) => Ok(syn::parse_quote!(#path)),
        Pat::TupleStruct(tuple_struct) => {
            let path = &tuple_struct.path;
            let defaults = tuple_struct
                .elems
                .iter()
                .map(|_| quote!(::core::default::Default::default()));
            Ok(syn::parse_quote!(#path(#(#defaults),*)))
        }
        _ => Err(syn::Error::new(
            pattern.span(),
            "nirvash_projection_model output_abstract only supports tuple-variant and unit-variant patterns",
        )),
    }
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
    if args.input_codec.is_some() {
        return Err(syn::Error::new(
            Span::call_site(),
            "binding-mode nirvash_runtime_contract does not support input_codec = ...",
        ));
    }
    let input_ty: Type = args
        .input_ty
        .unwrap_or_else(|| syn::parse_quote!(<#spec_ty as ::nirvash_lower::FrontendSpec>::Action));
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
            impl ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty> for #self_ty {
                type Input = #input_ty;
                type Session = #session_ty;

                async fn fresh_session(_spec: &#spec_ty) -> Self::Session {
                    #fresh_session
                }

                fn positive_witnesses(
                    _spec: &#spec_ty,
                    session: &Self::Session,
                    _prev: &<#spec_ty as ::nirvash_lower::FrontendSpec>::State,
                    action: &<#spec_ty as ::nirvash_lower::FrontendSpec>::Action,
                    _next: &<#spec_ty as ::nirvash_lower::FrontendSpec>::State,
                ) -> ::std::vec::Vec<::nirvash_conformance::PositiveWitness<Self::Context, Self::Input>> {
                    vec![
                        ::nirvash_conformance::PositiveWitness::new(
                            "principal",
                            session.clone(),
                            action.clone(),
                        ).with_canonical(true)
                    ]
                }

                fn negative_witnesses(
                    _spec: &#spec_ty,
                    session: &Self::Session,
                    _prev: &<#spec_ty as ::nirvash_lower::FrontendSpec>::State,
                    action: &<#spec_ty as ::nirvash_lower::FrontendSpec>::Action,
                ) -> ::std::vec::Vec<::nirvash_conformance::NegativeWitness<Self::Context, Self::Input>> {
                    vec![
                        ::nirvash_conformance::NegativeWitness::new(
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
                ) -> <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ProbeOutput {
                    <Self::Runtime as ::nirvash_conformance::ActionApplier>::execute_action(
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

        impl ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty> for #self_ty {
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
    let probe_state_ty = args.probe_state_ty.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires probe_state = ...",
        )
    })?;
    let probe_output_ty = args.probe_output_ty.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires probe_output = ...",
        )
    })?;
    let observe_state = args.observe_state.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires observe_state = ...",
        )
    })?;
    let observe_output = args.observe_output.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "runtime-mode contract requires observe_output = ...",
        )
    })?;
    let context_ty = args.context_ty;
    let context_expr = args
        .context_expr
        .unwrap_or_else(|| syn::parse_quote!(::core::default::Default::default()));
    let fresh_runtime = args.fresh_runtime;
    let input_ty_is_explicit = args.input_ty.is_some();
    let input_ty: Type = args
        .input_ty
        .unwrap_or_else(|| syn::parse_quote!(<#spec_ty as ::nirvash_lower::FrontendSpec>::Action));
    let session_ty: Type = args.session_ty.unwrap_or_else(|| context_ty.clone());
    let fresh_session = args.fresh_session.unwrap_or_else(|| context_expr.clone());
    let probe_context = args
        .probe_context
        .unwrap_or_else(|| syn::parse_quote!(session.clone()));
    let dispatch_input = args.dispatch_input;
    let input_codec = args.input_codec;
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

    if witness_tokens.is_some() && input_ty_is_explicit && dispatch_input.is_none() {
        return Err(syn::Error::new(
            Span::call_site(),
            "runtime-mode witness contracts require dispatch_input = ... when input = ... is specified",
        ));
    }

    let execute_branches = cases.iter().map(|(method_ident, case, _returns_result)| {
        let action = &case.action;
        let call_expr = case
            .call
            .clone()
            .unwrap_or_else(|| syn::parse_quote!(self.#method_ident()));
        let output_expr = quote! { (#observe_output)(self, _context, action, &result) };
        quote! {
            if *action == #action {
                let result = #call_expr.await;
                return #output_expr;
            }
        }
    });

    let generated_positive_witness = if !input_ty_is_explicit {
        quote! {
            vec![
                ::nirvash_conformance::PositiveWitness::new(
                    "principal",
                    session.clone(),
                    action.clone(),
                )
                .with_canonical(true),
            ]
        }
    } else if let Some(input_codec) = &input_codec {
        let probe_context_expr = probe_context.clone();
        quote! {
            <#input_codec as ::nirvash_conformance::ProtocolInputWitnessCodec<
                <#spec_ty as ::nirvash_lower::FrontendSpec>::Action
            >>::positive_family(action)
                .into_iter()
                .enumerate()
                .map(|(index, input)| {
                    ::nirvash_conformance::PositiveWitness::new(
                        <#input_codec as ::nirvash_conformance::ProtocolInputWitnessCodec<
                            <#spec_ty as ::nirvash_lower::FrontendSpec>::Action
                        >>::witness_name(
                            action,
                            if index == 0 {
                                ::nirvash_conformance::WitnessKind::CanonicalPositive
                            } else {
                                ::nirvash_conformance::WitnessKind::Positive
                            },
                            index,
                        ),
                        #probe_context_expr,
                        input,
                    )
                    .with_canonical(index == 0)
                })
                .collect()
        }
    } else {
        quote! { ::std::vec::Vec::new() }
    };
    let generated_negative_witness = if !input_ty_is_explicit {
        quote! {
            vec![
                ::nirvash_conformance::NegativeWitness::new(
                    "principal",
                    session.clone(),
                    action.clone(),
                ),
            ]
        }
    } else if let Some(input_codec) = &input_codec {
        let probe_context_expr = probe_context.clone();
        quote! {
            <#input_codec as ::nirvash_conformance::ProtocolInputWitnessCodec<
                <#spec_ty as ::nirvash_lower::FrontendSpec>::Action
            >>::negative_family(action)
                .into_iter()
                .enumerate()
                .map(|(index, input)| {
                    ::nirvash_conformance::NegativeWitness::new(
                        <#input_codec as ::nirvash_conformance::ProtocolInputWitnessCodec<
                            <#spec_ty as ::nirvash_lower::FrontendSpec>::Action
                        >>::witness_name(
                            action,
                            ::nirvash_conformance::WitnessKind::Negative,
                            index,
                        ),
                        #probe_context_expr,
                        input,
                    )
                })
                .collect()
        }
    } else {
        quote! { ::std::vec::Vec::new() }
    };
    let positive_witness_branches = cases.iter().map(|(_, case, _)| {
        let action = &case.action;
        if let Some(builder) = &case.positive {
            quote! {
                if *action == #action {
                    return (#builder)(spec, session, prev, action, next);
                }
            }
        } else {
            quote! {
                if *action == #action {
                    return #generated_positive_witness;
                }
            }
        }
    });
    let negative_witness_branches = cases.iter().map(|(_, case, _)| {
        let action = &case.action;
        if let Some(builder) = &case.negative {
            quote! {
                if *action == #action {
                    return (#builder)(spec, session, prev, action);
                }
            }
        } else {
            quote! {
                if *action == #action {
                    return #generated_negative_witness;
                }
            }
        }
    });
    let execute_input_expr = if let Some(dispatch_input) = dispatch_input {
        quote! { (#dispatch_input)(runtime, session, context, input).await }
    } else {
        quote! {
            <#self_ty as ::nirvash_conformance::ActionApplier>::execute_action(
                runtime,
                context,
                input,
            )
            .await
        }
    };

    let binding_struct = if generate_binding_struct {
        quote! {
            #[derive(Debug, Default, Clone, Copy)]
            struct #binding_ident;
        }
    } else {
        quote! {}
    };

    let witness_impl = if witness_tokens.is_some() {
        quote! {
            impl ::nirvash_conformance::ProtocolInputWitnessBinding<#spec_ty> for #binding_ty {
                type Input = #input_ty;
                type Session = #session_ty;

                async fn fresh_session(_spec: &#spec_ty) -> Self::Session {
                    #fresh_session
                }

                fn positive_witnesses(
                    spec: &#spec_ty,
                    session: &Self::Session,
                    prev: &<#spec_ty as ::nirvash_lower::FrontendSpec>::State,
                    action: &<#spec_ty as ::nirvash_lower::FrontendSpec>::Action,
                    next: &<#spec_ty as ::nirvash_lower::FrontendSpec>::State,
                ) -> ::std::vec::Vec<::nirvash_conformance::PositiveWitness<Self::Context, Self::Input>> {
                    #(#positive_witness_branches)*
                    ::std::vec::Vec::new()
                }

                fn negative_witnesses(
                    spec: &#spec_ty,
                    session: &Self::Session,
                    prev: &<#spec_ty as ::nirvash_lower::FrontendSpec>::State,
                    action: &<#spec_ty as ::nirvash_lower::FrontendSpec>::Action,
                ) -> ::std::vec::Vec<::nirvash_conformance::NegativeWitness<Self::Context, Self::Input>> {
                    #(#negative_witness_branches)*
                    ::std::vec::Vec::new()
                }

                async fn execute_input(
                    runtime: &Self::Runtime,
                    session: &mut Self::Session,
                    context: &Self::Context,
                    input: &Self::Input,
                ) -> <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::ProbeOutput {
                    #execute_input_expr
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

        #binding_struct

        impl ::nirvash_conformance::ProtocolRuntimeBinding<#spec_ty> for #binding_ty {
            type Runtime = #self_ty;
            type Context = #context_ty;

            async fn fresh_runtime(spec: &#spec_ty) -> Self::Runtime {
                #fresh_runtime
            }

            fn context(_spec: &#spec_ty) -> Self::Context {
                #context_expr
            }
        }

        impl ::nirvash_conformance::ActionApplier for #self_ty {
            type Action = <#spec_ty as ::nirvash_lower::FrontendSpec>::Action;
            type Output = #probe_output_ty;
            type Context = #context_ty;

            async fn execute_action(
                &self,
                _context: &Self::Context,
                action: &Self::Action,
            ) -> Self::Output {
                let spec = <#spec_ty as ::core::default::Default>::default();
                let observed_before = (#observe_state)(self, _context).await;
                let summary_before =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::summarize_state(
                        &spec,
                        &observed_before,
                    );
                let projected_before =
                    <#spec_ty as ::nirvash_conformance::ProtocolConformanceSpec>::abstract_state(
                        &spec,
                        &summary_before,
                    );
                if <#spec_ty as ::nirvash_lower::FrontendSpec>::transition_relation(
                    &spec,
                    &projected_before,
                    action,
                )
                .is_empty() {
                    return <#probe_output_ty as ::core::default::Default>::default();
                }
                #(#execute_branches)*
                panic!("no contract_case registered for action {:?}", action);
            }
        }

        impl ::nirvash_conformance::StateObserver for #self_ty {
            type SummaryState = #probe_state_ty;
            type Context = #context_ty;

            async fn observe_state(&self, _context: &Self::Context) -> Self::SummaryState {
                (#observe_state)(self, _context).await
            }
        }

        #grouped_tokens
        #witness_impl
        #witness_tokens
    })
}

fn path_tail_ident(path: &Path) -> syn::Result<&Ident> {
    path.segments
        .last()
        .map(|segment| &segment.ident)
        .ok_or_else(|| syn::Error::new(path.span(), "path cannot be empty"))
}

fn path_to_string_syn(path: &Path) -> syn::Result<String> {
    for segment in &path.segments {
        if !matches!(segment.arguments, PathArguments::None) {
            return Err(syn::Error::new(
                segment.ident.span(),
                format!("unsupported path argument in `{}`", segment.ident),
            ));
        }
    }
    Ok(path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::"))
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
