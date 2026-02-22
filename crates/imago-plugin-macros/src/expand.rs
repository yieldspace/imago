use std::path::Path;

use quote::{format_ident, quote};
use syn::{Error, Ident, ItemStruct, LitStr, Result};

use crate::{
    args::ImagoNativePluginArgs,
    descriptor::{ParseOptions, parse_wit_descriptor},
};

pub(crate) fn expand_imago_native_plugin(
    args: ImagoNativePluginArgs,
    plugin_struct: ItemStruct,
) -> Result<proc_macro2::TokenStream> {
    if !plugin_struct.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &plugin_struct.ident,
            "imago_native_plugin does not support generic plugin structs",
        ));
    }
    if !args.descriptor_only && !args.generate_bindings {
        return Err(Error::new_spanned(
            &plugin_struct.ident,
            "generate_bindings=false requires descriptor_only=true",
        ));
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|err| {
        Error::new_spanned(
            &plugin_struct.ident,
            format!("failed to read CARGO_MANIFEST_DIR: {err}"),
        )
    })?;
    let wit_path = Path::new(&manifest_dir).join(args.wit.value());

    let descriptor = parse_wit_descriptor(
        &wit_path,
        &args.world.value(),
        ParseOptions {
            allow_multiple_imports: args.multi_imports,
            allow_non_resource_types: args.allow_non_resource_types,
        },
    )
    .map_err(|message| {
        Error::new_spanned(
            &plugin_struct.ident,
            format!(
                "failed to build native plugin descriptor from '{}' (world='{}'): {message}",
                wit_path.display(),
                args.world.value()
            ),
        )
    })?;

    let plugin_ident = &plugin_struct.ident;
    let bindings_module_name = format_ident!("{}_bindings", ident_to_snake_case(plugin_ident));
    let wit_path_lit = args.wit;
    let world_lit = args.world;
    let package_name_lit = LitStr::new(&descriptor.package_name, plugin_ident.span());
    let import_lits = descriptor
        .import_names
        .iter()
        .map(|import_name| LitStr::new(import_name, plugin_ident.span()))
        .collect::<Vec<_>>();
    let symbol_lits = descriptor
        .symbols
        .iter()
        .map(|symbol| LitStr::new(symbol, plugin_ident.span()))
        .collect::<Vec<_>>();
    let import_name_const = if descriptor.import_names.len() == 1 {
        let single_import_lit = LitStr::new(&descriptor.import_names[0], plugin_ident.span());
        quote! {
            pub const IMPORT_NAME: &'static str = #single_import_lit;
        }
    } else {
        quote! {}
    };
    let bindings_module = if args.generate_bindings {
        quote! {
            pub mod #bindings_module_name {
                ::wasmtime::component::bindgen!({
                    path: #wit_path_lit,
                    world: #world_lit,
                });
            }
        }
    } else {
        quote! {}
    };
    let native_plugin_impl = if args.descriptor_only {
        quote! {}
    } else {
        quote! {
            impl ::imagod_runtime_wasmtime::native_plugins::NativePlugin for #plugin_ident {
                fn package_name(&self) -> &'static str {
                    Self::PACKAGE_NAME
                }

                fn supports_import(&self, import_name: &str) -> bool {
                    Self::IMPORTS.contains(&import_name)
                }

                fn symbols(&self) -> &'static [&'static str] {
                    Self::SYMBOLS
                }

                fn supports_symbol(&self, symbol: &str) -> bool {
                    Self::IMPORTS.iter().any(|import_name| {
                        symbol
                            .strip_prefix(import_name)
                            .is_some_and(|tail| tail.starts_with('.'))
                    })
                }

                fn add_to_linker(
                    &self,
                    linker: &mut ::imagod_runtime_wasmtime::native_plugins::NativePluginLinker,
                ) -> ::imagod_runtime_wasmtime::native_plugins::NativePluginResult<()> {
                    #bindings_module_name::Host_::add_to_linker::<
                        _,
                        ::imagod_runtime_wasmtime::native_plugins::HasSelf<_>,
                    >(linker, |state| state)
                    .map_err(|err| {
                        ::imagod_runtime_wasmtime::native_plugins::map_native_plugin_linker_error(
                            Self::PACKAGE_NAME,
                            err,
                        )
                    })
                }
            }
        }
    };

    let expanded = quote! {
        #plugin_struct

        #bindings_module

        impl #plugin_ident {
            pub const PACKAGE_NAME: &'static str = #package_name_lit;
            #import_name_const
            pub const IMPORTS: &'static [&'static str] = &[#(#import_lits),*];
            pub const SYMBOLS: &'static [&'static str] = &[#(#symbol_lits),*];
        }

        #native_plugin_impl
    };

    Ok(expanded)
}

fn ident_to_snake_case(ident: &Ident) -> String {
    let text = ident.to_string();
    let mut out = String::with_capacity(text.len() + 8);
    for (index, ch) in text.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
