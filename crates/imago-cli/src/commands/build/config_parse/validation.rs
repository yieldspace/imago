use super::*;
use crate::commands::build::DEFAULT_HTTP_LISTEN_ADDR;

pub(in crate::commands::build) fn required_string(
    root: &toml::Table,
    key: &str,
) -> anyhow::Result<String> {
    let value = root
        .get(key)
        .ok_or_else(|| anyhow!("imago.toml missing required key: {key}"))?;
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("imago.toml key '{}' must be a string", key))?
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(anyhow!("imago.toml key '{}' must not be empty", key));
    }
    Ok(text)
}

pub(crate) fn validate_service_name(name: &str) -> anyhow::Result<()> {
    build_validation::validate_service_name(name)
}

pub(crate) fn validate_app_type(app_type: &str) -> anyhow::Result<()> {
    build_validation::validate_app_type(app_type)
}

pub(in crate::commands::build) fn parse_http_section(
    root: &toml::Table,
    app_type: &str,
) -> anyhow::Result<Option<ManifestHttp>> {
    let http = root.get("http");

    if app_type != "http" {
        if http.is_some() {
            return Err(anyhow!(
                "http section is only allowed when type is \"http\""
            ));
        }
        return Ok(None);
    }

    let table = http
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("type=\"http\" requires [http] table"))?;
    let raw_port = table
        .get("port")
        .and_then(TomlValue::as_integer)
        .ok_or_else(|| anyhow!("http.port is required when type=\"http\""))?;
    let port = u16::try_from(raw_port)
        .map_err(|_| anyhow!("http.port must be in range 1..=65535 (got {raw_port})"))?;
    if port == 0 {
        return Err(anyhow!("http.port must be in range 1..=65535 (got 0)"));
    }

    let max_body_bytes = match table.get("max_body_bytes") {
        Some(value) => {
            let raw = value.as_integer().ok_or_else(|| {
                anyhow!("http.max_body_bytes must be in range 1..={MAX_HTTP_MAX_BODY_BYTES}")
            })?;
            let value = u64::try_from(raw).map_err(|_| {
                anyhow!(
                    "http.max_body_bytes must be in range 1..={} (got {raw})",
                    MAX_HTTP_MAX_BODY_BYTES
                )
            })?;
            if value == 0 || value > MAX_HTTP_MAX_BODY_BYTES {
                return Err(anyhow!(
                    "http.max_body_bytes must be in range 1..={} (got {value})",
                    MAX_HTTP_MAX_BODY_BYTES
                ));
            }
            value
        }
        None => DEFAULT_HTTP_MAX_BODY_BYTES,
    };

    let listen_addr = match table.get("listen_addr") {
        Some(value) => {
            let listen_addr = value
                .as_str()
                .ok_or_else(|| anyhow!("http.listen_addr must be a valid IP address"))?
                .trim()
                .to_string();
            if listen_addr.is_empty() {
                return Err(anyhow!(
                    "http.listen_addr must be a valid IP address (got empty value)"
                ));
            }
            listen_addr.parse::<IpAddr>().map_err(|err| {
                anyhow!("http.listen_addr must be a valid IP address (got '{listen_addr}'): {err}")
            })?;
            listen_addr
        }
        None => DEFAULT_HTTP_LISTEN_ADDR.to_string(),
    };

    Ok(Some(ManifestHttp {
        port,
        listen_addr,
        max_body_bytes,
    }))
}

pub(in crate::commands::build) fn parse_socket_section(
    root: &toml::Table,
    app_type: &str,
) -> anyhow::Result<Option<ManifestSocket>> {
    let socket = root.get("socket");

    if app_type != "socket" {
        if socket.is_some() {
            return Err(anyhow!(
                "socket section is only allowed when type is \"socket\""
            ));
        }
        return Ok(None);
    }

    let table = socket
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("type=\"socket\" requires [socket] table"))?;

    let protocol_raw = table
        .get("protocol")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("socket.protocol is required when type=\"socket\""))?;
    let protocol = match protocol_raw {
        "udp" => ManifestSocketProtocol::Udp,
        "tcp" => ManifestSocketProtocol::Tcp,
        "both" => ManifestSocketProtocol::Both,
        _ => {
            return Err(anyhow!(
                "socket.protocol must be one of: udp, tcp, both (got: {protocol_raw})"
            ));
        }
    };

    let direction_raw = table
        .get("direction")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("socket.direction is required when type=\"socket\""))?;
    let direction = match direction_raw {
        "inbound" => ManifestSocketDirection::Inbound,
        "outbound" => ManifestSocketDirection::Outbound,
        "both" => ManifestSocketDirection::Both,
        _ => {
            return Err(anyhow!(
                "socket.direction must be one of: inbound, outbound, both (got: {direction_raw})"
            ));
        }
    };

    let listen_addr = table
        .get("listen_addr")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("socket.listen_addr is required when type=\"socket\""))?
        .trim()
        .to_string();
    if listen_addr.is_empty() {
        return Err(anyhow!(
            "socket.listen_addr must be a valid IP address (got empty value)"
        ));
    }
    listen_addr.parse::<IpAddr>().map_err(|err| {
        anyhow!("socket.listen_addr must be a valid IP address (got '{listen_addr}'): {err}")
    })?;

    let raw_port = table
        .get("listen_port")
        .and_then(TomlValue::as_integer)
        .ok_or_else(|| anyhow!("socket.listen_port is required when type=\"socket\""))?;
    let listen_port = u16::try_from(raw_port)
        .map_err(|_| anyhow!("socket.listen_port must be in range 1..=65535 (got {raw_port})"))?;
    if listen_port == 0 {
        return Err(anyhow!(
            "socket.listen_port must be in range 1..=65535 (got 0)"
        ));
    }

    Ok(Some(ManifestSocket {
        protocol,
        direction,
        listen_addr,
        listen_port,
    }))
}

