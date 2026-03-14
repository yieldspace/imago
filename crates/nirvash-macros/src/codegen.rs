use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{ToTokens, format_ident, quote};
use std::path::PathBuf;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    Error, Expr, ExprLit, ExprRange, FnArg, GenericArgument, Ident, ImplItem, ImplItemFn, Item,
    ItemEnum, ItemImpl, ItemStruct, ItemType, Lit, Pat, PatIdent, Path, PathArguments, PathSegment,
    ReturnType, Token, Type, braced, bracketed,
};

pub fn expand_code_tests(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream2> {
    let args = syn::parse::<CodeTestsArgs>(attr)?;
    let item = syn::parse::<Item>(item)?;
    let spec_ident = spec_ident(&item)?;
    ensure_non_generic_spec(&item)?;

    let export_ident = Ident::new("generated", Span::call_site());
    let export_alias = args
        .export
        .clone()
        .filter(|ident| ident != &export_ident)
        .map(|ident| {
            quote! {
                pub use #export_ident as #ident;
            }
        })
        .unwrap_or_default();
    let default_model_names = args.models.clone().unwrap_or_else(default_model_names);
    let profile_plan = resolve_profiles(args.profiles.clone());
    let default_profile_idents = if let Some(profiles) = &args.profiles {
        profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect()
    } else {
        vec![
            Ident::new("smoke_default", Span::call_site()),
            Ident::new("unit_default", Span::call_site()),
            Ident::new("e2e_default", Span::call_site()),
        ]
    };
    let default_profile_names = default_profile_idents
        .iter()
        .map(|ident| ident.to_string())
        .collect::<Vec<_>>();

    let hidden_all = format_ident!(
        "__nirvash_generated_all_tests_{}",
        spec_ident.to_string().to_lowercase()
    );
    let hidden_tests = format_ident!(
        "__nirvash_generated_tests_{}",
        spec_ident.to_string().to_lowercase()
    );
    let hidden_unit = format_ident!(
        "__nirvash_generated_unit_tests_{}",
        spec_ident.to_string().to_lowercase()
    );
    let hidden_trace = format_ident!(
        "__nirvash_generated_trace_tests_{}",
        spec_ident.to_string().to_lowercase()
    );
    let hidden_kani = format_ident!(
        "__nirvash_generated_kani_harnesses_{}",
        spec_ident.to_string().to_lowercase()
    );
    let hidden_loom = format_ident!(
        "__nirvash_generated_loom_tests_{}",
        spec_ident.to_string().to_lowercase()
    );
    let sanitized_spec_name = sanitize_file_component(&spec_ident.to_string());
    let generated_kani_rel_paths = profile_plan
        .ordered
        .iter()
        .filter(|profile| profile_has_kani(profile))
        .map(|profile| {
            (
                profile.name.to_string(),
                format!(
                    "tests/generated/{}_{}_kani.rs",
                    sanitized_spec_name,
                    sanitize_file_component(&profile.name.to_string()),
                ),
            )
        })
        .collect::<Vec<_>>();
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let generated_kani_includes = generated_kani_rel_paths
        .iter()
        .map(|(profile_name, path)| {
            let exists = manifest_dir
                .as_ref()
                .map(|dir| dir.join(path).exists())
                .unwrap_or(false);
            if exists {
                quote! {
                    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/", #path));
                }
            } else {
                quote! {
                    compile_error!(concat!(
                        "missing materialized Kani harness `",
                        #path,
                        "`; run `cargo nirvash materialize-tests --spec ",
                        stringify!(#spec_ident),
                        " --binding ",
                        stringify!($binding),
                        " --profile ",
                        #profile_name,
                        "` first"
                    ));
                }
            }
        })
        .collect::<Vec<_>>();
    let default_model_label_tokens = default_model_names
        .iter()
        .map(|ident| ident.to_string())
        .collect::<Vec<_>>();

    let generated_profile_fns = profile_plan
        .ordered
        .iter()
        .map(|profile| profile_fn_tokens(profile, spec_ident, &default_model_label_tokens));
    let profile_builder_match_arms = profile_plan.ordered.iter().map(|profile| {
        let ident = &profile.name;
        let label = ident.to_string();
        quote! { #label => Some(super::profiles::#ident()), }
    });
    let default_profile_exprs = default_profile_idents
        .iter()
        .map(|ident| quote! { super::profiles::#ident() })
        .collect::<Vec<_>>();
    let default_model_name_tokens = default_model_names
        .iter()
        .map(|ident| ident.to_string())
        .collect::<Vec<_>>();
    let default_profile_name_tokens = default_profile_names.clone();
    Ok(quote! {
        #item

        #[allow(non_snake_case)]
        pub mod #export_ident {
            #![allow(unexpected_cfgs)]
            use crate::*;

            type GeneratedSpec = super::#spec_ident;
            type GeneratedState = <GeneratedSpec as ::nirvash_lower::FrontendSpec>::State;
            type GeneratedAction = <GeneratedSpec as ::nirvash_lower::FrontendSpec>::Action;
            type GeneratedExpectedOutput =
                <GeneratedSpec as ::nirvash_conformance::SpecOracle>::ExpectedOutput;
            const EXPORT_MODULE_PATH: &str = module_path!();

            pub fn spec() -> GeneratedSpec {
                <GeneratedSpec as ::core::default::Default>::default()
            }

            pub fn __spec_marker() -> ::core::marker::PhantomData<GeneratedSpec> {
                ::core::marker::PhantomData
            }

            fn default_model_instance(
                spec: &GeneratedSpec,
            ) -> ::nirvash_lower::ModelInstance<GeneratedState, GeneratedAction> {
                <GeneratedSpec as ::nirvash_lower::FrontendSpec>::model_instances(spec)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            }

            fn model_instance_for(
                spec: &GeneratedSpec,
                label: &str,
            ) -> ::nirvash_lower::ModelInstance<GeneratedState, GeneratedAction> {
                let cases = <GeneratedSpec as ::nirvash_lower::FrontendSpec>::model_instances(spec);
                cases
                    .iter()
                    .find(|candidate| candidate.label() == label)
                    .cloned()
                    .or_else(|| cases.into_iter().next())
                    .unwrap_or_default()
            }

                pub mod prelude {
                    pub use ::nirvash::TrustTier;
                    pub use ::nirvash_conformance::{
                        CoverageGoal, boundary_numbers, profiles, seeds, small_keys,
                        smoke_fixture,
                    };
                pub use ::nirvash_macros::{
                    nirvash, nirvash_binding, nirvash_fixture, nirvash_project,
                    nirvash_project_output, nirvash_trace,
                };
                pub use super::bindings::*;
                pub use super::install::{
                    all_tests, kani_harnesses, loom_tests, tests, trace_tests, unit_tests,
                };
                pub use super::metadata::*;
                pub use super::plans::*;
                pub use super::replay::*;
                pub use super::seeds::{boundary, concurrent_small, e2e_default, small, soak};
            }

            pub mod metadata {
                use super::*;

                pub const SPEC_NAME: &str = stringify!(#spec_ident);
                pub const EXPORT_MODULE: &str = super::EXPORT_MODULE_PATH;
                pub const CRATE_PACKAGE: &str = env!("CARGO_PKG_NAME");
                pub const CRATE_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
                pub const DEFAULT_PROFILES: &[&str] = &[#(#default_profile_name_tokens),*];
                pub const DEFAULT_MODELS: &[&str] = &[#(#default_model_name_tokens),*];

                pub fn spec_metadata() -> ::nirvash_conformance::GeneratedSpecMetadata {
                    ::nirvash_conformance::GeneratedSpecMetadata {
                        spec_name: SPEC_NAME,
                        export_module: EXPORT_MODULE,
                        crate_package: CRATE_PACKAGE,
                        crate_manifest_dir: CRATE_MANIFEST_DIR,
                        normalized_fragment: ::nirvash_conformance::normalized_fragment_info(&super::spec()),
                        default_profiles: DEFAULT_PROFILES,
                    }
                }
            }

            pub mod seeds {
                pub fn small() -> ::nirvash_conformance::SeedProfile<super::GeneratedSpec> {
                    ::nirvash_conformance::small_for(&super::spec())
                }

                pub fn boundary() -> ::nirvash_conformance::SeedProfile<super::GeneratedSpec> {
                    ::nirvash_conformance::boundary_for(&super::spec())
                }

                pub fn concurrent_small() -> ::nirvash_conformance::SeedProfile<super::GeneratedSpec> {
                    ::nirvash_conformance::concurrent_small_for(&super::spec())
                }

                pub fn e2e_default() -> ::nirvash_conformance::SeedProfile<super::GeneratedSpec> {
                    ::nirvash_conformance::e2e_default_for(&super::spec())
                }

                pub fn soak() -> ::nirvash_conformance::SeedProfile<super::GeneratedSpec> {
                    ::nirvash_conformance::soak_for(&super::spec())
                }
            }

            pub mod profiles {
                use super::*;

                #(#generated_profile_fns)*
            }

            pub mod plans {
                use super::*;

                pub fn artifact_dir() -> ::nirvash_conformance::ArtifactDirPolicy {
                    ::nirvash_conformance::ArtifactDirPolicy::default()
                }

                pub fn from_builders(
                    fixture_factory: fn() -> ::nirvash_conformance::SharedFixtureValue,
                    builders: ::std::vec::Vec<::nirvash_conformance::TestProfileBuilder<GeneratedSpec>>,
                ) -> ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec> {
                    ::nirvash_conformance::GeneratedHarnessPlan::new(
                        super::metadata::spec_metadata(),
                        builders
                            .into_iter()
                            .map(|builder| {
                                builder
                                    .with_fixture_factory(fixture_factory)
                                    .build()
                            })
                            .collect(),
                        artifact_dir(),
                        true,
                    )
                }

                pub fn all(
                    fixture_factory: fn() -> ::nirvash_conformance::SharedFixtureValue,
                ) -> ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec> {
                    from_builders(fixture_factory, vec![#(#default_profile_exprs),*])
                }

                pub fn all_for<Binding>(
                    fixture_factory: fn() -> ::nirvash_conformance::SharedFixtureValue,
                ) -> ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec>
                where
                    Binding: ::nirvash_conformance::GeneratedBinding<GeneratedSpec>,
                {
                    let builders = vec![#(#default_profile_exprs),*]
                        .into_iter()
                        .filter(|builder| {
                            let profile = builder
                                .clone()
                                .with_fixture_factory(fixture_factory)
                                .build();
                            let needs_trace = profile.engines.iter().any(|engine| {
                                matches!(
                                    engine,
                                    ::nirvash_conformance::EnginePlan::TraceValidation { .. }
                                )
                            });
                            let needs_concurrency = profile.engines.iter().any(|engine| {
                                matches!(
                                    engine,
                                    ::nirvash_conformance::EnginePlan::LoomSmall { .. }
                                        | ::nirvash_conformance::EnginePlan::ShuttlePCT { .. }
                                )
                            });
                            (!needs_trace
                                || <Binding as ::nirvash_conformance::GeneratedBinding<
                                    GeneratedSpec,
                                >>::supports_trace())
                                && (!needs_concurrency
                                    || <Binding as ::nirvash_conformance::GeneratedBinding<
                                        GeneratedSpec,
                                    >>::supports_concurrency())
                        })
                        .collect();
                    from_builders(fixture_factory, builders)
                }

                pub fn unit(
                    fixture_factory: fn() -> ::nirvash_conformance::SharedFixtureValue,
                ) -> ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec> {
                    from_builders(fixture_factory, vec![super::profiles::unit_default()])
                }

                pub fn trace(
                    fixture_factory: fn() -> ::nirvash_conformance::SharedFixtureValue,
                ) -> ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec> {
                    from_builders(fixture_factory, vec![super::profiles::e2e_default()])
                }

                pub fn kani(
                    fixture_factory: fn() -> ::nirvash_conformance::SharedFixtureValue,
                ) -> ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec> {
                    from_builders(
                        fixture_factory,
                        vec![
                            super::profiles::unit_default().engines([
                                ::nirvash_conformance::EnginePlan::KaniBounded { depth: 4 }
                            ])
                        ],
                    )
                }

                pub fn builder_for_label(
                    label: &str,
                ) -> Option<::nirvash_conformance::TestProfileBuilder<GeneratedSpec>> {
                    match label {
                        #(#profile_builder_match_arms)*
                        _ => None,
                    }
                }

                pub fn profile_for_label(
                    label: &str,
                ) -> Option<::nirvash_conformance::TestProfile<GeneratedSpec>> {
                    builder_for_label(label).map(|builder| builder.build())
                }

                pub fn collect_kani_obligations_for(
                    label: &str,
                ) -> Result<
                    ::std::vec::Vec<::nirvash_conformance::MaterializedKaniObligation<GeneratedAction>>,
                    ::nirvash_conformance::HarnessError,
                >
                where
                    GeneratedSpec:
                        ::nirvash_conformance::SpecOracle
                            + ::nirvash_lower::TemporalSpec
                            + ::core::default::Default,
                    GeneratedState:
                        Clone + PartialEq + ::nirvash_lower::FiniteModelDomain + Send + Sync + 'static,
                    GeneratedAction:
                        Clone + PartialEq + ::serde::Serialize + ::serde::de::DeserializeOwned + Send + Sync + 'static,
                {
                    let profile = profile_for_label(label).ok_or_else(|| {
                        ::nirvash_conformance::HarnessError::Binding(format!(
                            "unknown generated profile `{label}`"
                        ))
                    })?;
                    let depth = profile
                        .engines
                        .iter()
                        .find_map(|engine| match engine {
                            ::nirvash_conformance::EnginePlan::KaniBounded { depth } => Some(*depth),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            ::nirvash_conformance::HarnessError::Binding(format!(
                                "profile `{label}` does not contain EnginePlan::KaniBounded"
                            ))
                        })?;
                    Ok(::nirvash_conformance::collect_kani_obligations(
                        &super::spec(),
                        &profile,
                        depth,
                    )?
                    .into_iter()
                    .map(|(id, actions)| ::nirvash_conformance::MaterializedKaniObligation {
                        id,
                        actions,
                    })
                    .collect())
                }

                pub fn loom(
                    fixture_factory: fn() -> ::nirvash_conformance::SharedFixtureValue,
                ) -> ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec> {
                    from_builders(
                        fixture_factory,
                        vec![
                            super::profiles::concurrency_default().engines([
                                ::nirvash_conformance::EnginePlan::LoomSmall {
                                    threads: 2,
                                    max_permutations: 8,
                                }
                            ])
                        ],
                    )
                }
            }

            pub mod bindings {
                pub type State = super::GeneratedState;
                pub type Action = super::GeneratedAction;
                pub type ExpectedOutput = super::GeneratedExpectedOutput;

                pub fn binding_name<Binding>() -> &'static str {
                    ::core::any::type_name::<Binding>()
                }
            }

            pub mod replay {
                use super::*;
                use ::serde::{Serialize, de::DeserializeOwned};

                pub fn load(
                    path: impl AsRef<::std::path::Path>,
                ) -> Result<
                    ::nirvash_conformance::GeneratedReplayBundle<
                        GeneratedState,
                        GeneratedAction,
                        GeneratedExpectedOutput,
                    >,
                    ::nirvash_conformance::HarnessError,
                >
                where
                    GeneratedState: DeserializeOwned,
                    GeneratedAction: DeserializeOwned,
                    GeneratedExpectedOutput: DeserializeOwned,
                {
                    ::nirvash_conformance::read_replay_bundle(path)
                }

                pub fn run<Binding>(
                    path: impl AsRef<::std::path::Path>,
                ) -> Result<(), ::nirvash_conformance::HarnessError>
                where
                    GeneratedSpec: ::nirvash_conformance::SpecOracle,
                    GeneratedState: Clone + PartialEq + DeserializeOwned,
                    GeneratedAction: Clone + PartialEq + ::core::fmt::Debug + DeserializeOwned,
                    GeneratedExpectedOutput:
                        Clone + PartialEq + Eq + ::core::fmt::Debug + DeserializeOwned,
                    Binding: ::nirvash_conformance::GeneratedBinding<GeneratedSpec>,
                {
                    let fixture = <Binding as ::nirvash_conformance::GeneratedBinding<GeneratedSpec>>::generated_fixture()
                        .downcast::<<Binding as ::nirvash_conformance::RuntimeBinding<GeneratedSpec>>::Fixture>()
                        .map_err(|_| {
                            ::nirvash_conformance::HarnessError::Binding(
                                "generated fixture factory returned the wrong type".to_owned(),
                            )
                        })?;
                    let bundle = load(path)?;
                    ::nirvash_conformance::replay_action_trace::<GeneratedSpec, Binding>(
                        &super::spec(),
                        &bundle.action_trace,
                        fixture.as_ref(),
                    )
                }

                pub fn run_with<Binding>(
                    path: impl AsRef<::std::path::Path>,
                    fixture: <Binding as ::nirvash_conformance::RuntimeBinding<GeneratedSpec>>::Fixture,
                ) -> Result<(), ::nirvash_conformance::HarnessError>
                where
                    GeneratedSpec: ::nirvash_conformance::SpecOracle,
                    GeneratedState: Clone + PartialEq + DeserializeOwned,
                    GeneratedAction: Clone + PartialEq + ::core::fmt::Debug + DeserializeOwned,
                    GeneratedExpectedOutput:
                        Clone + PartialEq + Eq + ::core::fmt::Debug + DeserializeOwned,
                    Binding: ::nirvash_conformance::RuntimeBinding<GeneratedSpec>,
                {
                    let bundle = load(path)?;
                    ::nirvash_conformance::replay_action_trace::<GeneratedSpec, Binding>(
                        &super::spec(),
                        &bundle.action_trace,
                        &fixture,
                    )
                }

                pub fn persist(
                    bundle: &::nirvash_conformance::GeneratedReplayBundle<
                        GeneratedState,
                        GeneratedAction,
                        GeneratedExpectedOutput,
                    >,
                    profile: &str,
                    engine: &str,
                ) -> Result<
                    (::std::path::PathBuf, ::std::path::PathBuf),
                    ::nirvash_conformance::HarnessError,
                >
                where
                    GeneratedState: Serialize,
                    GeneratedAction: Serialize,
                    GeneratedExpectedOutput: Serialize,
                {
                    ::nirvash_conformance::write_replay_bundle(
                        &super::plans::artifact_dir(),
                        super::metadata::SPEC_NAME,
                        profile,
                        engine,
                        bundle,
                    )
                }
            }

            pub mod install {
                use super::*;
                use ::serde::{Serialize, de::DeserializeOwned};

                fn profile_is_concurrent(
                    profile: &::nirvash_conformance::TestProfile<GeneratedSpec>,
                ) -> bool {
                    profile.engines.iter().any(|engine| {
                        matches!(
                            engine,
                            ::nirvash_conformance::EnginePlan::LoomSmall { .. }
                                | ::nirvash_conformance::EnginePlan::ShuttlePCT { .. }
                        )
                    })
                }

                fn profile_is_trace(
                    profile: &::nirvash_conformance::TestProfile<GeneratedSpec>,
                ) -> bool {
                    let has_trace = profile.engines.iter().any(|engine| {
                        matches!(
                            engine,
                            ::nirvash_conformance::EnginePlan::TraceValidation { .. }
                        )
                    });
                    has_trace
                        && profile.engines.iter().all(|engine| {
                            matches!(
                                engine,
                                ::nirvash_conformance::EnginePlan::TraceValidation { .. }
                                    | ::nirvash_conformance::EnginePlan::KaniBounded { .. }
                            )
                        })
                }

                pub fn __run_selected_plan<Binding>(
                    plan: ::nirvash_conformance::GeneratedHarnessPlan<GeneratedSpec>,
                    binding_name: &str,
                ) -> Result<(), ::nirvash_conformance::HarnessError>
                where
                    GeneratedSpec:
                        ::nirvash_conformance::SpecOracle
                            + ::nirvash_lower::TemporalSpec
                            + ::core::default::Default,
                    GeneratedState:
                        Clone + PartialEq + ::nirvash_lower::FiniteModelDomain + Serialize + DeserializeOwned + Send + Sync + 'static,
                    GeneratedAction:
                        Clone + PartialEq + ::core::fmt::Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
                    GeneratedExpectedOutput:
                        Clone + PartialEq + Eq + ::core::fmt::Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
                    Binding: ::nirvash_conformance::GeneratedBinding<GeneratedSpec>,
                {
                    let spec = super::spec();
                    for profile in &plan.profiles {
                        if profile_is_concurrent(profile) {
                            <Binding as ::nirvash_conformance::GeneratedBinding<GeneratedSpec>>::run_generated_concurrent_profile(
                                &spec,
                                plan.metadata(),
                                profile,
                                binding_name,
                                plan.artifact_dir_policy(),
                                plan.materialize_failures,
                            )?;
                        } else if profile_is_trace(profile) {
                            <Binding as ::nirvash_conformance::GeneratedBinding<GeneratedSpec>>::run_generated_trace_profile(
                                &spec,
                                plan.metadata(),
                                profile,
                                binding_name,
                                plan.artifact_dir_policy(),
                                plan.materialize_failures,
                            )?;
                        } else {
                            <Binding as ::nirvash_conformance::GeneratedBinding<GeneratedSpec>>::run_generated_profile(
                                &spec,
                                plan.metadata(),
                                profile,
                                binding_name,
                                plan.artifact_dir_policy(),
                                plan.materialize_failures,
                            )?;
                        }
                    }
                    Ok(())
                }

                pub fn __run_materialized_kani<Binding>(
                    binding_name: &str,
                    profile_label: &str,
                    obligation_id: &str,
                    actions: &[GeneratedAction],
                ) -> Result<(), ::nirvash_conformance::HarnessError>
                where
                    GeneratedSpec:
                        ::nirvash_conformance::SpecOracle
                            + ::nirvash_lower::TemporalSpec
                            + ::core::default::Default,
                    GeneratedState:
                        Clone + PartialEq + ::nirvash_lower::FiniteModelDomain + Serialize + DeserializeOwned + Send + Sync + 'static,
                    GeneratedAction:
                        Clone + PartialEq + ::core::fmt::Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
                    GeneratedExpectedOutput:
                        Clone + PartialEq + Eq + ::core::fmt::Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
                    Binding: ::nirvash_conformance::GeneratedBinding<GeneratedSpec>,
                {
                    let profile = super::plans::builder_for_label(profile_label)
                        .ok_or_else(|| {
                            ::nirvash_conformance::HarnessError::Binding(format!(
                                "unknown generated profile `{profile_label}`"
                            ))
                        })?
                        .with_fixture_factory(
                            <Binding as ::nirvash_conformance::GeneratedBinding<
                                GeneratedSpec,
                            >>::generated_fixture,
                        )
                        .build();
                    let spec = super::spec();
                    let artifact_dir = super::plans::artifact_dir();
                    ::nirvash_conformance::run_materialized_kani_obligation::<
                        GeneratedSpec,
                        Binding,
                    >(
                        &spec,
                        &super::metadata::spec_metadata(),
                        &profile,
                        binding_name,
                        &artifact_dir,
                        obligation_id,
                        actions,
                    )
                }

                #[macro_export]
                macro_rules! #hidden_all {
                    (binding = $binding:ident $(,)?) => {
                        #[test]
                        fn generated_all_tests() {
                            let plan = $crate::#export_ident::plans::all_for::<$binding>(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                concat!(module_path!(), "::", stringify!($binding)),
                            )
                            .expect("all_tests! plan should pass");
                        }
                    };
                    (binding = $binding:ty $(,)?) => {
                        #[test]
                        fn generated_all_tests() {
                            let plan = $crate::#export_ident::plans::all_for::<$binding>(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                stringify!($binding),
                            )
                            .expect("all_tests! plan should pass");
                        }
                    };
                }

                #[macro_export]
                macro_rules! #hidden_tests {
                    (binding = $binding:ident, profiles = [$($profile:expr),* $(,)?] $(,)?) => {
                        #[test]
                        fn generated_tests() {
                            let plan = $crate::#export_ident::plans::from_builders(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                                vec![$($profile),*],
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                concat!(module_path!(), "::", stringify!($binding)),
                            )
                            .expect("tests! plan should pass");
                        }
                    };
                    (binding = $binding:ty, profiles = [$($profile:expr),* $(,)?] $(,)?) => {
                        #[test]
                        fn generated_tests() {
                            let plan = $crate::#export_ident::plans::from_builders(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                                vec![$($profile),*],
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                stringify!($binding),
                            )
                            .expect("tests! plan should pass");
                        }
                    };
                }

                #[macro_export]
                macro_rules! #hidden_unit {
                    (binding = $binding:ident $(,)?) => {
                        #[test]
                        fn generated_unit_tests() {
                            let plan = $crate::#export_ident::plans::unit(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                concat!(module_path!(), "::", stringify!($binding)),
                            )
                            .expect("unit_tests! plan should pass");
                        }
                    };
                    (binding = $binding:ty $(,)?) => {
                        #[test]
                        fn generated_unit_tests() {
                            let plan = $crate::#export_ident::plans::unit(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                stringify!($binding),
                            )
                            .expect("unit_tests! plan should pass");
                        }
                    };
                }

                #[macro_export]
                macro_rules! #hidden_trace {
                    (binding = $binding:ident $(,)?) => {
                        #[test]
                        fn generated_trace_tests() {
                            let _ = ::nirvash_conformance::require_trace_binding::<crate::#spec_ident, $binding>;
                            let plan = $crate::#export_ident::plans::trace(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                concat!(module_path!(), "::", stringify!($binding)),
                            )
                            .expect("trace_tests! plan should pass");
                        }
                    };
                    (binding = $binding:ty $(,)?) => {
                        #[test]
                        fn generated_trace_tests() {
                            let _ = ::nirvash_conformance::require_trace_binding::<crate::#spec_ident, $binding>;
                            let plan = $crate::#export_ident::plans::trace(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                stringify!($binding),
                            )
                            .expect("trace_tests! plan should pass");
                        }
                    };
                }

                #[macro_export]
                macro_rules! #hidden_kani {
                    (binding = $binding:ident $(,)?) => {
                        #[allow(unexpected_cfgs)]
                        mod generated_kani_harnesses_holder {
                            #[cfg(kani)]
                            mod generated_kani_harnesses {
                                type __NirvashBinding = $binding;
                                const __NIRVASH_BINDING_NAME: &str =
                                    concat!(module_path!(), "::", stringify!($binding));
                                #(#generated_kani_includes)*
                            }
                        }
                    };
                    (binding = $binding:ty $(,)?) => {
                        #[allow(unexpected_cfgs)]
                        mod generated_kani_harnesses_holder {
                            #[cfg(kani)]
                            mod generated_kani_harnesses {
                                type __NirvashBinding = $binding;
                                const __NIRVASH_BINDING_NAME: &str =
                                    stringify!($binding);
                                #(#generated_kani_includes)*
                            }
                        }
                    };
                }

                #[macro_export]
                macro_rules! #hidden_loom {
                    (binding = $binding:ident $(,)?) => {
                        #[allow(unexpected_cfgs)]
                        #[cfg(feature = "loom")]
                        #[test]
                        fn generated_loom_tests() {
                            let _ = ::nirvash_conformance::require_concurrent_binding::<crate::#spec_ident, $binding>;
                            let plan = $crate::#export_ident::plans::loom(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                concat!(module_path!(), "::", stringify!($binding)),
                            )
                            .expect("loom_tests! plan should pass");
                        }
                    };
                    (binding = $binding:ty $(,)?) => {
                        #[allow(unexpected_cfgs)]
                        #[cfg(feature = "loom")]
                        #[test]
                        fn generated_loom_tests() {
                            let _ = ::nirvash_conformance::require_concurrent_binding::<crate::#spec_ident, $binding>;
                            let plan = $crate::#export_ident::plans::loom(
                                <$binding as ::nirvash_conformance::GeneratedBinding<crate::#spec_ident>>::generated_fixture,
                            );
                            $crate::#export_ident::install::__run_selected_plan::<$binding>(
                                plan,
                                stringify!($binding),
                            )
                            .expect("loom_tests! plan should pass");
                        }
                    };
                }

                pub use #hidden_all as all_tests;
                pub use #hidden_kani as kani_harnesses;
                pub use #hidden_loom as loom_tests;
                pub use #hidden_tests as tests;
                pub use #hidden_trace as trace_tests;
                pub use #hidden_unit as unit_tests;
            }
        }

        #export_alias
    })
}

