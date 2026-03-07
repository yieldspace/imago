use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, ExprRange, Field, Fields, Ident, ImplItem,
    ItemConst, ItemFn, ItemImpl, LitStr, Path, RangeLimits, Type, parse_macro_input,
};

#[proc_macro_derive(Signature, attributes(signature))]
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
pub fn invariant(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::Invariant)
}

#[proc_macro_attribute]
pub fn illegal(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_registration_attr(attr, item, RegistrationKind::Illegal)
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
}

impl SignatureArgs {
    fn from_attrs(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut args = Self::default();
        for attr in attrs {
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
                Err(meta.error("unsupported #[signature(...)] argument"))
            })?;
        }
        Ok(args)
    }
}

struct SpecArgs {
    checker_config: Option<Path>,
    subsystems: Vec<LitStr>,
}

impl syn::parse::Parse for SpecArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self {
            checker_config: None,
            subsystems: Vec::new(),
        };

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let content;
            syn::parenthesized!(content in input);
            match ident.to_string().as_str() {
                "checker_config" => args.checker_config = Some(parse_single_path(&content)?),
                "subsystems" => args.subsystems = parse_string_list(&content)?,
                "invariants" | "illegal" | "state_constraints" | "action_constraints"
                | "properties" | "fairness" | "symmetry" => {
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
    init: Ident,
    cases: Option<Ident>,
    composition: Option<Ident>,
}

impl syn::parse::Parse for TestArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut spec = None;
        let mut init = None;
        let mut cases = None;
        let mut composition = None;
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let _eq: syn::Token![=] = input.parse()?;
            match ident.to_string().as_str() {
                "spec" => spec = Some(input.parse()?),
                "init" => init = Some(input.parse()?),
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
            init: init.ok_or_else(|| syn::Error::new(Span::call_site(), "missing init = ..."))?,
            cases,
            composition,
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
    Illegal,
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
            Self::Illegal => "illegal",
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
            Self::Illegal => format_ident!("RegisteredIllegal"),
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
            Self::Illegal => {
                quote! { ::nirvash_core::StepPredicate<<#spec as ::nirvash_core::TransitionSystem>::State, <#spec as ::nirvash_core::TransitionSystem>::Action> }
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

    if args.custom && args.range.is_some() {
        return Err(syn::Error::new(
            ident.span(),
            "#[signature(custom)] cannot be combined with #[signature(range = ...)]",
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
    if let Some(range) = &args.range {
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
        return Ok(quote! {
            ::nirvash_core::BoundedDomain::new((#iter).map(Self).collect())
        });
    }

    match data {
        Data::Enum(data) => enum_domain_body(data),
        Data::Struct(data) => struct_domain_body(data),
        Data::Union(data) => Err(syn::Error::new(
            data.union_token.span(),
            "Signature derive does not support unions",
        )),
    }
}

fn signature_invariant_body(
    ident: &Ident,
    data: &Data,
    args: &SignatureArgs,
) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(range) = &args.range {
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
        return Ok(quote! { (#range).contains(&self.0) });
    }
    match data {
        Data::Enum(data) => enum_invariant_body(data),
        Data::Struct(data) => struct_invariant_body(data),
        Data::Union(data) => Err(syn::Error::new(
            data.union_token.span(),
            "Signature derive does not support unions",
        )),
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
                let construct = quote! {
                    Self::#variant_ident(#(#bindings.clone()),*)
                };
                nested_loops(
                    &bindings,
                    &fields.unnamed,
                    quote! { values.push(#construct); },
                )
            }
            Fields::Named(fields) => {
                let bindings = named_field_bindings(&fields.named);
                let names = fields
                    .named
                    .iter()
                    .map(|field| field.ident.as_ref().expect("named"));
                let construct = quote! {
                    Self::#variant_ident { #(#names: #bindings.clone()),* }
                };
                nested_loops(
                    &bindings,
                    &fields.named,
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

fn struct_domain_body(data: &DataStruct) -> syn::Result<proc_macro2::TokenStream> {
    match &data.fields {
        Fields::Unit => Ok(quote! { ::nirvash_core::BoundedDomain::singleton(Self) }),
        Fields::Unnamed(fields) => {
            let bindings = field_bindings(&fields.unnamed);
            let construct = quote! { Self(#(#bindings.clone()),*) };
            let loops = nested_loops(
                &bindings,
                &fields.unnamed,
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
            let names = fields
                .named
                .iter()
                .map(|field| field.ident.as_ref().expect("named"));
            let construct = quote! { Self { #(#names: #bindings.clone()),* } };
            let loops = nested_loops(
                &bindings,
                &fields.named,
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

fn enum_invariant_body(data: &DataEnum) -> syn::Result<proc_macro2::TokenStream> {
    let arms = data.variants.iter().map(|variant| {
        let ident = &variant.ident;
        match &variant.fields {
            Fields::Unit => quote! { Self::#ident => true },
            Fields::Unnamed(fields) => {
                let bindings = field_bindings(&fields.unnamed);
                quote! {
                    Self::#ident(#(#bindings),*) => true #(&& ::nirvash_core::Signature::invariant(#bindings))*
                }
            }
            Fields::Named(fields) => {
                let bindings = named_field_bindings(&fields.named);
                let names = fields.named.iter().map(|field| field.ident.as_ref().expect("named"));
                quote! {
                    Self::#ident { #(#names: #bindings),* } => true #(&& ::nirvash_core::Signature::invariant(#bindings))*
                }
            }
        }
    });
    Ok(quote! { match self { #(#arms),* } })
}

fn struct_invariant_body(data: &DataStruct) -> syn::Result<proc_macro2::TokenStream> {
    match &data.fields {
        Fields::Unit => Ok(quote! { true }),
        Fields::Unnamed(fields) => {
            let checks = fields.unnamed.iter().enumerate().map(|(index, _)| {
                let access = syn::Index::from(index);
                quote! { ::nirvash_core::Signature::invariant(&self.#access) }
            });
            Ok(quote! { true #(&& #checks)* })
        }
        Fields::Named(fields) => {
            let checks = fields.named.iter().map(|field| {
                let ident = field.ident.as_ref().expect("named");
                quote! { ::nirvash_core::Signature::invariant(&self.#ident) }
            });
            Ok(quote! { true #(&& #checks)* })
        }
    }
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
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
    inner: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    bindings
        .iter()
        .zip(fields.iter())
        .rev()
        .fold(inner, |acc, (binding, field)| {
            let ty = &field.ty;
            quote! {
                for #binding in &<#ty as ::nirvash_core::Signature>::bounded_domain().into_vec() {
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
    let state_ty = associated_type(&item, "State")?;
    let action_ty = associated_type(&item, "Action")?;

    let checker_config = args.checker_config;
    let subsystems = args.subsystems;

    if !emit_composition && !subsystems.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "subsystems(...) is only supported on #[system_spec]",
        ));
    }

    let checker_config_expr = if let Some(checker_config) = checker_config {
        quote! { #checker_config() }
    } else {
        quote! { ::nirvash_core::ModelCheckConfig::default() }
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
                    let mut composition = ::nirvash_core::SystemComposition::new(self.name())
                        .with_checker_config(
                            <#self_ty as ::nirvash_core::TemporalSpec>::checker_config(self)
                        );
                    #(#subsystem_calls)*
                    for invariant in <#self_ty as ::nirvash_core::TemporalSpec>::invariants(self) {
                        composition = composition.with_invariant(invariant);
                    }
                    for illegal in <#self_ty as ::nirvash_core::TemporalSpec>::illegal_transitions(self) {
                        composition = composition.with_illegal_transition(illegal);
                    }
                    for constraint in <#self_ty as ::nirvash_core::TemporalSpec>::state_constraints(self) {
                        composition = composition.with_state_constraint(constraint);
                    }
                    for constraint in <#self_ty as ::nirvash_core::TemporalSpec>::action_constraints(self) {
                        composition = composition.with_action_constraint(constraint);
                    }
                    for property in <#self_ty as ::nirvash_core::TemporalSpec>::properties(self) {
                        composition = composition.with_property(property);
                    }
                    for fairness in <#self_ty as ::nirvash_core::TemporalSpec>::fairness(self) {
                        composition = composition.with_fairness(fairness);
                    }
                    if let ::core::option::Option::Some(symmetry) =
                        <#self_ty as ::nirvash_core::TemporalSpec>::symmetry(self)
                    {
                        composition = composition.with_symmetry(symmetry);
                    }
                    composition
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #item

        impl ::nirvash_core::TemporalSpec for #self_ty {
            fn invariants(&self) -> Vec<::nirvash_core::StatePredicate<Self::State>> {
                ::nirvash_core::registry::collect_invariants::<Self>()
            }

            fn illegal_transitions(
                &self,
            ) -> Vec<::nirvash_core::StepPredicate<Self::State, Self::Action>> {
                ::nirvash_core::registry::collect_illegal::<Self>()
            }

            fn state_constraints(
                &self,
            ) -> Vec<::nirvash_core::StateConstraint<Self::State>> {
                ::nirvash_core::registry::collect_state_constraints::<Self>()
            }

            fn action_constraints(
                &self,
            ) -> Vec<::nirvash_core::ActionConstraint<Self::State, Self::Action>> {
                ::nirvash_core::registry::collect_action_constraints::<Self>()
            }

            fn properties(&self) -> Vec<::nirvash_core::Ltl<Self::State, Self::Action>> {
                ::nirvash_core::registry::collect_properties::<Self>()
            }

            fn fairness(&self) -> Vec<::nirvash_core::Fairness<Self::State, Self::Action>> {
                ::nirvash_core::registry::collect_fairness::<Self>()
            }

            fn symmetry(&self) -> ::core::option::Option<::nirvash_core::SymmetryReducer<Self::State>> {
                ::nirvash_core::registry::collect_symmetry::<Self>()
            }

            fn checker_config(&self) -> ::nirvash_core::ModelCheckConfig {
                #checker_config_expr
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
    let init_method = args.init;
    let cases_method = args.cases;
    let composition_method = args.composition;
    let module_ident = format_ident!(
        "__nirvash_formal_tests_{}",
        path_tail_ident(&spec_ty)?.to_string().to_lowercase()
    );

    let cases_expr = if let Some(cases_method) = cases_method {
        quote! { <#spec_ty>::#cases_method() }
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
                    let expected_illegal = <#spec_ty as ::nirvash_core::TemporalSpec>::illegal_transitions(&spec)
                        .into_iter()
                        .map(|predicate| predicate.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_state_constraints = <#spec_ty as ::nirvash_core::TemporalSpec>::state_constraints(&spec)
                        .into_iter()
                        .map(|constraint| constraint.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_action_constraints = <#spec_ty as ::nirvash_core::TemporalSpec>::action_constraints(&spec)
                        .into_iter()
                        .map(|constraint| constraint.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_properties = <#spec_ty as ::nirvash_core::TemporalSpec>::properties(&spec)
                        .into_iter()
                        .map(|property| property.describe())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_fairness = <#spec_ty as ::nirvash_core::TemporalSpec>::fairness(&spec)
                        .into_iter()
                        .map(|fairness| fairness.name())
                        .collect::<::std::vec::Vec<_>>();
                    let expected_symmetry = <#spec_ty as ::nirvash_core::TemporalSpec>::symmetry(&spec)
                        .map(|symmetry| symmetry.name());

                    assert_eq!(composition.subsystems(), <#spec_ty>::registered_subsystems());
                    assert_eq!(composition.invariants().iter().map(|predicate| predicate.name()).collect::<::std::vec::Vec<_>>(), expected_invariants);
                    assert_eq!(composition.illegal_transitions().iter().map(|predicate| predicate.name()).collect::<::std::vec::Vec<_>>(), expected_illegal);
                    assert_eq!(composition.state_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>(), expected_state_constraints);
                    assert_eq!(composition.action_constraints().iter().map(|constraint| constraint.name()).collect::<::std::vec::Vec<_>>(), expected_action_constraints);
                    assert_eq!(composition.properties().iter().map(|property| property.describe()).collect::<::std::vec::Vec<_>>(), expected_properties);
                    assert_eq!(composition.fairness().iter().map(|fairness| fairness.name()).collect::<::std::vec::Vec<_>>(), expected_fairness);
                    assert_eq!(composition.symmetry().map(|symmetry| symmetry.name()), expected_symmetry);
                    assert_eq!(composition.checker_config(), <#spec_ty as ::nirvash_core::TemporalSpec>::checker_config(&spec));
                }
            }
        }
    });

    Ok(quote! {
        #[cfg(test)]
        mod #module_ident {
            use super::*;

            type GeneratedState = <#spec_ty as ::nirvash_core::TransitionSystem>::State;
            type GeneratedAction = <#spec_ty as ::nirvash_core::TransitionSystem>::Action;

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            fn generated_states() -> ::std::vec::Vec<GeneratedState> {
                <GeneratedState as ::nirvash_core::Signature>::bounded_domain().into_vec()
            }

            fn generated_actions() -> ::std::vec::Vec<GeneratedAction> {
                <GeneratedAction as ::nirvash_core::Signature>::bounded_domain().into_vec()
            }

            #[test]
            fn generated_init_state_satisfies_invariants() {
                for spec in generated_cases() {
                    let init = spec.#init_method();
                    assert!(<#spec_ty as ::nirvash_core::TransitionSystem>::init(&spec, &init));
                    assert!(::nirvash_core::Signature::invariant(&init));
                    assert!(
                        <#spec_ty as ::nirvash_core::TemporalSpec>::invariants(&spec)
                            .iter()
                            .all(|predicate| predicate.eval(&init))
                    );
                }
            }

            #[test]
            fn generated_model_checker_accepts_spec() {
                for spec in generated_cases() {
                    let checker = ::nirvash_core::ModelChecker::new(&spec);
                    let result = checker.check_all().expect("model checker should run");
                    assert!(result.is_ok(), "{:?}", result.violations());
                }
            }

            #[test]
            fn generated_state_domain_satisfies_signature_invariant() {
                for state in generated_states() {
                    assert!(
                        <GeneratedState as ::nirvash_core::Signature>::invariant(&state),
                        "state domain violates signature invariant: {:?}",
                        state
                    );
                }
            }

            #[test]
            fn generated_action_domain_satisfies_signature_invariant() {
                for action in generated_actions() {
                    assert!(
                        <GeneratedAction as ::nirvash_core::Signature>::invariant(&action),
                        "action domain violates signature invariant: {:?}",
                        action
                    );
                }
            }

            #[test]
            fn generated_state_domain_satisfies_registered_state_predicates() {
                for spec in generated_cases() {
                    let invariants = <#spec_ty as ::nirvash_core::TemporalSpec>::invariants(&spec);
                    let state_constraints = <#spec_ty as ::nirvash_core::TemporalSpec>::state_constraints(&spec);
                    for state in generated_states() {
                        assert!(
                            invariants.iter().all(|predicate| predicate.eval(&state)),
                            "registered invariant failed for state {:?}",
                            state
                        );
                        assert!(
                            state_constraints.iter().all(|constraint| constraint.eval(&state)),
                            "state constraint failed for state {:?}",
                            state
                        );
                    }
                }
            }

            #[test]
            fn generated_illegal_predicates_exclude_transitions() {
                for spec in generated_cases() {
                    let illegal = <#spec_ty as ::nirvash_core::TemporalSpec>::illegal_transitions(&spec);
                    for prev in generated_states() {
                        for action in generated_actions() {
                            for next in generated_states() {
                                let is_illegal = illegal.iter().any(|predicate| predicate.eval(&prev, &action, &next));
                                if is_illegal {
                                    assert!(
                                        !<#spec_ty as ::nirvash_core::TransitionSystem>::next(&spec, &prev, &action, &next),
                                        "illegal predicate allowed transition: {:?} -- {:?} --> {:?}",
                                        prev,
                                        action,
                                        next
                                    );
                                }
                            }
                        }
                    }
                }
            }

            #[test]
            fn generated_allowed_transitions_respect_constraints() {
                for spec in generated_cases() {
                    let state_constraints = <#spec_ty as ::nirvash_core::TemporalSpec>::state_constraints(&spec);
                    let action_constraints = <#spec_ty as ::nirvash_core::TemporalSpec>::action_constraints(&spec);
                    for prev in generated_states() {
                        for action in generated_actions() {
                            for next in generated_states() {
                                if <#spec_ty as ::nirvash_core::TransitionSystem>::next(&spec, &prev, &action, &next) {
                                    assert!(
                                        <GeneratedState as ::nirvash_core::Signature>::invariant(&next),
                                        "allowed transition produced state violating signature invariant: {:?}",
                                        next
                                    );
                                    assert!(
                                        state_constraints.iter().all(|constraint| constraint.eval(&next)),
                                        "allowed transition produced state violating state constraints: {:?}",
                                        next
                                    );
                                    assert!(
                                        action_constraints.iter().all(|constraint| constraint.eval(&prev, &action, &next)),
                                        "allowed transition violated action constraints: {:?} -- {:?} --> {:?}",
                                        prev,
                                        action,
                                        next
                                    );
                                }
                            }
                        }
                    }
                }
            }

            #composition_test
        }
    })
}

fn path_tail_ident(path: &Path) -> syn::Result<&Ident> {
    path.segments
        .last()
        .map(|segment| &segment.ident)
        .ok_or_else(|| syn::Error::new(path.span(), "path cannot be empty"))
}
