use std::path::Path;

use quote::{format_ident, quote};
use syn::{Error, Ident, ItemStruct, LitStr, Result};

use crate::{args::ImagoNativePluginArgs, descriptor::parse_wit_descriptor};

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

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|err| {
        Error::new_spanned(
            &plugin_struct.ident,
            format!("failed to read CARGO_MANIFEST_DIR: {err}"),
        )
    })?;
    let wit_path = Path::new(&manifest_dir).join(args.wit.value());

    let descriptor = parse_wit_descriptor(&wit_path, &args.world.value()).map_err(|message| {
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
    let import_name_lit = LitStr::new(&descriptor.import_name, plugin_ident.span());
    let symbol_lits = descriptor
        .symbols
        .iter()
        .map(|symbol| LitStr::new(symbol, plugin_ident.span()))
        .collect::<Vec<_>>();

    let expanded = quote! {
        #plugin_struct

        pub mod #bindings_module_name {
            ::wasmtime::component::bindgen!({
                path: #wit_path_lit,
                world: #world_lit,
            });
        }

        impl #plugin_ident {
            pub const PACKAGE_NAME: &'static str = #package_name_lit;
            pub const IMPORT_NAME: &'static str = #import_name_lit;
            pub const SYMBOLS: &'static [&'static str] = &[#(#symbol_lits),*];
        }

        impl ::imagod_runtime_wasmtime::native_plugins::NativePlugin for #plugin_ident {
            fn package_name(&self) -> &'static str {
                Self::PACKAGE_NAME
            }

            fn supports_import(&self, import_name: &str) -> bool {
                import_name == Self::IMPORT_NAME
            }

            fn symbols(&self) -> &'static [&'static str] {
                Self::SYMBOLS
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
