use syn::{
    Error, Ident, LitStr, Result,
    parse::{Parse, ParseStream},
};

pub(crate) struct ImagoNativePluginArgs {
    pub(crate) wit: LitStr,
    pub(crate) world: LitStr,
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