pub fn expand_binding(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream2> {
    let args = syn::parse::<BindingArgs>(attr)?;
    let mut item = syn::parse::<ItemImpl>(item)?;
    ensure_inherent_impl(&item)?;

    let self_ty = item.self_ty.as_ref().clone();
    let spec_ty = args.spec.clone();
    let mut create = None;
    let mut fixture = None;
    let mut project = None;
    let mut project_output = None;
    let mut trace = None;
    let mut actions = Vec::new();
    let mut has_new = false;
    let mut has_builder = false;

    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        if looks_like_zero_arg_constructor(method, "new") {
            has_new = true;
        }
        if looks_like_zero_arg_constructor(method, "builder") {
            has_builder = true;
        }
        let role = take_role(method)?;
        match role {
            Some(MethodRole::Create) => assign_once(&mut create, method.clone(), "create")?,
            Some(MethodRole::Fixture) => assign_once(&mut fixture, method.clone(), "fixture")?,
            Some(MethodRole::Project) => assign_once(&mut project, method.clone(), "project")?,
            Some(MethodRole::ProjectOutput) => {
                assign_once(&mut project_output, method.clone(), "project_output")?
            }
            Some(MethodRole::Trace) => assign_once(&mut trace, method.clone(), "trace")?,
            Some(MethodRole::Action(action)) => {
                actions.push(ActionMethod::from_method(method, action)?)
            }
            None => {}
        }
    }

    if actions.is_empty() {
        return Err(Error::new(
            item.self_ty.span(),
            "nirvash_binding requires at least one #[nirvash(action = ...)] method",
        ));
    }

    let create = CreateMethod::new(create)?;
    let fixture_config =
        FixtureConfig::new(&self_ty, &create, fixture.as_ref(), has_new, has_builder)?;
    let output_ty = actions[0].output_ty.clone();
    for action in &actions[1..] {
        if !same_type(&output_ty, &action.output_ty) {
            return Err(Error::new(
                action.method.sig.output.span(),
                "nirvash_binding action methods must share one output type",
            ));
        }
    }

    let mut error_ty = create.error_ty.clone();
    for action in &actions {
        match (&error_ty, &action.error_ty) {
            (Some(existing), Some(candidate)) if !same_type(existing, candidate) => {
                return Err(Error::new(
                    action.method.sig.output.span(),
                    "nirvash_binding action methods must share one error type",
                ));
            }
            (None, Some(candidate)) => error_ty = Some(candidate.clone()),
            _ => {}
        }
    }
    let error_ty = error_ty.unwrap_or_else(|| syn::parse_quote!(::core::convert::Infallible));
    let create_expr = create.call_expr(&self_ty);
    let fixture_ty = fixture_config.fixture_ty.clone();
    let fixture_expr = fixture_config.fixture_expr();
    let generated_snapshot_fixture_expr = if same_type(&fixture_ty, &self_ty) {
        quote! {
            Err(::nirvash_conformance::HarnessError::Binding(
                "snapshot fixture replay requires an explicit deserializable fixture type"
                    .to_owned(),
            ))
        }
    } else {
        quote! {
            <Self::Fixture as ::nirvash_conformance::SnapshotFixture>::from_snapshot(value)
        }
    };
    let action_ty: Type = syn::parse_quote!(<#spec_ty as ::nirvash_lower::FrontendSpec>::Action);
    let state_ty: Type = syn::parse_quote!(<#spec_ty as ::nirvash_lower::FrontendSpec>::State);
    let expected_output_ty: Type =
        syn::parse_quote!(<#spec_ty as ::nirvash_conformance::SpecOracle>::ExpectedOutput);
    let project_expr = if let Some(project) = project.as_ref() {
        let project_method_ident = &project.sig.ident;
        let project_returns_projected = return_type(&project.sig.output)
            .map(|ty| type_is_projected_state(&ty))
            .unwrap_or(false);
        if project_returns_projected {
            quote! { sut.#project_method_ident() }
        } else {
            quote! { ::nirvash_conformance::ProjectedState::Exact(sut.#project_method_ident()) }
        }
    } else {
        return Err(Error::new(
            item.self_ty.span(),
            "nirvash_binding requires one #[nirvash_project] method",
        ));
    };
    let project_output_expr = if let Some(project_output) = project_output.as_ref() {
        let project_output_ident = &project_output.sig.ident;
        ensure_no_receiver(project_output, "project_output")?;
        quote! { #self_ty::#project_output_ident(action, output) }
    } else {
        quote! {
            <#expected_output_ty as ::core::convert::From<#output_ty>>::from(output.clone())
        }
    };

    let action_arms = actions.iter().map(|action| action.match_arm(&self_ty));
    let action_seed_builders = actions.iter().map(|action| action.seed_builder(&spec_ty));
    let trace_impl = trace
        .as_ref()
        .map(|method| {
            let ident = method.sig.ident.clone();
            quote! {
                impl ::nirvash_conformance::TraceBinding<#spec_ty> for #self_ty {
                    fn record_update(
                        sut: &Self::Sut,
                        output: &Self::Output,
                        sink: &mut dyn ::nirvash_conformance::TraceSink<#spec_ty>,
                    ) {
                        sut.#ident(output, sink);
                    }
                }
            }
        })
        .unwrap_or_default();
    let trace_runner = if trace.is_some() {
        quote! {
            ::nirvash_conformance::run_trace_profile::<#spec_ty, Self>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                materialize_failures,
            )
        }
    } else {
        quote! {
            Err(::nirvash_conformance::HarnessError::Binding(
                "trace validation requires #[nirvash_trace] on the binding".to_owned(),
            ))
        }
    };
    let concurrent_impl = if args.concurrent {
        quote! {
            impl ::nirvash_conformance::ConcurrentBinding<#spec_ty> for #self_ty {}
        }
    } else {
        TokenStream2::new()
    };
    let concurrent_runner = if args.concurrent {
        quote! {
            ::nirvash_conformance::run_concurrent_profile::<#spec_ty, Self>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                materialize_failures,
            )
        }
    } else {
        quote! {
            Err(::nirvash_conformance::HarnessError::Binding(
                "concurrency profiles require #[nirvash_binding(spec = ..., concurrent)]".to_owned(),
            ))
        }
    };
    let concurrent_flag = args.concurrent;
    let trace_flag = trace.is_some();

    Ok(quote! {
        #item

        impl ::nirvash_conformance::RuntimeBinding<#spec_ty> for #self_ty {
            type Sut = #self_ty;
            type Fixture = #fixture_ty;
            type Output = #output_ty;
            type Error = #error_ty;

            fn create(fixture: &Self::Fixture) -> Result<Self::Sut, Self::Error> {
                #create_expr
            }

            fn apply(
                sut: &mut Self::Sut,
                action: &#action_ty,
                env: &mut ::nirvash_conformance::TestEnvironment,
            ) -> Result<Self::Output, Self::Error> {
                let _ = env;
                match action {
                    #(#action_arms)*
                    _ => unreachable!("nirvash_binding generated an incomplete action dispatch"),
                }
            }

            fn project(sut: &Self::Sut) -> ::nirvash_conformance::ProjectedState<#state_ty> {
                #project_expr
            }

            fn project_output(
                action: &#action_ty,
                output: &Self::Output,
            ) -> #expected_output_ty {
                #project_output_expr
            }
        }

        impl ::nirvash_conformance::GeneratedBinding<#spec_ty> for #self_ty {
            fn generated_fixture() -> ::nirvash_conformance::SharedFixtureValue {
                ::std::sync::Arc::new(#fixture_expr)
            }

            fn generated_snapshot_fixture(
                value: &::serde_json::Value,
            ) -> Result<Self::Fixture, ::nirvash_conformance::HarnessError> {
                #generated_snapshot_fixture_expr
            }

            fn generated_action_candidates(
                spec: &#spec_ty,
                seeds: &::nirvash_conformance::SeedProfile<#spec_ty>,
            ) -> Result<
                ::std::vec::Vec<<#spec_ty as ::nirvash_lower::FrontendSpec>::Action>,
                ::nirvash_conformance::HarnessError,
            > {
                let _ = spec;
                let mut actions = ::std::vec::Vec::new();
                #(#action_seed_builders)*
                Ok(actions)
            }

            fn run_generated_profile(
                spec: &#spec_ty,
                metadata: &::nirvash_conformance::GeneratedSpecMetadata,
                profile: &::nirvash_conformance::TestProfile<#spec_ty>,
                binding_name: &str,
                artifact_dir: &::nirvash_conformance::ArtifactDirPolicy,
                materialize_failures: bool,
            ) -> Result<(), ::nirvash_conformance::HarnessError>
            where
                #spec_ty: ::nirvash_lower::TemporalSpec,
                <#spec_ty as ::nirvash_lower::FrontendSpec>::State:
                    Clone
                        + PartialEq
                        + ::nirvash_lower::FiniteModelDomain
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
                <#spec_ty as ::nirvash_lower::FrontendSpec>::Action:
                    Clone
                        + PartialEq
                        + ::core::fmt::Debug
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
                <#spec_ty as ::nirvash_conformance::SpecOracle>::ExpectedOutput:
                    Clone
                        + ::core::fmt::Debug
                        + PartialEq
                        + Eq
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
            {
                ::nirvash_conformance::run_profile::<#spec_ty, Self>(
                    spec,
                    metadata,
                    profile,
                    binding_name,
                    artifact_dir,
                    materialize_failures,
                )
            }

            fn run_generated_trace_profile(
                spec: &#spec_ty,
                metadata: &::nirvash_conformance::GeneratedSpecMetadata,
                profile: &::nirvash_conformance::TestProfile<#spec_ty>,
                binding_name: &str,
                artifact_dir: &::nirvash_conformance::ArtifactDirPolicy,
                materialize_failures: bool,
            ) -> Result<(), ::nirvash_conformance::HarnessError>
            where
                #spec_ty: ::nirvash_lower::TemporalSpec,
                <#spec_ty as ::nirvash_lower::FrontendSpec>::State:
                    Clone
                        + PartialEq
                        + ::nirvash_lower::FiniteModelDomain
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
                <#spec_ty as ::nirvash_lower::FrontendSpec>::Action:
                    Clone
                        + PartialEq
                        + ::core::fmt::Debug
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
                <#spec_ty as ::nirvash_conformance::SpecOracle>::ExpectedOutput:
                    Clone
                        + ::core::fmt::Debug
                        + PartialEq
                        + Eq
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
            {
                #trace_runner
            }

            fn run_generated_concurrent_profile(
                spec: &#spec_ty,
                metadata: &::nirvash_conformance::GeneratedSpecMetadata,
                profile: &::nirvash_conformance::TestProfile<#spec_ty>,
                binding_name: &str,
                artifact_dir: &::nirvash_conformance::ArtifactDirPolicy,
                materialize_failures: bool,
            ) -> Result<(), ::nirvash_conformance::HarnessError>
            where
                #spec_ty: ::nirvash_lower::FrontendSpec,
                <#spec_ty as ::nirvash_lower::FrontendSpec>::State:
                    Clone
                        + PartialEq
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
                <#spec_ty as ::nirvash_lower::FrontendSpec>::Action:
                    Clone
                        + PartialEq
                        + ::core::fmt::Debug
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
                <#spec_ty as ::nirvash_conformance::SpecOracle>::ExpectedOutput:
                    Clone
                        + ::core::fmt::Debug
                        + PartialEq
                        + Eq
                        + ::serde::Serialize
                        + ::serde::de::DeserializeOwned
                        + Send
                        + Sync
                        + 'static,
            {
                #concurrent_runner
            }

            fn supports_trace() -> bool {
                #trace_flag
            }

            fn supports_concurrency() -> bool {
                #concurrent_flag
            }
        }

        #trace_impl
        #concurrent_impl
    })
}