pub(in crate::commands::build) fn normalize_relative_path(
    raw: &str,
    field_name: &str,
) -> anyhow::Result<PathBuf> {
    normalize_relative_path_with_policy(raw, field_name, false)
}

pub(in crate::commands::build) fn normalize_source_main_path(
    raw: &str,
    field_name: &str,
) -> anyhow::Result<PathBuf> {
    normalize_relative_path_with_policy(raw, field_name, true)
}

fn normalize_relative_path_with_policy(
    raw: &str,
    field_name: &str,
    allow_parent_dirs: bool,
) -> anyhow::Result<PathBuf> {
    if raw.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(anyhow!("{field_name} must be a relative path: {raw}"));
    }
    if raw.contains('\\') {
        return Err(anyhow!("{field_name} must not contain backslashes: {raw}"));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(anyhow!("{field_name} must not be windows-prefixed: {raw}"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir if allow_parent_dirs => normalized.push(".."),
            Component::ParentDir | Component::RootDir => {
                return Err(anyhow!(
                    "{field_name} must not contain path traversal: {raw}"
                ));
            }
            _ => {
                return Err(anyhow!(
                    "{field_name} contains invalid path component: {raw}"
                ));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} is invalid: {raw}"));
    }

    Ok(normalized)
}

pub(in crate::commands::build) fn normalized_path_to_string(path: &Path) -> String {
    path.iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

pub(in crate::commands::build) fn ensure_file_exists(
    project_root: &Path,
    relative: &Path,
    field_name: &str,
) -> anyhow::Result<()> {
    let path = project_root.join(relative);
    let metadata = fs::metadata(&path)
        .with_context(|| format!("{} file is not accessible: {}", field_name, path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!(
            "{} path is not a file: {}",
            field_name,
            path.display()
        ));
    }
    Ok(())
}

pub(in crate::commands::build) fn validate_dependency_package_name(
    name: &str,
) -> anyhow::Result<()> {
    build_validation::validate_dependency_package_name(name)
}

pub(in crate::commands::build) fn normalize_wasi_guest_path(
    raw: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let path = Path::new(raw.trim());
    if path.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    if raw.contains('\\') {
        return Err(anyhow!(
            "{field_name} must not contain backslashes: {}",
            raw.trim()
        ));
    }
    if !path.is_absolute() {
        return Err(anyhow!(
            "{field_name} must be an absolute path: {}",
            raw.trim()
        ));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(anyhow!(
            "{field_name} must not be windows-prefixed: {}",
            raw.trim()
        ));
    }

    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(segment) => {
                segments.push(segment.to_string_lossy().to_string());
            }
            Component::ParentDir | Component::CurDir => {
                return Err(anyhow!(
                    "{field_name} must not contain path traversal: {}",
                    raw.trim()
                ));
            }
            _ => {
                return Err(anyhow!("{field_name} is invalid: {}", raw.trim()));
            }
        }
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        normalize_relative_path, normalize_source_main_path, normalize_wasi_guest_path,
        validate_service_name,
    };

    #[test]
    fn validate_service_name_rejects_unsupported_character() {
        let err = validate_service_name("svc@name").expect_err("unsupported character must fail");
        assert!(err.to_string().contains("unsupported characters"));
    }

    #[test]
    fn normalize_relative_path_rejects_absolute_and_parent_components() {
        let absolute_err =
            normalize_relative_path("/tmp/file.wit", "field").expect_err("absolute path must fail");
        assert!(absolute_err.to_string().contains("relative path"));

        let parent_err =
            normalize_relative_path("../file.wit", "field").expect_err("parent must fail");
        assert!(parent_err.to_string().contains("path traversal"));
    }

    #[test]
    fn normalize_source_main_path_allows_parent_components_but_rejects_invalid_roots() {
        assert_eq!(
            normalize_source_main_path("../build/app.wasm", "main")
                .expect("parent path should pass"),
            PathBuf::from("../build/app.wasm")
        );
        assert_eq!(
            normalize_source_main_path("./out/../app.wasm", "main")
                .expect("dot segments should normalize"),
            PathBuf::from("out/../app.wasm")
        );

        let empty_err = normalize_source_main_path("", "main").expect_err("empty path must fail");
        assert!(empty_err.to_string().contains("must not be empty"));

        let absolute_err = normalize_source_main_path("/tmp/app.wasm", "main")
            .expect_err("absolute path must fail");
        assert!(absolute_err.to_string().contains("relative path"));

        let windows_err =
            normalize_source_main_path("C:\\app.wasm", "main").expect_err("windows path must fail");
        assert!(windows_err.to_string().contains("backslashes"));

        let backslash_err = normalize_source_main_path("..\\app.wasm", "main")
            .expect_err("backslash traversal must fail");
        assert!(backslash_err.to_string().contains("backslashes"));
    }

    #[test]
    fn normalize_wasi_guest_path_rejects_relative_and_parent_paths() {
        let relative_err = normalize_wasi_guest_path("tmp/data", "guest")
            .expect_err("relative guest path must fail");
        assert!(relative_err.to_string().contains("absolute path"));

        let parent_err =
            normalize_wasi_guest_path("/tmp/../etc", "guest").expect_err("parent must fail");
        assert!(parent_err.to_string().contains("path traversal"));
    }

    #[test]
    fn normalize_wasi_guest_path_normalizes_root_and_nested_paths() {
        assert_eq!(
            normalize_wasi_guest_path("/", "guest").expect("root should pass"),
            "/"
        );
        assert_eq!(
            normalize_wasi_guest_path("/var/log/imago", "guest").expect("nested path should pass"),
            "/var/log/imago"
        );
    }
}
