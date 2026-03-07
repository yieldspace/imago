use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, ExprRange, Field, Fields, Ident, ImplItem,
    ItemConst, ItemImpl, LitStr, Path, RangeLimits, Type, parse_macro_input,
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
pub fn imago_subsystem_spec(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as SpecArgs);
    let item = parse_macro_input!(item as ItemImpl);
    match expand_temporal_spec(args, item, false) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn imago_system_spec(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as SpecArgs);
    let item = parse_macro_input!(item as ItemImpl);
    match expand_temporal_spec(args, item, true) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn imago_formal_tests(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as TestArgs);
    let _item = parse_macro_input!(item as ItemConst);
    match expand_formal_tests(args) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn imago_invariant(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn imago_illegal(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn imago_property(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn imago_fairness(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn imago_state_constraint(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn imago_action_constraint(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn imago_symmetry(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
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
    invariants: Vec<Path>,
    illegal: Vec<Path>,
    state_constraints: Vec<Path>,
    action_constraints: Vec<Path>,
    properties: Vec<Path>,
    fairness: Vec<Path>,
    symmetry: Vec<Path>,
    checker_config: Option<Path>,
    subsystems: Vec<LitStr>,
}

impl syn::parse::Parse for SpecArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self {
            invariants: Vec::new(),
            illegal: Vec::new(),
            state_constraints: Vec::new(),
            action_constraints: Vec::new(),
            properties: Vec::new(),
            fairness: Vec::new(),
            symmetry: Vec::new(),
            checker_config: None,
            subsystems: Vec::new(),
        };

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            let content;
            syn::parenthesized!(content in input);
            match ident.to_string().as_str() {
                "invariants" => args.invariants = parse_path_list(&content)?,
                "illegal" => args.illegal = parse_path_list(&content)?,
                "state_constraints" => args.state_constraints = parse_path_list(&content)?,
                "action_constraints" => args.action_constraints = parse_path_list(&content)?,
                "properties" => args.properties = parse_path_list(&content)?,
                "fairness" => args.fairness = parse_path_list(&content)?,
                "symmetry" => args.symmetry = parse_path_list(&content)?,
                "checker_config" => args.checker_config = Some(parse_single_path(&content)?),
                "subsystems" => args.subsystems = parse_string_list(&content)?,
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

fn parse_path_list(input: &syn::parse::ParseBuffer<'_>) -> syn::Result<Vec<Path>> {
    let mut paths = Vec::new();
    while !input.is_empty() {
        paths.push(input.parse()?);
        if input.peek(syn::Token![,]) {
            let _ = input.parse::<syn::Token![,]>()?;
        }
    }
    Ok(paths)
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
            fn representatives() -> ::imago_formal_core::BoundedDomain<Self>;

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
                fn representatives() -> ::imago_formal_core::BoundedDomain<Self> {
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

        impl #impl_generics ::imago_formal_core::Signature for #ident #ty_generics #where_clause {
            fn bounded_domain() -> ::imago_formal_core::BoundedDomain<Self> {
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
            ::imago_formal_core::BoundedDomain::new((#iter).map(Self).collect())
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
        ::imago_formal_core::BoundedDomain::new(values)
    })
}

fn struct_domain_body(data: &DataStruct) -> syn::Result<proc_macro2::TokenStream> {
    match &data.fields {
        Fields::Unit => Ok(quote! { ::imago_formal_core::BoundedDomain::singleton(Self) }),
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
                ::imago_formal_core::BoundedDomain::new(values)
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
                ::imago_formal_core::BoundedDomain::new(values)
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
                    Self::#ident(#(#bindings),*) => true #(&& ::imago_formal_core::Signature::invariant(#bindings))*
                }
            }
            Fields::Named(fields) => {
                let bindings = named_field_bindings(&fields.named);
                let names = fields.named.iter().map(|field| field.ident.as_ref().expect("named"));
                quote! {
                    Self::#ident { #(#names: #bindings),* } => true #(&& ::imago_formal_core::Signature::invariant(#bindings))*
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
                quote! { ::imago_formal_core::Signature::invariant(&self.#access) }
            });
            Ok(quote! { true #(&& #checks)* })
        }
        Fields::Named(fields) => {
            let checks = fields.named.iter().map(|field| {
                let ident = field.ident.as_ref().expect("named");
                quote! { ::imago_formal_core::Signature::invariant(&self.#ident) }
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
                for #binding in &<#ty as ::imago_formal_core::Signature>::bounded_domain().into_vec() {
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

    let invariants = args.invariants;
    let illegal = args.illegal;
    let state_constraints = args.state_constraints;
    let action_constraints = args.action_constraints;
    let properties = args.properties;
    let fairness = args.fairness;
    let symmetry = args.symmetry;
    let checker_config = args.checker_config;
    let subsystems = args.subsystems;

    if symmetry.len() > 1 {
        return Err(syn::Error::new(
            Span::call_site(),
            "at most one symmetry(...) entry is supported",
        ));
    }
    let symmetry_expr = if let Some(symmetry) = symmetry.first() {
        quote! { ::core::option::Option::Some(#symmetry()) }
    } else {
        quote! { ::core::option::Option::None }
    };
    let checker_config_expr = if let Some(checker_config) = checker_config {
        quote! { #checker_config() }
    } else {
        quote! { ::imago_formal_core::ModelCheckConfig::default() }
    };

    let composition_impl = if emit_composition {
        let subsystem_calls = subsystems.iter().map(|name| {
            quote! { .with_subsystem(#name) }
        });
        quote! {
            impl #self_ty {
                pub fn composition(&self) -> ::imago_formal_core::SystemComposition<#state_ty, #action_ty> {
                    ::imago_formal_core::SystemComposition::new(self.name())
                        #(#subsystem_calls)*
                        #(.with_invariant(#invariants()))*
                        #(.with_illegal_transition(#illegal()))*
                        #(.with_state_constraint(#state_constraints()))*
                        #(.with_action_constraint(#action_constraints()))*
                        #(.with_property(#properties()))*
                        #(.with_fairness(#fairness()))*
                        #(.with_symmetry(#symmetry()))*
                        .with_checker_config(
                            <#self_ty as ::imago_formal_core::TemporalSpec>::checker_config(self)
                        )
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #item

        impl ::imago_formal_core::TemporalSpec for #self_ty {
            fn invariants(&self) -> Vec<::imago_formal_core::StatePredicate<Self::State>> {
                vec![#(#invariants()),*]
            }

            fn illegal_transitions(
                &self,
            ) -> Vec<::imago_formal_core::StepPredicate<Self::State, Self::Action>> {
                vec![#(#illegal()),*]
            }

            fn state_constraints(
                &self,
            ) -> Vec<::imago_formal_core::StateConstraint<Self::State>> {
                vec![#(#state_constraints()),*]
            }

            fn action_constraints(
                &self,
            ) -> Vec<::imago_formal_core::ActionConstraint<Self::State, Self::Action>> {
                vec![#(#action_constraints()),*]
            }

            fn properties(&self) -> Vec<::imago_formal_core::Ltl<Self::State, Self::Action>> {
                vec![#(#properties()),*]
            }

            fn fairness(&self) -> Vec<::imago_formal_core::Fairness<Self::State, Self::Action>> {
                vec![#(#fairness()),*]
            }

            fn symmetry(&self) -> ::core::option::Option<::imago_formal_core::SymmetryReducer<Self::State>> {
                #symmetry_expr
            }

            fn checker_config(&self) -> ::imago_formal_core::ModelCheckConfig {
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
        "__imago_formal_tests_{}",
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
            fn generated_composition_exposes_registered_fragments() {
                for spec in generated_cases() {
                    let composition = spec.#composition_method();
                    assert!(!composition.subsystems().is_empty());
                    assert!(!composition.invariants().is_empty());
                    assert!(!composition.illegal_transitions().is_empty());
                    assert!(!composition.properties().is_empty());
                }
            }
        }
    });

    Ok(quote! {
        #[cfg(test)]
        mod #module_ident {
            use super::*;

            fn generated_cases() -> ::std::vec::Vec<#spec_ty> {
                #cases_expr
            }

            #[test]
            fn generated_init_state_satisfies_invariants() {
                for spec in generated_cases() {
                    let init = spec.#init_method();
                    assert!(<#spec_ty as ::imago_formal_core::TransitionSystem>::init(&spec, &init));
                    assert!(::imago_formal_core::Signature::invariant(&init));
                    assert!(
                        <#spec_ty as ::imago_formal_core::TemporalSpec>::invariants(&spec)
                            .iter()
                            .all(|predicate| predicate.eval(&init))
                    );
                }
            }

            #[test]
            fn generated_model_checker_accepts_spec() {
                for spec in generated_cases() {
                    let checker = ::imago_formal_core::ModelChecker::new(&spec);
                    let invariants = checker.check_invariants().expect("invariant check should run");
                    let illegal = checker
                        .check_illegal_transitions()
                        .expect("illegal transition check should run");
                    let deadlocks = checker.check_deadlocks().expect("deadlock check should run");
                    let properties = checker.check_properties().expect("property check should run");
                    assert!(invariants.is_ok(), "{:?}", invariants.violations());
                    assert!(illegal.is_ok(), "{:?}", illegal.violations());
                    assert!(deadlocks.is_ok(), "{:?}", deadlocks.violations());
                    assert!(properties.is_ok(), "{:?}", properties.violations());
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
