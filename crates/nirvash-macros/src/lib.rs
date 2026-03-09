use std::collections::BTreeMap;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Expr, ExprRange, Field, Fields, Ident,
    ImplItem, ItemConst, ItemFn, ItemImpl, LitStr, Path, RangeLimits, Token, Type,
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

struct TargetSpecArg {
    spec: Path,
}

impl syn::parse::Parse for TargetSpecArg {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            spec: input.parse()?,
        })
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
    let args = parse_macro_input!(attr as TargetSpecArg);
    let item = parse_macro_input!(item as ItemFn);
    match expand_registration(args, item, kind) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_registration(
    args: TargetSpecArg,
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
    let spec = args.spec;
    let expected = kind.expected_type(&spec);
    let registry_ident = kind.registry_ident();
    let label = kind.label();
    let build_ident = format_ident!("__nirvash_{}_build_{}", label, fn_ident);
    let spec_id_ident = format_ident!("__nirvash_{}_spec_type_id_{}", label, fn_ident);

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

        ::nirvash_core::inventory::submit! {
            ::nirvash_core::registry::#registry_ident {
                spec_type_id: #spec_id_ident,
                name: stringify!(#fn_ident),
                build: #build_ident,
            }
        }
    })
}

fn expand_signature_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let args = SignatureArgs::from_attrs(&input.attrs)?;
    let ident = input.ident;
    let generics = input.generics;
    let trait_ident = companion_trait_ident(&ident);
    let trait_generics = trait_generics(&generics);
    let trait_where_clause = &generics.where_clause;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let supported_data = ensure_supported_signature_data(&ident, &input.data)?;

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
    })
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
                let state_constraints = ::nirvash_core::registry::collect_state_constraints::<Self>();
                let action_constraints = ::nirvash_core::registry::collect_action_constraints::<Self>();
                let symmetry = ::nirvash_core::registry::collect_symmetry::<Self>();
                for model_case in &mut model_cases {
                    let mut next_model_case = ::core::mem::take(model_case);
                    for constraint in &state_constraints {
                        next_model_case = next_model_case.with_state_constraint(*constraint);
                    }
                    for constraint in &action_constraints {
                        next_model_case = next_model_case.with_action_constraint(*constraint);
                    }
                    if next_model_case.symmetry().is_none() {
                        if let ::core::option::Option::Some(symmetry) = symmetry {
                            next_model_case = next_model_case.with_symmetry(symmetry);
                        }
                    }
                    *model_case = next_model_case;
                }
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
        "__nirvash_formal_tests_{}",
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
                                                label: format!("{:?}", edge.action),
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
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
                        let checker = ::nirvash_core::ModelChecker::for_case(&spec, model_case);
                        let result = checker.check_all().expect("model checker should run");
                        assert!(result.is_ok(), "{:?}", result.violations());
                    }
                }
            }

            #[test]
            fn generated_reachable_states_satisfy_registered_state_predicates() {
                for spec in generated_cases() {
                    let invariants = <#spec_ty as ::nirvash_core::TemporalSpec>::invariants(&spec);
                    for model_case in generated_model_cases(&spec) {
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
                }
            }

            #[test]
            fn generated_reachable_transitions_respect_constraints() {
                for spec in generated_cases() {
                    for model_case in generated_model_cases(&spec) {
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
            type GeneratedObservedState =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ObservedState;
            type GeneratedObservedOutput =
                <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::ObservedOutput;

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
                let mut projected =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::project_state(
                        spec,
                        &observed,
                    );
                let initial_states =
                    <#spec_ty as ::nirvash_core::conformance::TransitionSystem>::initial_states(spec);
                assert!(
                    initial_states.iter().any(|state| *state == projected),
                    "runtime initial state {:?} must be one of the declared initial states {:?}",
                    projected,
                    initial_states,
                );
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
                    let projected_output =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::project_output(
                            spec,
                            &output,
                        );
                    assert_eq!(projected_output, expected_output);
                    let observed_after =
                        <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                            &runtime,
                            context,
                        )
                        .await;
                    let projected_after =
                        <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::project_state(
                            spec,
                            &observed_after,
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
                let projected_before =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::project_state(
                        spec,
                        &observed_before,
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
                let projected_output =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::project_output(
                        spec,
                        &output,
                    );
                let observed_after =
                    <GeneratedRuntime as ::nirvash_core::conformance::StateObserver>::observe_state(
                        &runtime,
                        context,
                    )
                    .await;
                let projected_after =
                    <#spec_ty as ::nirvash_core::conformance::ProtocolConformanceSpec>::project_state(
                        spec,
                        &observed_after,
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