fn spec_ident(item: &Item) -> syn::Result<&Ident> {
    match item {
        Item::Struct(ItemStruct { ident, .. })
        | Item::Enum(ItemEnum { ident, .. })
        | Item::Type(ItemType { ident, .. }) => Ok(ident),
        _ => Err(Error::new(
            item.span(),
            "code_tests must be attached to a named spec item",
        )),
    }
}

fn ensure_non_generic_spec(item: &Item) -> syn::Result<()> {
    let generics = match item {
        Item::Struct(ItemStruct { generics, .. })
        | Item::Enum(ItemEnum { generics, .. })
        | Item::Type(ItemType { generics, .. }) => generics,
        _ => return Ok(()),
    };
    if generics.params.is_empty() {
        Ok(())
    } else {
        Err(Error::new(
            generics.span(),
            "code_tests does not support generic spec items",
        ))
    }
}

fn ensure_inherent_impl(item: &ItemImpl) -> syn::Result<()> {
    if item.trait_.is_some() || !item.generics.params.is_empty() {
        return Err(Error::new(
            item.span(),
            "nirvash_binding supports only non-generic inherent impl blocks",
        ));
    }
    Ok(())
}

fn assign_once<T>(slot: &mut Option<T>, value: T, label: &str) -> syn::Result<()> {
    if slot.is_some() {
        return Err(Error::new(
            Span::call_site(),
            format!("duplicate nirvash binding helper for {label}"),
        ));
    }
    *slot = Some(value);
    Ok(())
}

