mod args;
mod descriptor;
mod expand;

use proc_macro::TokenStream;
use syn::{ItemStruct, parse_macro_input};

use crate::{args::ImagoNativePluginArgs, expand::expand_imago_native_plugin};

#[proc_macro_attribute]
pub fn imago_native_plugin(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ImagoNativePluginArgs);
    let plugin_struct = parse_macro_input!(item as ItemStruct);

    match expand_imago_native_plugin(args, plugin_struct) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
