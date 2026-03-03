use super::*;

pub(in crate::commands::build) fn parse_assets(
    value: Option<&TomlValue>,
    project_root: &Path,
) -> anyhow::Result<Vec<AssetSource>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("assets must be an array"))?;

    let mut assets = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let table = item
            .as_table()
            .ok_or_else(|| anyhow!("assets[{}] must be a table", index))?;

        let path_value = table
            .get("path")
            .ok_or_else(|| anyhow!("assets[{}].path is required", index))?;
        let path_text = path_value
            .as_str()
            .ok_or_else(|| anyhow!("assets[{}].path must be a string", index))?;
        let normalized = normalize_relative_path(path_text, "assets[].path")?;
        ensure_file_exists(project_root, &normalized, "assets[].path")?;

        let mut extra = BTreeMap::new();
        for (key, value) in table {
            if key == "path" {
                continue;
            }
            extra.insert(key.clone(), toml_to_json_normalized(value)?);
        }

        assets.push(AssetSource {
            manifest_asset: ManifestAsset {
                path: normalized_path_to_string(&normalized),
                extra,
            },
            source_path: normalized,
        });
    }

    Ok(assets)
}

pub(in crate::commands::build) fn parse_resources_section(
    root: &toml::Table,
    assets: &[AssetSource],
) -> anyhow::Result<Option<ManifestResourcesConfig>> {
    let Some(value) = root.get("resources") else {
        return Ok(None);
    };
    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("resources must be a table"))?;

    let args = parse_resources_args(table.get("args"))?;
    let env = parse_string_table(table.get("env"), "resources.env")?;
    let http_outbound = parse_resources_http_outbound(table.get("http_outbound"))?;

    let allowed_asset_dirs = collect_allowed_resource_asset_dirs(assets);
    let mounts =
        parse_resource_mount_entries(table.get("mounts"), "resources.mounts", &allowed_asset_dirs)?;
    let read_only_mounts = parse_resource_mount_entries(
        table.get("read_only_mounts"),
        "resources.read_only_mounts",
        &allowed_asset_dirs,
    )?;
    validate_resource_mount_uniqueness(&mounts, &read_only_mounts)?;

    let mut extra = BTreeMap::new();
    for (key, value) in table {
        if matches!(
            key.as_str(),
            "args" | "env" | "http_outbound" | "mounts" | "read_only_mounts"
        ) {
            continue;
        }
        if key.trim().is_empty() {
            return Err(anyhow!("resources contains an empty key"));
        }
        extra.insert(key.clone(), toml_to_json_normalized(value)?);
    }

    let resources = ManifestResourcesConfig {
        args,
        env,
        http_outbound,
        mounts,
        read_only_mounts,
        extra,
    };
    if resources.is_empty() {
        Ok(None)
    } else {
        Ok(Some(resources))
    }
}

pub(in crate::commands::build) fn parse_resources_args(
    value: Option<&TomlValue>,
) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("resources.args must be an array of strings"))?;
    let mut args = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let arg = value
            .as_str()
            .ok_or_else(|| anyhow!("resources.args[{index}] must be a string"))?
            .trim()
            .to_string();
        if arg.is_empty() {
            return Err(anyhow!("resources.args[{index}] must not be empty"));
        }
        args.push(arg);
    }
    Ok(args)
}

