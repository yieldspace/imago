//! TOML parsing and typed decode helpers for `imagod.toml`.

use std::path::Path;

use imagod_common::ImagodError;
use imagod_spec::ErrorCode;

use crate::{ImagodConfig, ImagodTomlDocument};

pub(crate) fn parse(path: &Path, content: &str) -> Result<toml::Value, ImagodError> {
    toml::from_str(content).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!("config parse failed: {e}"),
        )
        .with_detail("path", path.to_string_lossy())
    })
}

pub(crate) fn decode(path: &Path, raw: toml::Value) -> Result<ImagodConfig, ImagodError> {
    let document: ImagodTomlDocument = raw.try_into().map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!("config decode failed: {e}"),
        )
        .with_detail("path", path.to_string_lossy())
    })?;
    Ok(document.into_config())
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use std::path::Path;

    use super::{decode, parse};

    #[test]
    fn given_toml_content__when_parse__then_valid_and_invalid_cases_return_expected_errors() {
        let valid = r#"
listen_addr = "127.0.0.1:4443"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = []
"#;
        parse(Path::new("/tmp/imagod.toml"), valid).expect("valid toml should parse");

        let err = parse(Path::new("/tmp/imagod.toml"), "listen_addr = [")
            .expect_err("invalid toml should fail");
        assert_eq!(err.code, imagod_spec::ErrorCode::BadRequest);
        assert_eq!(err.stage, "config.load");
        assert!(err.message.contains("config parse failed"));
        assert_eq!(
            err.details.get("path").map(String::as_str),
            Some("/tmp/imagod.toml")
        );
    }

    #[test]
    fn given_decoded_toml_value__when_decode__then_type_mismatch_is_reported() {
        let valid_raw: toml::Value = toml::from_str(
            r#"
listen_addr = "127.0.0.1:4443"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = []
"#,
        )
        .expect("fixture toml should parse");
        decode(Path::new("/tmp/imagod.toml"), valid_raw).expect("valid value should decode");

        let invalid_raw: toml::Value = toml::from_str(
            r#"
listen_addr = 1
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = []
"#,
        )
        .expect("fixture toml should parse");
        let err = decode(Path::new("/tmp/imagod.toml"), invalid_raw)
            .expect_err("invalid type should fail decode");
        assert_eq!(err.code, imagod_spec::ErrorCode::BadRequest);
        assert_eq!(err.stage, "config.load");
        assert!(err.message.contains("config decode failed"));
        assert_eq!(
            err.details.get("path").map(String::as_str),
            Some("/tmp/imagod.toml")
        );
    }
}