fn take_role(method: &mut ImplItemFn) -> syn::Result<Option<MethodRole>> {
    let mut role = None;
    let mut kept = Vec::new();
    for attr in method.attrs.drain(..) {
        let next_role = if attr.path().is_ident("nirvash") {
            Some(attr.parse_args::<MethodRoleAttr>()?.role)
        } else if attr.path().is_ident("nirvash_fixture") {
            Some(MethodRole::Fixture)
        } else if attr.path().is_ident("nirvash_project") {
            Some(MethodRole::Project)
        } else if attr.path().is_ident("nirvash_project_output") {
            Some(MethodRole::ProjectOutput)
        } else if attr.path().is_ident("nirvash_trace") {
            Some(MethodRole::Trace)
        } else {
            None
        };
        if let Some(parsed_role) = next_role {
            if role.is_some() {
                return Err(Error::new(
                    attr.span(),
                    "each method may carry at most one nirvash binding marker",
                ));
            }
            role = Some(parsed_role);
        } else {
            kept.push(attr);
        }
    }
    method.attrs = kept;
    Ok(role)
}

fn ensure_no_receiver(method: &ImplItemFn, label: &str) -> syn::Result<()> {
    if method.sig.receiver().is_some() {
        return Err(Error::new(
            method.sig.span(),
            format!("#{label} helper methods must not take self"),
        ));
    }
    Ok(())
}