pub(in crate::commands::build) fn parse_resources_http_outbound(
    value: Option<&TomlValue>,
) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("resources.http_outbound must be an array of strings"))?;
    let mut rules = Vec::with_capacity(array.len());
    let mut seen = BTreeSet::new();
    for (index, value) in array.iter().enumerate() {
        let raw = value
            .as_str()
            .ok_or_else(|| anyhow!("resources.http_outbound[{index}] must be a string"))?;
        let normalized =
            normalize_wasi_http_outbound_rule(raw, &format!("resources.http_outbound[{index}]"))?;
        if seen.insert(normalized.clone()) {
            rules.push(normalized);
        }
    }
    Ok(rules)
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_rule(
    raw: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    if value.contains('*') {
        return Err(anyhow!("{field_name} wildcard is not supported: {}", value));
    }
    if value.chars().any(|ch| ch.is_whitespace()) {
        return Err(anyhow!(
            "{field_name} must not contain whitespace: {}",
            value
        ));
    }
    if value.contains('/') {
        return normalize_wasi_http_outbound_cidr(value, field_name);
    }

    normalize_wasi_http_outbound_host_or_host_port(value, field_name)
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_cidr(
    value: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let (ip_text, prefix_text) = value.split_once('/').ok_or_else(|| {
        anyhow!(
            "{field_name} must be hostname, host:port, or CIDR: {}",
            value
        )
    })?;
    if ip_text.is_empty() || prefix_text.is_empty() || prefix_text.contains('/') {
        return Err(anyhow!(
            "{field_name} must be valid CIDR (<ip>/<prefix>): {}",
            value
        ));
    }
    let ip = ip_text.parse::<IpAddr>().map_err(|err| {
        anyhow!(
            "{field_name} CIDR ip is invalid '{}': {err}",
            ip_text.trim()
        )
    })?;
    let prefix = prefix_text.parse::<u8>().map_err(|err| {
        anyhow!(
            "{field_name} CIDR prefix is invalid '{}': {err}",
            prefix_text.trim()
        )
    })?;
    let max_prefix = match ip {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if prefix > max_prefix {
        return Err(anyhow!(
            "{field_name} CIDR prefix must be in range 0..={max_prefix}: {}",
            prefix
        ));
    }

    let network_ip = cidr_network_ip(ip, prefix);
    Ok(format!("{network_ip}/{prefix}"))
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_host_or_host_port(
    value: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    if value.starts_with('[') {
        let close_index = value
            .find(']')
            .ok_or_else(|| anyhow!("{field_name} has invalid bracketed host: {value}"))?;
        let host_text = &value[1..close_index];
        let host_ip = host_text.parse::<Ipv6Addr>().map_err(|err| {
            anyhow!(
                "{field_name} bracketed host must be valid IPv6: {} ({err})",
                host_text
            )
        })?;
        let rest = &value[(close_index + 1)..];
        if rest.is_empty() {
            return Ok(host_ip.to_string());
        }
        let port_text = rest.strip_prefix(':').ok_or_else(|| {
            anyhow!(
                "{field_name} bracketed host must use [ipv6]:port format: {}",
                value
            )
        })?;
        let port = parse_wasi_http_outbound_port(port_text, field_name)?;
        return Ok(format!("[{host_ip}]:{port}"));
    }

    if value.matches(':').count() > 1 {
        let ip = value.parse::<IpAddr>().map_err(|err| {
            anyhow!(
                "{field_name} must use [ipv6]:port for IPv6 host: {} ({err})",
                value
            )
        })?;
        return Ok(ip.to_string());
    }

    if let Some((host_text, port_text)) = value.rsplit_once(':')
        && port_text.chars().all(|ch| ch.is_ascii_digit())
    {
        let host = normalize_wasi_http_outbound_host(host_text, field_name)?;
        let port = parse_wasi_http_outbound_port(port_text, field_name)?;
        if host.contains(':') {
            return Ok(format!("[{host}]:{port}"));
        }
        return Ok(format!("{host}:{port}"));
    }

    normalize_wasi_http_outbound_host(value, field_name)
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_host(
    raw_host: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let host = raw_host.trim();
    if host.is_empty() {
        return Err(anyhow!("{field_name} host must not be empty"));
    }
    if host.contains('*') {
        return Err(anyhow!(
            "{field_name} wildcard host is not supported: {}",
            host
        ));
    }
    if host.contains('/') || host.contains('\\') {
        return Err(anyhow!(
            "{field_name} host must not contain path separators: {}",
            host
        ));
    }
    if host.chars().any(|ch| ch.is_whitespace()) {
        return Err(anyhow!(
            "{field_name} host must not contain whitespace: {}",
            host
        ));
    }
    if host.starts_with('[') || host.ends_with(']') {
        return Err(anyhow!(
            "{field_name} host must not contain brackets: {}",
            host
        ));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip.to_string());
    }

    if host.contains(':') {
        return Err(anyhow!(
            "{field_name} host with ':' must use [ipv6]:port format: {}",
            host
        ));
    }

    Ok(host.to_ascii_lowercase())
}

pub(in crate::commands::build) fn parse_wasi_http_outbound_port(
    port_text: &str,
    field_name: &str,
) -> anyhow::Result<u16> {
    let port = port_text.parse::<u16>().map_err(|err| {
        anyhow!(
            "{field_name} port must be in range 1..=65535 (got '{}'): {err}",
            port_text
        )
    })?;
    if port == 0 {
        return Err(anyhow!(
            "{field_name} port must be in range 1..=65535 (got 0)"
        ));
    }
    Ok(port)
}

pub(in crate::commands::build) fn cidr_network_ip(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << u32::from(32_u8.saturating_sub(prefix))
            };
            IpAddr::V4(Ipv4Addr::from(bits & mask))
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << u32::from(128_u8.saturating_sub(prefix))
            };
            IpAddr::V6(Ipv6Addr::from(bits & mask))
        }
    }
}

