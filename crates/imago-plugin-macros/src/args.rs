use syn::{
    Error, Ident, LitBool, LitStr, Result,
    parse::{Parse, ParseStream},
};

pub(crate) struct ImagoNativePluginArgs {
    pub(crate) wit: LitStr,
    pub(crate) world: LitStr,
    pub(crate) descriptor_only: bool,
    pub(crate) multi_imports: bool,
    pub(crate) allow_non_resource_types: bool,
    pub(crate) generate_bindings: bool,
}

impl Parse for ImagoNativePluginArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut wit = None;
        let mut world = None;
        let mut descriptor_only = false;
        let mut multi_imports = false;
        let mut allow_non_resource_types = false;
        let mut generate_bindings = true;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let key_text = key.to_string();
            input.parse::<syn::Token![=]>()?;

            match key_text.as_str() {
                "wit" => wit = Some(input.parse::<LitStr>()?),
                "world" => world = Some(input.parse::<LitStr>()?),
                "descriptor_only" => descriptor_only = input.parse::<LitBool>()?.value,
                "multi_imports" => multi_imports = input.parse::<LitBool>()?.value,
                "allow_non_resource_types" => {
                    allow_non_resource_types = input.parse::<LitBool>()?.value
                }
                "generate_bindings" => generate_bindings = input.parse::<LitBool>()?.value,
                _ => {
                    return Err(Error::new_spanned(
                        key,
                        "unsupported argument; expected `wit`, `world`, `descriptor_only`, `multi_imports`, `allow_non_resource_types`, or `generate_bindings`",
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

        Ok(Self {
            wit,
            world,
            descriptor_only,
            multi_imports,
            allow_non_resource_types,
            generate_bindings,
        })
    }
}