fn ensure_zero_args(method: &ImplItemFn, label: &str) -> syn::Result<()> {
    if !method.sig.inputs.is_empty() {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("#{label} helper methods must not take arguments"),
        ));
    }
    Ok(())
}

fn return_type(output: &ReturnType) -> Option<Type> {
    match output {
        ReturnType::Default => None,
        ReturnType::Type(_, ty) => Some((**ty).clone()),
    }
}

fn same_type(lhs: &Type, rhs: &Type) -> bool {
    lhs.to_token_stream().to_string() == rhs.to_token_stream().to_string()
}

fn type_is_projected_state(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "ProjectedState"),
        _ => false,
    }
}

fn result_parts(ty: &Type) -> Option<(Type, Type)> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let mut args = arguments.args.iter();
    let GenericArgument::Type(ok_ty) = args.next()? else {
        return None;
    };
    let GenericArgument::Type(err_ty) = args.next()? else {
        return None;
    };
    Some((ok_ty.clone(), err_ty.clone()))
}

fn fn_arg_ident(arg: &FnArg) -> syn::Result<Ident> {
    match arg {
        FnArg::Typed(typed) => match typed.pat.as_ref() {
            Pat::Ident(PatIdent { ident, .. }) => Ok(ident.clone()),
            _ => Err(Error::new(
                typed.pat.span(),
                "nirvash_binding action arguments must be simple identifiers",
            )),
        },
        FnArg::Receiver(receiver) => Err(Error::new(
            receiver.span(),
            "receiver is not a positional action argument",
        )),
    }
}

fn fn_arg_type(arg: &FnArg) -> syn::Result<Type> {
    match arg {
        FnArg::Typed(typed) => Ok((*typed.ty).clone()),
        FnArg::Receiver(receiver) => Err(Error::new(
            receiver.span(),
            "receiver is not a positional action argument",
        )),
    }
}

fn looks_like_zero_arg_constructor(method: &ImplItemFn, expected: &str) -> bool {
    method.sig.ident == expected
        && method.sig.receiver().is_none()
        && method.sig.inputs.is_empty()
        && return_type(&method.sig.output).is_some()
}

#[derive(Clone)]
struct CreateMethod {
    ident: Ident,
    fixture_ty: Type,
    takes_ref: bool,
    returns_result: bool,
    error_ty: Option<Type>,
}

impl CreateMethod {
    fn new(method: Option<ImplItemFn>) -> syn::Result<Self> {
        let Some(method) = method else {
            return Ok(Self {
                ident: Ident::new("default", Span::call_site()),
                fixture_ty: syn::parse_quote!(()),
                takes_ref: true,
                returns_result: true,
                error_ty: None,
            });
        };
        ensure_no_receiver(&method, "create")?;
        if method.sig.inputs.len() != 1 {
            return Err(Error::new(
                method.sig.inputs.span(),
                "#create methods must take exactly one fixture argument",
            ));
        }
        let fixture_arg = method.sig.inputs.first().expect("fixture arg");
        let fixture_ty = match fixture_arg {
            FnArg::Typed(typed) => match typed.ty.as_ref() {
                Type::Reference(reference) => (*reference.elem).clone(),
                other => other.clone(),
            },
            FnArg::Receiver(receiver) => {
                return Err(Error::new(receiver.span(), "#create must not take self"));
            }
        };
        let takes_ref = matches!(fixture_arg, FnArg::Typed(typed) if matches!(typed.ty.as_ref(), Type::Reference(_)));
        let Some(ret_ty) = return_type(&method.sig.output) else {
            return Err(Error::new(
                method.sig.output.span(),
                "#create methods must return Self or Result<Self, Error>",
            ));
        };
        let (returns_result, error_ty) = if let Some((_ok, err)) = result_parts(&ret_ty) {
            (true, Some(err))
        } else {
            (false, None)
        };
        Ok(Self {
            ident: method.sig.ident,
            fixture_ty,
            takes_ref,
            returns_result,
            error_ty,
        })
    }