pub(in crate::commands::build) fn load_dotenv_resources_env(
    project_root: &Path,
) -> anyhow::Result<BTreeMap<String, String>> {
    let path = project_root.join(".env");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let iter =
        from_path_iter(&path).with_context(|| format!("failed to parse {}", path.display()))?;

    let mut env = BTreeMap::new();
    for entry in iter {
        let (key, value) = entry.with_context(|| format!("failed to parse {}", path.display()))?;
        env.insert(key, value);
    }
    Ok(env)
}

pub(in crate::commands::build) fn collect_allowed_resource_asset_dirs(
    assets: &[AssetSource],
) -> BTreeSet<PathBuf> {
    let mut allowed = BTreeSet::new();
    for asset in assets {
        if let Some(parent) = asset.source_path.parent()
            && !parent.as_os_str().is_empty()
        {
            allowed.insert(parent.to_path_buf());
        }
    }
    allowed
}

pub(in crate::commands::build) fn parse_resource_mount_entries(
    value: Option<&TomlValue>,
    field_name: &str,
    allowed_asset_dirs: &BTreeSet<PathBuf>,
) -> anyhow::Result<Vec<ManifestWasiMount>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("{field_name} must be an array"))?;
    let mut mounts = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let entry = item
            .as_table()
            .ok_or_else(|| anyhow!("{field_name}[{index}] must be a table"))?;
        for key in entry.keys() {
            if !matches!(key.as_str(), "asset_dir" | "guest_path") {
                return Err(anyhow!("{field_name}[{index}].{key} is not supported"));
            }
        }

        let asset_dir_raw = entry
            .get("asset_dir")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("{field_name}[{index}].asset_dir must be a string"))?;
        let asset_dir =
            normalize_relative_path(asset_dir_raw, &format!("{field_name}[{index}].asset_dir"))?;
        if !allowed_asset_dirs.contains(&asset_dir) {
            return Err(anyhow!(
                "{field_name}[{index}].asset_dir must match a directory derived from assets[].path"
            ));
        }

        let guest_path_raw = entry
            .get("guest_path")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("{field_name}[{index}].guest_path must be a string"))?;
        let guest_path = normalize_wasi_guest_path(
            guest_path_raw,
            &format!("{field_name}[{index}].guest_path"),
        )?;

        mounts.push(ManifestWasiMount {
            asset_dir: normalized_path_to_string(&asset_dir),
            guest_path,
        });
    }
    Ok(mounts)
}

pub(in crate::commands::build) fn validate_resource_mount_uniqueness(
    mounts: &[ManifestWasiMount],
    read_only_mounts: &[ManifestWasiMount],
) -> anyhow::Result<()> {
    let mut seen_guest_paths = BTreeSet::new();
    let mut seen_asset_dirs = BTreeSet::new();
    for mount in mounts.iter().chain(read_only_mounts.iter()) {
        if !seen_guest_paths.insert(mount.guest_path.clone()) {
            return Err(anyhow!(
                "resources mounts contain duplicate guest_path: {}",
                mount.guest_path
            ));
        }
        if !seen_asset_dirs.insert(mount.asset_dir.clone()) {
            return Err(anyhow!(
                "resources mounts contain duplicate asset_dir: {}",
                mount.asset_dir
            ));
        }
    }
    Ok(())
}

