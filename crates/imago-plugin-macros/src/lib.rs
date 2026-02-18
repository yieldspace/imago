use std::path::Path;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Error, Ident, ItemStruct, LitStr, Result,
    parse::{Parse, ParseStream},
    parse_macro_input,
};
use wit_parser::{Resolve, WorldItem};

#[proc_macro_attribute]
pub fn imago_native_plugin(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ImagoNativePluginArgs);
    let plugin_struct = parse_macro_input!(item as ItemStruct);

    match expand_imago_native_plugin(args, plugin_struct) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_imago_native_plugin(
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

#[derive(Debug)]
struct WitDescriptor {
    package_name: String,
    import_name: String,
    symbols: Vec<String>,
}

fn parse_wit_descriptor(
    wit_path: &Path,
    world_name: &str,
) -> std::result::Result<WitDescriptor, String> {
    let mut resolve = Resolve::default();
    let (top_package, _) = resolve
        .push_path(wit_path)
        .map_err(|err| format!("failed to parse WIT path: {err}"))?;
    let world_id = resolve
        .select_world(&[top_package], Some(world_name))
        .map_err(|err| format!("world '{world_name}' was not found: {err}"))?;
    let world = &resolve.worlds[world_id];

    if !world.exports.is_empty() {
        return Err(
            "world exports are not supported for native plugin descriptor generation".to_string(),
        );
    }

    let mut imported_interfaces = Vec::new();
    for (_key, item) in &world.imports {
        match item {
            WorldItem::Interface { id, .. } => {
                let import_name = resolve
                    .id_of(*id)
                    .ok_or_else(|| "anonymous interface import is not supported".to_string())?;
                imported_interfaces.push((*id, import_name));
            }
            WorldItem::Function(function) => {
                return Err(format!(
                    "imported function '{}' is not supported; import one interface only",
                    function.name
                ));
            }
            WorldItem::Type(_) => {
                return Err("imported type is not supported; import one interface only".to_string());
            }
        }
    }

    if imported_interfaces.len() != 1 {
        return Err(format!(
            "world must import exactly one interface, found {}",
            imported_interfaces.len()
        ));
    }

    let (interface_id, import_name) = imported_interfaces.remove(0);
    let interface = &resolve.interfaces[interface_id];

    if !interface.types.is_empty() {
        return Err("imported interface must not define non-function types".to_string());
    }

    if interface.functions.is_empty() {
        return Err("imported interface must define at least one function".to_string());
    }

    let package_id = interface
        .package
        .ok_or_else(|| "imported interface package metadata is missing".to_string())?;
    let package_name = &resolve.packages[package_id].name;
    let package_name = format!("{}:{}", package_name.namespace, package_name.name);

    let symbols = interface
        .functions
        .keys()
        .map(|function_name| format!("{import_name}.{function_name}"))
        .collect::<Vec<_>>();

    Ok(WitDescriptor {
        package_name,
        import_name,
        symbols,
    })
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

struct ImagoNativePluginArgs {
    wit: LitStr,
    world: LitStr,
}

impl Parse for ImagoNativePluginArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut wit = None;
        let mut world = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let key_text = key.to_string();
            input.parse::<syn::Token![=]>()?;
            let value: LitStr = input.parse()?;

            match key_text.as_str() {
                "wit" => wit = Some(value),
                "world" => world = Some(value),
                _ => {
                    return Err(Error::new_spanned(
                        key,
                        "unsupported argument; expected `wit` or `world`",
                    ));
                }
            }

            if input.is_empty() {
                break;
            }
            input.parse::<syn::Token![,]>()?;
        }

        let wit = wit.ok_or_else(|| Error::new(input.span(), "missing required argument `wit`"))?;
        let world =
            world.ok_or_else(|| Error::new(input.span(), "missing required argument `world`"))?;

        Ok(Self { wit, world })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-plugin-macros-tests-{}-{}-{}",
            test_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent should be created");
        }
        std::fs::write(path, text).expect("file should be written");
    }

    #[test]
    fn parse_wit_descriptor_reads_package_import_and_symbols() {
        let root = new_temp_dir("valid");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    service-name: func() -> string;
    release-hash: func() -> string;
}

world host {
    import runtime;
}
"#,
        );

        let descriptor = parse_wit_descriptor(&root.join("wit"), "host")
            .expect("valid wit should produce descriptor");

        assert_eq!(descriptor.package_name, "imago:admin");
        assert_eq!(descriptor.import_name, "imago:admin/runtime@0.1.0");
        assert_eq!(
            descriptor.symbols,
            vec![
                "imago:admin/runtime@0.1.0.service-name".to_string(),
                "imago:admin/runtime@0.1.0.release-hash".to_string(),
            ]
        );
    }

    #[test]
    fn parse_wit_descriptor_rejects_multiple_import_interfaces() {
        let root = new_temp_dir("multiple-imports");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    ping: func() -> string;
}

interface extra {
    pong: func() -> string;
}

world host {
    import runtime;
    import extra;
}
"#,
        );

        let err = parse_wit_descriptor(&root.join("wit"), "host")
            .expect_err("multiple imports should fail");
        assert!(
            err.contains("exactly one interface"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_wit_descriptor_rejects_non_function_types() {
        let root = new_temp_dir("non-function-type");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    type app-state = string;
    service-name: func() -> string;
}

world host {
    import runtime;
}
"#,
        );

        let err = parse_wit_descriptor(&root.join("wit"), "host")
            .expect_err("interface types should fail");
        assert!(
            err.contains("must not define non-function types"),
            "unexpected error: {err}"
        );
    }
}