    fn call_expr(&self, self_ty: &Type) -> TokenStream2 {
        if self.ident == "default" {
            return quote! { Ok(fixture.clone()) };
        }
        let ident = &self.ident;
        let arg = if self.takes_ref {
            quote! { fixture }
        } else {
            quote! { fixture.clone() }
        };
        if self.returns_result {
            quote! { #self_ty::#ident(#arg) }
        } else {
            quote! { Ok(#self_ty::#ident(#arg)) }
        }
    }
}

struct FixtureConfig {
    fixture_ty: Type,
    fixture_expr: TokenStream2,
}

impl FixtureConfig {
    fn default_then(fixture_ty: &Type, fallback: TokenStream2) -> TokenStream2 {
        quote! {{
            struct __NirvashDefaultProbe<T>(::core::marker::PhantomData<T>);

            trait __NirvashMaybeDefault<T> {
                fn maybe_default(self) -> ::core::option::Option<T>;
            }

            impl<T> __NirvashMaybeDefault<T> for &__NirvashDefaultProbe<T>
            where
                T: ::core::default::Default,
            {
                fn maybe_default(self) -> ::core::option::Option<T> {
                    let _ = self;
                    ::core::option::Option::Some(<T as ::core::default::Default>::default())
                }
            }

            impl<T> __NirvashMaybeDefault<T> for &&__NirvashDefaultProbe<T> {
                fn maybe_default(self) -> ::core::option::Option<T> {
                    let _ = self;
                    ::core::option::Option::None
                }
            }

            (&__NirvashDefaultProbe::<#fixture_ty>(::core::marker::PhantomData))
                .maybe_default()
                .unwrap_or_else(|| #fallback)
        }}
    }

    fn new(
        self_ty: &Type,
        create: &CreateMethod,
        fixture_method: Option<&ImplItemFn>,
        has_new: bool,
        has_builder: bool,
    ) -> syn::Result<Self> {
        if let Some(fixture_method) = fixture_method {
            ensure_no_receiver(fixture_method, "fixture")?;
            ensure_zero_args(fixture_method, "fixture")?;
            let fixture_ty = return_type(&fixture_method.sig.output).ok_or_else(|| {
                Error::new(
                    fixture_method.sig.output.span(),
                    "#[nirvash_fixture] must return a fixture value",
                )
            })?;
            if create.ident == "default" && !same_type(&fixture_ty, self_ty) {
                return Err(Error::new(
                    fixture_ty.span(),
                    "custom fixture types require an explicit #[nirvash(create)] helper",
                ));
            }
            let ident = &fixture_method.sig.ident;
            return Ok(Self {
                fixture_ty,
                fixture_expr: quote! { #self_ty::#ident() },
            });
        }

        if create.ident != "default" {
            let fixture_ty = create.fixture_ty.clone();
            let fixture_expr = if same_type(&fixture_ty, self_ty) && has_new {
                Self::default_then(&fixture_ty, quote! { #self_ty::new() })
            } else if same_type(&fixture_ty, self_ty) && has_builder {
                Self::default_then(&fixture_ty, quote! { #self_ty::builder().build() })
            } else {
                quote! { <#fixture_ty as ::core::default::Default>::default() }
            };
            return Ok(Self {
                fixture_ty,
                fixture_expr,
            });
        }

        if has_new {
            return Ok(Self {
                fixture_ty: self_ty.clone(),
                fixture_expr: Self::default_then(self_ty, quote! { #self_ty::new() }),
            });
        }
        if has_builder {
            return Ok(Self {
                fixture_ty: self_ty.clone(),
                fixture_expr: Self::default_then(self_ty, quote! { #self_ty::builder().build() }),
            });
        }

        Ok(Self {
            fixture_ty: self_ty.clone(),
            fixture_expr: quote! { <#self_ty as ::core::default::Default>::default() },
        })
    }

    fn fixture_expr(&self) -> TokenStream2 {
        self.fixture_expr.clone()
    }
}

struct ActionMethod {
    method: ImplItemFn,
    action: Path,
    arg_idents: Vec<Ident>,
    arg_types: Vec<Type>,
    output_ty: Type,
    error_ty: Option<Type>,
    returns_result: bool,
}

impl ActionMethod {
    fn from_method(method: &ImplItemFn, action: Path) -> syn::Result<Self> {
        if method.sig.receiver().is_none() {
            return Err(Error::new(
                method.sig.span(),
                "#[nirvash(action = ...)] methods must take self",
            ));
        }
        let mut arg_idents = Vec::new();
        let mut arg_types = Vec::new();
        for arg in method.sig.inputs.iter().skip(1) {
            arg_idents.push(fn_arg_ident(arg)?);
            arg_types.push(fn_arg_type(arg)?);
        }
        let ret_ty = return_type(&method.sig.output).ok_or_else(|| {
            Error::new(
                method.sig.output.span(),
                "#[nirvash(action = ...)] methods must return an output value",
            )
        })?;
        let (output_ty, error_ty, returns_result) =
            if let Some((ok_ty, err_ty)) = result_parts(&ret_ty) {
                (ok_ty, Some(err_ty), true)
            } else {
                (ret_ty, None, false)
            };
        Ok(Self {
            method: method.clone(),
            action,
            arg_idents,
            arg_types,
            output_ty,
            error_ty,
            returns_result,
        })
    }

    fn match_arm(&self, self_ty: &Type) -> TokenStream2 {
        let ident = &self.method.sig.ident;
        let pattern = if self.arg_idents.is_empty() {
            let action = &self.action;
            quote! { #action }
        } else {
            let action = &self.action;
            let vars = &self.arg_idents;
            quote! { #action(#(#vars),*) }
        };
        let args = self
            .arg_idents
            .iter()
            .map(|ident| quote! { #ident.clone() });
        if self.returns_result {
            quote! {
                #pattern => {
                    return #self_ty::#ident(sut, #(#args),*);
                }
            }
        } else {
            quote! {
                #pattern => {
                    return Ok(#self_ty::#ident(sut, #(#args),*));
                }
            }
        }
    }

    fn seed_builder(&self, spec_ty: &Path) -> TokenStream2 {
        let ident = &self.method.sig.ident;
        let action = &self.action;
        let action_label = action.to_token_stream().to_string();
        if self.arg_idents.is_empty() {
            return quote! {
                {
                    let mut seeded = false;
                    if let Some(values) = seeds.actions.get(stringify!(#ident)) {
                        for value in values {
                            ::nirvash_conformance::push_unique_action(&mut actions, value.clone());
                        }
                        seeded = true;
                    }
                    if !seeded {
                        if let Some(values) = seeds.actions.get(#action_label) {
                            for value in values {
                                ::nirvash_conformance::push_unique_action(&mut actions, value.clone());
                            }
                            seeded = true;
                        }
                    }
                    if !seeded {
                        ::nirvash_conformance::push_unique_action(&mut actions, #action);
                    }
                }
            };
        }

        let value_bindings = self
            .arg_types
            .iter()
            .enumerate()
            .map(|(index, ty)| {
                let values_ident = format_ident!("__nirvash_seed_values_{index}");
                quote! {
                    let #values_ident =
                        ::nirvash_conformance::monomorphic_typed_seed_candidates!(#ty, #spec_ty, seeds);
                }
            })
            .collect::<Vec<_>>();

        let nested = self.seed_loop_body(0);
        quote! {
            {
                let mut seeded = false;
                if let Some(values) = seeds.actions.get(stringify!(#ident)) {
                    for value in values {
                        ::nirvash_conformance::push_unique_action(&mut actions, value.clone());
                    }
                    seeded = true;
                }
                if !seeded {
                    if let Some(values) = seeds.actions.get(#action_label) {
                        for value in values {
                            ::nirvash_conformance::push_unique_action(&mut actions, value.clone());
                        }
                        seeded = true;
                    }
                }
                if !seeded {
                    #(#value_bindings)*
                    #nested
                }
            }
        }
    }

    fn seed_loop_body(&self, index: usize) -> TokenStream2 {
        if index == self.arg_idents.len() {
            let action = &self.action;
            let args = (0..self.arg_idents.len())
                .map(|position| format_ident!("__nirvash_seed_{position}"))
                .collect::<Vec<_>>();
            return quote! {
                ::nirvash_conformance::push_unique_action(
                    &mut actions,
                    #action(#(#args.clone()),*),
                );
            };
        }

        let values_ident = format_ident!("__nirvash_seed_values_{index}");
        let value_ident = format_ident!("__nirvash_seed_{index}");
        let nested = self.seed_loop_body(index + 1);
        quote! {
            for #value_ident in &#values_ident {
                #nested
            }
        }
    }
}

#[derive(Clone)]
struct BindingArgs {
    spec: Path,
    concurrent: bool,
}

impl Parse for BindingArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut spec = None;
        let mut concurrent = false;
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if ident == "spec" {
                let _eq: Token![=] = input.parse()?;
                spec = Some(input.parse()?);
            } else if ident == "concurrent" {
                concurrent = true;
            } else {
                return Err(Error::new(
                    ident.span(),
                    "unsupported nirvash_binding argument",
                ));
            }
            if input.peek(Token![,]) {
                let _ = input.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            spec: spec.ok_or_else(|| Error::new(Span::call_site(), "missing spec = ..."))?,
            concurrent,
        })
    }
}

enum MethodRole {
    Create,
    Fixture,
    Project,
    ProjectOutput,
    Trace,
    Action(Path),
}

struct MethodRoleAttr {
    role: MethodRole,
}

impl Parse for MethodRoleAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        let role = match ident.to_string().as_str() {
            "create" => MethodRole::Create,
            "fixture" => {
                return Err(Error::new(
                    ident.span(),
                    "use #[nirvash_fixture] instead of #[nirvash(fixture)]",
                ));
            }
            "project" => {
                return Err(Error::new(
                    ident.span(),
                    "use #[nirvash_project] instead of #[nirvash(project)]",
                ));
            }
            "project_output" => {
                return Err(Error::new(
                    ident.span(),
                    "use #[nirvash_project_output] instead of #[nirvash(project_output)]",
                ));
            }
            "trace" => {
                return Err(Error::new(
                    ident.span(),
                    "use #[nirvash_trace] instead of #[nirvash(trace)]",
                ));
            }
            "action" => {
                let _eq: Token![=] = input.parse()?;
                MethodRole::Action(input.parse()?)
            }
            _ => {
                return Err(Error::new(
                    ident.span(),
                    "unsupported nirvash helper attribute",
                ));
            }
        };
        Ok(Self { role })
    }
}

#[derive(Clone, Default)]
struct CodeTestsArgs {
    export: Option<Ident>,
    models: Option<Vec<Ident>>,
    profiles: Option<Vec<ProfileDef>>,
}

impl Parse for CodeTestsArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "spec" | "binding" => {
                    return Err(Error::new(
                        ident.span(),
                        "code_tests no longer accepts legacy `spec`/`binding` arguments; attach #[code_tests] to the spec item and configure runtime bindings with #[nirvash_binding(spec = ...)]",
                    ));
                }
                "export" => {
                    let _eq: Token![=] = input.parse()?;
                    args.export = Some(input.parse()?);
                }
                "models" => {
                    let _eq: Token![=] = input.parse()?;
                    let content;
                    bracketed!(content in input);
                    let mut models = Vec::new();
                    while !content.is_empty() {
                        models.push(content.parse()?);
                        if content.peek(Token![,]) {
                            let _ = content.parse::<Token![,]>()?;
                        }
                    }
                    args.models = Some(models);
                }
                "profiles" => {
                    let _eq: Token![=] = input.parse()?;
                    let content;
                    bracketed!(content in input);
                    let mut profiles = Vec::new();
                    while !content.is_empty() {
                        profiles.push(content.parse()?);
                        if content.peek(Token![,]) {
                            let _ = content.parse::<Token![,]>()?;
                        }
                    }
                    args.profiles = Some(profiles);
                }
                other => {
                    return Err(Error::new(
                        ident.span(),
                        format!("unsupported code_tests argument `{other}`"),
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

struct ImportGeneratedTestsArgs {
    spec: Path,
    binding: Type,
    profiles: Option<Vec<Expr>>,
}

impl Parse for ImportGeneratedTestsArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut spec = None;
        let mut binding = None;
        let mut profiles = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "spec" => {
                    let _eq: Token![=] = input.parse()?;
                    spec = Some(input.parse()?);
                }
                "binding" => {
                    let _eq: Token![=] = input.parse()?;
                    binding = Some(input.parse()?);
                }
                "profiles" => {
                    let _eq: Token![=] = input.parse()?;
                    let content;
                    bracketed!(content in input);
                    let mut values = Vec::new();
                    while !content.is_empty() {
                        values.push(content.parse()?);
                        if content.peek(Token![,]) {
                            let _ = content.parse::<Token![,]>()?;
                        }
                    }
                    profiles = Some(values);
                }
                other => {
                    return Err(Error::new(
                        ident.span(),
                        format!("unsupported import_generated_tests argument `{other}`"),
                    ));
                }
            }

            if input.peek(Token![,]) {
                let _ = input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            spec: spec.ok_or_else(|| Error::new(Span::call_site(), "missing spec = ..."))?,
            binding: binding
                .ok_or_else(|| Error::new(Span::call_site(), "missing binding = ..."))?,
            profiles,
        })
    }
}

fn generated_module_path(spec: &Path) -> syn::Result<Path> {
    let mut path = spec.clone();
    if path.segments.pop().is_none() {
        return Err(Error::new(
            spec.span(),
            "spec path must name a concrete spec item",
        ));
    }
    path.segments.push(PathSegment::from(Ident::new(
        "generated",
        Span::call_site(),
    )));
    Ok(path)
}

#[derive(Clone)]
struct ProfileDef {
    name: Ident,
    coverage: Vec<CoverageDef>,
    engines: Vec<EngineDef>,
}

impl Parse for ProfileDef {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let _eq: Token![=] = input.parse()?;
        let content;
        braced!(content in input);
        let mut coverage = None;
        let mut engines = None;
        while !content.is_empty() {
            let ident: Ident = content.parse()?;
            match ident.to_string().as_str() {
                "coverage" => {
                    let _eq: Token![=] = content.parse()?;
                    let items;
                    bracketed!(items in content);
                    let mut values = Vec::new();
                    while !items.is_empty() {
                        values.push(items.parse()?);
                        if items.peek(Token![,]) {
                            let _ = items.parse::<Token![,]>()?;
                        }
                    }
                    coverage = Some(values);
                }
                "engines" => {
                    let _eq: Token![=] = content.parse()?;
                    let items;
                    bracketed!(items in content);
                    let mut values = Vec::new();
                    while !items.is_empty() {
                        values.push(items.parse()?);
                        if items.peek(Token![,]) {
                            let _ = items.parse::<Token![,]>()?;
                        }
                    }
                    engines = Some(values);
                }
                other => {
                    return Err(Error::new(
                        ident.span(),
                        format!("unsupported profile field `{other}`"),
                    ));
                }
            }
            if content.peek(Token![,]) {
                let _ = content.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            name,
            coverage: coverage.unwrap_or_default(),
            engines: engines.unwrap_or_default(),
        })
    }
}

#[derive(Clone)]
enum CoverageDef {
    Transitions,
    TransitionPairs(usize),
    GuardBoundaries,
    PropertyPrefixes,
    Goal(String),
}

impl Parse for CoverageDef {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        match ident.to_string().as_str() {
            "transitions" => Ok(Self::Transitions),
            "guard_boundaries" => Ok(Self::GuardBoundaries),
            "property_prefixes" => Ok(Self::PropertyPrefixes),
            "transition_pairs" => {
                let content;
                syn::parenthesized!(content in input);
                let value = parse_usize_lit(&content.parse::<Lit>()?)?;
                Ok(Self::TransitionPairs(value))
            }
            "goal" => {
                let content;
                syn::parenthesized!(content in input);
                let literal = content.parse::<Lit>()?;
                let Lit::Str(string) = literal else {
                    return Err(Error::new(
                        literal.span(),
                        "goal(...) expects a string literal",
                    ));
                };
                Ok(Self::Goal(string.value()))
            }
            other => Err(Error::new(
                ident.span(),
                format!("unsupported coverage goal `{other}`"),
            )),
        }
    }
}

impl CoverageDef {
    fn tokens(&self) -> TokenStream2 {
        match self {
            Self::Transitions => quote! { ::nirvash_conformance::CoverageGoal::Transitions },
            Self::TransitionPairs(width) => {
                quote! { ::nirvash_conformance::CoverageGoal::TransitionPairs(#width) }
            }
            Self::GuardBoundaries => {
                quote! { ::nirvash_conformance::CoverageGoal::GuardBoundaries }
            }
            Self::PropertyPrefixes => {
                quote! { ::nirvash_conformance::CoverageGoal::PropertyPrefixes }
            }
            Self::Goal(label) => quote! { ::nirvash_conformance::CoverageGoal::Goal(#label) },
        }
    }
}

#[derive(Clone)]
enum EngineDef {
    ExplicitSuite,
    ProptestOnline {
        cases: usize,
        max_steps: usize,
    },
    KaniBounded {
        depth: usize,
    },
    TraceValidation,
    LoomSmall {
        threads: usize,
        max_permutations: usize,
    },
    ShuttlePCT {
        depth: usize,
        runs: usize,
    },
}

impl Parse for EngineDef {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        match ident.to_string().as_str() {
            "explicit_suite" => Ok(Self::ExplicitSuite),
            "trace_validation" => Ok(Self::TraceValidation),
            "proptest_online" => {
                let content;
                syn::parenthesized!(content in input);
                let mut cases = None;
                let mut max_steps = None;
                while !content.is_empty() {
                    let key: Ident = content.parse()?;
                    let _eq: Token![=] = content.parse()?;
                    match key.to_string().as_str() {
                        "cases" => cases = Some(parse_expr_usize(&content.parse::<Expr>()?)?),
                        "steps" => max_steps = Some(parse_steps_expr(&content.parse::<Expr>()?)?),
                        other => {
                            return Err(Error::new(
                                key.span(),
                                format!("unsupported proptest_online field `{other}`"),
                            ));
                        }
                    }
                    if content.peek(Token![,]) {
                        let _ = content.parse::<Token![,]>()?;
                    }
                }
                Ok(Self::ProptestOnline {
                    cases: cases.unwrap_or(4096),
                    max_steps: max_steps.unwrap_or(32),
                })
            }
            "kani" => {
                let content;
                syn::parenthesized!(content in input);
                let key: Ident = content.parse()?;
                if key != "depth" {
                    return Err(Error::new(key.span(), "kani(...) expects depth = <int>"));
                }
                let _eq: Token![=] = content.parse()?;
                let depth = parse_expr_usize(&content.parse::<Expr>()?)?;
                Ok(Self::KaniBounded { depth })
            }
            "loom_small" => {
                let content;
                syn::parenthesized!(content in input);
                let mut threads = 2;
                let mut max_permutations = 8;
                while !content.is_empty() {
                    let key: Ident = content.parse()?;
                    let _eq: Token![=] = content.parse()?;
                    match key.to_string().as_str() {
                        "threads" => threads = parse_expr_usize(&content.parse::<Expr>()?)?,
                        "max_permutations" => {
                            max_permutations = parse_expr_usize(&content.parse::<Expr>()?)?
                        }
                        other => {
                            return Err(Error::new(
                                key.span(),
                                format!("unsupported loom_small field `{other}`"),
                            ));
                        }
                    }
                    if content.peek(Token![,]) {
                        let _ = content.parse::<Token![,]>()?;
                    }
                }
                Ok(Self::LoomSmall {
                    threads,
                    max_permutations,
                })
            }
            "shuttle_pct" => {
                let content;
                syn::parenthesized!(content in input);
                let mut depth = None;
                let mut runs = None;
                while !content.is_empty() {
                    let key: Ident = content.parse()?;
                    let _eq: Token![=] = content.parse()?;
                    match key.to_string().as_str() {
                        "depth" => depth = Some(parse_expr_usize(&content.parse::<Expr>()?)?),
                        "runs" => runs = Some(parse_expr_usize(&content.parse::<Expr>()?)?),
                        other => {
                            return Err(Error::new(
                                key.span(),
                                format!("unsupported shuttle_pct field `{other}`"),
                            ));
                        }
                    }
                    if content.peek(Token![,]) {
                        let _ = content.parse::<Token![,]>()?;
                    }
                }
                Ok(Self::ShuttlePCT {
                    depth: depth.unwrap_or(2),
                    runs: runs.unwrap_or(2000),
                })
            }
            other => Err(Error::new(
                ident.span(),
                format!("unsupported engine `{other}`"),
            )),
        }
    }
}

impl EngineDef {
    fn tokens(&self) -> TokenStream2 {
        match self {
            Self::ExplicitSuite => quote! { ::nirvash_conformance::EnginePlan::ExplicitSuite },
            Self::ProptestOnline { cases, max_steps } => quote! {
                ::nirvash_conformance::EnginePlan::ProptestOnline {
                    cases: #cases,
                    max_steps: #max_steps,
                }
            },
            Self::KaniBounded { depth } => {
                quote! { ::nirvash_conformance::EnginePlan::KaniBounded { depth: #depth } }
            }
            Self::TraceValidation => quote! {
                ::nirvash_conformance::EnginePlan::TraceValidation {
                    engine: ::nirvash_conformance::TraceValidationEngine::Explicit,
                }
            },
            Self::LoomSmall {
                threads,
                max_permutations,
            } => quote! {
                ::nirvash_conformance::EnginePlan::LoomSmall {
                    threads: #threads,
                    max_permutations: #max_permutations,
                }
            },
            Self::ShuttlePCT { depth, runs } => quote! {
                ::nirvash_conformance::EnginePlan::ShuttlePCT {
                    depth: #depth,
                    runs: #runs,
                }
            },
        }
    }
}

#[derive(Clone)]
struct ResolvedProfiles {
    ordered: Vec<ProfileDef>,
}

fn resolve_profiles(custom: Option<Vec<ProfileDef>>) -> ResolvedProfiles {
    let mut ordered = vec![
        default_profile_def("smoke_default"),
        default_profile_def("unit_default"),
        default_profile_def("boundary_default"),
        default_profile_def("e2e_default"),
        default_profile_def("concurrency_default"),
    ];
    if let Some(custom_profiles) = custom {
        for profile in custom_profiles {
            if let Some(existing) = ordered
                .iter_mut()
                .find(|candidate| candidate.name == profile.name)
            {
                *existing = profile;
            } else {
                ordered.push(profile);
            }
        }
    }
    ResolvedProfiles { ordered }
}

fn default_profile_def(name: &str) -> ProfileDef {
    let ident = Ident::new(name, Span::call_site());
    match name {
        "smoke_default" => ProfileDef {
            name: ident,
            coverage: vec![CoverageDef::Transitions],
            engines: vec![EngineDef::ExplicitSuite],
        },
        "unit_default" => ProfileDef {
            name: ident,
            coverage: vec![
                CoverageDef::Transitions,
                CoverageDef::TransitionPairs(2),
                CoverageDef::GuardBoundaries,
            ],
            engines: vec![
                EngineDef::ExplicitSuite,
                EngineDef::ProptestOnline {
                    cases: 4096,
                    max_steps: 32,
                },
                EngineDef::KaniBounded { depth: 4 },
            ],
        },
        "boundary_default" => ProfileDef {
            name: ident,
            coverage: vec![CoverageDef::GuardBoundaries],
            engines: vec![
                EngineDef::ExplicitSuite,
                EngineDef::ProptestOnline {
                    cases: 512,
                    max_steps: 8,
                },
            ],
        },
        "e2e_default" => ProfileDef {
            name: ident,
            coverage: vec![CoverageDef::PropertyPrefixes],
            engines: vec![EngineDef::TraceValidation],
        },
        "concurrency_default" => ProfileDef {
            name: ident,
            coverage: vec![CoverageDef::Transitions, CoverageDef::TransitionPairs(2)],
            engines: vec![
                EngineDef::LoomSmall {
                    threads: 2,
                    max_permutations: 8,
                },
                EngineDef::ShuttlePCT {
                    depth: 2,
                    runs: 2000,
                },
            ],
        },
        _ => ProfileDef {
            name: ident,
            coverage: vec![CoverageDef::Transitions],
            engines: vec![EngineDef::ExplicitSuite],
        },
    }
}

fn default_model_names() -> Vec<Ident> {
    vec![
        Ident::new("small", Span::call_site()),
        Ident::new("boundary", Span::call_site()),
        Ident::new("e2e_default", Span::call_site()),
    ]
}

fn sanitize_file_component(raw: &str) -> String {
    raw.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch,
            _ => '_',
        })
        .collect()
}

fn profile_has_kani(profile: &ProfileDef) -> bool {
    profile
        .engines
        .iter()
        .any(|engine| matches!(engine, EngineDef::KaniBounded { .. }))
}

fn profile_seed_tokens(profile: &ProfileDef, spec_ident: &Ident) -> TokenStream2 {
    let _ = spec_ident;
    let name = profile.name.to_string();
    let has_concurrency = profile.engines.iter().any(|engine| {
        matches!(
            engine,
            EngineDef::LoomSmall { .. } | EngineDef::ShuttlePCT { .. }
        )
    });
    let has_trace = profile
        .engines
        .iter()
        .any(|engine| matches!(engine, EngineDef::TraceValidation));
    if has_concurrency || name == "concurrency_default" {
        quote! { super::seeds::concurrent_small() }
    } else if has_trace || name == "e2e_default" {
        quote! { super::seeds::e2e_default() }
    } else if name.contains("boundary") {
        quote! { super::seeds::boundary() }
    } else {
        quote! { super::seeds::small() }
    }
}

fn profile_fn_tokens(
    profile: &ProfileDef,
    spec_ident: &Ident,
    model_labels: &[String],
) -> TokenStream2 {
    let ident = &profile.name;
    let label = ident.to_string();
    let coverage = profile.coverage.iter().map(CoverageDef::tokens);
    let engines = profile.engines.iter().map(EngineDef::tokens);
    let seed = profile_seed_tokens(profile, spec_ident);
    let model_label = profile_model_label(profile, model_labels);
    quote! {
        pub fn #ident() -> ::nirvash_conformance::TestProfileBuilder<GeneratedSpec> {
            ::nirvash_conformance::TestProfileBuilder::new(
                #label,
                super::model_instance_for(&super::spec(), #model_label),
                #seed,
            )
            .coverage([#(#coverage),*])
            .engines([#(#engines),*])
        }
    }
}

pub fn expand_import_generated_tests(input: TokenStream) -> syn::Result<TokenStream2> {
    let args = syn::parse::<ImportGeneratedTestsArgs>(input)?;
    let spec = args.spec;
    let binding = args.binding;
    let generated_path = generated_module_path(&spec)?;
    let module_name = format_ident!(
        "__nirvash_import_generated_tests_{}",
        sanitize_file_component(
            &spec
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("_"),
        )
        .to_lowercase()
    );

    let body = if let Some(profiles) = args.profiles {
        quote! {
            #[test]
            fn generated_tests() {
                let plan = #generated_path::plans::from_builders(
                    <#binding as ::nirvash_conformance::GeneratedBinding<#spec>>::generated_fixture,
                    vec![#(#profiles),*],
                );
                #generated_path::install::__run_selected_plan::<#binding>(
                    plan,
                    stringify!(#binding),
                )
                .expect("tests! plan should pass");
            }
        }
    } else {
        quote! {
            #[test]
            fn generated_all_tests() {
                let plan = #generated_path::plans::all_for::<#binding>(
                    <#binding as ::nirvash_conformance::GeneratedBinding<#spec>>::generated_fixture,
                );
                #generated_path::install::__run_selected_plan::<#binding>(
                    plan,
                    stringify!(#binding),
                )
                .expect("all_tests! plan should pass");
            }
        }
    };

    Ok(quote! {
        const _: fn() = || {
            let _: ::core::marker::PhantomData<#spec> = #generated_path::__spec_marker();
        };

        #[allow(non_snake_case)]
        mod #module_name {
            #body
        }
    })
}

fn profile_model_label<'a>(profile: &ProfileDef, model_labels: &'a [String]) -> &'a str {
    let first = model_labels.first().map(String::as_str).unwrap_or("small");
    let second = model_labels.get(1).map(String::as_str).unwrap_or(first);
    let third = model_labels.get(2).map(String::as_str).unwrap_or(second);
    let name = profile.name.to_string();
    let has_concurrency = profile.engines.iter().any(|engine| {
        matches!(
            engine,
            EngineDef::LoomSmall { .. } | EngineDef::ShuttlePCT { .. }
        )
    });
    let has_trace = profile
        .engines
        .iter()
        .any(|engine| matches!(engine, EngineDef::TraceValidation));

    if has_trace || name == "e2e_default" {
        third
    } else if name.contains("boundary") || has_concurrency {
        second
    } else {
        first
    }
}

fn parse_expr_usize(expr: &Expr) -> syn::Result<usize> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(value),
            ..
        }) => value.base10_parse(),
        _ => Err(Error::new(expr.span(), "expected an integer literal")),
    }
}

fn parse_steps_expr(expr: &Expr) -> syn::Result<usize> {
    match expr {
        Expr::Range(ExprRange { end: Some(end), .. }) => parse_expr_usize(end),
        _ => parse_expr_usize(expr),
    }
}

fn parse_usize_lit(literal: &Lit) -> syn::Result<usize> {
    match literal {
        Lit::Int(value) => value.base10_parse(),
        _ => Err(Error::new(literal.span(), "expected an integer literal")),
    }
}