pub(in crate::commands::build) fn toml_to_json_normalized(
    value: &TomlValue,
) -> anyhow::Result<JsonValue> {
    Ok(match value {
        TomlValue::String(v) => JsonValue::String(v.clone()),
        TomlValue::Integer(v) => JsonValue::Number((*v).into()),
        TomlValue::Float(v) => {
            let number = serde_json::Number::from_f64(*v)
                .ok_or_else(|| anyhow!("floating-point value is not representable as JSON"))?;
            JsonValue::Number(number)
        }
        TomlValue::Boolean(v) => JsonValue::Bool(*v),
        TomlValue::Datetime(v) => JsonValue::String(v.to_string()),
        TomlValue::Array(values) => JsonValue::Array(
            values
                .iter()
                .map(toml_to_json_normalized)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        TomlValue::Table(table) => {
            let mut keys = table.keys().cloned().collect::<Vec<_>>();
            keys.sort();

            let mut object = serde_json::Map::new();
            for key in keys {
                let nested = table
                    .get(&key)
                    .ok_or_else(|| anyhow!("internal error: missing table key"))?;
                object.insert(key, toml_to_json_normalized(nested)?);
            }
            JsonValue::Object(object)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use toml::Value as TomlValue;

    use super::{
        AssetSource, ManifestAsset, cidr_network_ip, normalize_wasi_http_outbound_rule,
        parse_resource_mount_entries, parse_resources_http_outbound,
        validate_resource_mount_uniqueness,
    };

    fn parse_table(raw: &str) -> toml::Table {
        toml::from_str::<TomlValue>(raw)
            .expect("toml should parse")
            .as_table()
            .expect("value should be table")
            .clone()
    }

    #[test]
    fn normalize_wasi_http_outbound_rule_normalizes_cidr_network() {
        let normalized = normalize_wasi_http_outbound_rule("192.168.10.11/24", "field")
            .expect("cidr should normalize");
        assert_eq!(normalized, "192.168.10.0/24");
    }

    #[test]
    fn normalize_wasi_http_outbound_rule_rejects_wildcard() {
        let err = normalize_wasi_http_outbound_rule("*.example.com", "field")
            .expect_err("wildcard should be rejected");
        assert!(err.to_string().contains("wildcard is not supported"));
    }

    #[test]
    fn parse_resources_http_outbound_deduplicates_entries() {
        let table = parse_table(
            r#"
rules = ["example.com:443", " example.com:443 ", "192.168.0.1/24"]
"#,
        );
        let rules = parse_resources_http_outbound(table.get("rules"))
            .expect("http_outbound rules should parse");
        assert_eq!(rules, vec!["example.com:443", "192.168.0.0/24"]);
    }

    #[test]
    fn parse_resource_mount_entries_rejects_unknown_asset_dir() {
        let mounts = parse_table(
            r#"
entries = [{ asset_dir = "assets", guest_path = "/data" }]
"#,
        );
        let allowed = BTreeSet::from([std::path::PathBuf::from("public")]);
        let err = parse_resource_mount_entries(mounts.get("entries"), "resources.mounts", &allowed)
            .expect_err("unknown asset_dir should be rejected");
        assert!(
            err.to_string()
                .contains("must match a directory derived from assets")
        );
    }

    #[test]
    fn validate_resource_mount_uniqueness_rejects_duplicate_guest_path() {
        let mounts = vec![crate::commands::build::ManifestWasiMount {
            asset_dir: "assets".to_string(),
            guest_path: "/data".to_string(),
        }];
        let read_only_mounts = vec![crate::commands::build::ManifestWasiMount {
            asset_dir: "assets-ro".to_string(),
            guest_path: "/data".to_string(),
        }];

        let err = validate_resource_mount_uniqueness(&mounts, &read_only_mounts)
            .expect_err("duplicate guest path should fail");
        assert!(err.to_string().contains("duplicate guest_path"));
    }

    #[test]
    fn collect_allowed_resource_asset_dirs_from_assets_can_be_used_by_mount_parser() {
        let assets = vec![AssetSource {
            manifest_asset: ManifestAsset {
                path: "public/static/logo.svg".to_string(),
                extra: BTreeMap::new(),
            },
            source_path: std::path::PathBuf::from("public/static/logo.svg"),
        }];
        let allowed = super::collect_allowed_resource_asset_dirs(&assets);
        let mounts = parse_table(
            r#"
entries = [{ asset_dir = "public/static", guest_path = "/assets" }]
"#,
        );

        let parsed =
            parse_resource_mount_entries(mounts.get("entries"), "resources.mounts", &allowed)
                .expect("asset dir derived from assets should be accepted");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].guest_path, "/assets");
    }

    #[test]
    fn cidr_network_ip_masks_ipv4_bits() {
        let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 1, 2, 200));
        let masked = cidr_network_ip(ip, 16);
        assert_eq!(masked.to_string(), "10.1.0.0");
    }
}
