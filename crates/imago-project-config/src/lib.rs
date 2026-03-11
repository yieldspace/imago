//! Typed configuration model for `imago.toml` plus JSON Schema derivation.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use url::Url;

pub const IMAGO_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/yieldspace/imago/main/schemas/imago.schema.json";
pub const IMAGO_SCHEMA_FILENAME: &str = "imago.schema.json";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImagoTomlDocument {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(flatten)]
    pub config: ImagoTomlConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImagoTomlConfig {
    #[schemars(required)]
    pub name: Option<String>,
    #[schemars(required)]
    pub main: Option<String>,
    #[serde(rename = "type")]
    #[schemars(with = "Option<AppType>", required)]
    pub app_type: Option<String>,
    #[schemars(with = "Option<RestartPolicy>")]
    pub restart: Option<String>,
    pub build: Option<BuildSection>,
    #[schemars(required)]
    pub target: Option<BTreeMap<String, TargetEntry>>,
    pub assets: Option<Vec<AssetEntry>>,
    pub http: Option<HttpSection>,
    pub socket: Option<SocketSection>,
    pub resources: Option<ResourcesSection>,
    pub capabilities: Option<CapabilityPolicy>,
    pub bindings: Option<Vec<BindingEntry>>,
    pub dependencies: Option<Vec<DependencyEntry>>,
    pub namespace_registries: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AppType {
    Cli,
    Http,
    Socket,
    Rpc,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BuildSection {
    pub command: Option<BuildCommand>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum BuildCommand {
    Shell(String),
    Argv(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetEntry {
    #[schemars(required)]
    pub remote: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssetEntry {
    #[schemars(required)]
    pub path: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HttpSection {
    #[schemars(required)]
    pub port: Option<u16>,
    pub max_body_bytes: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SocketSection {
    #[schemars(with = "Option<SocketProtocol>", required)]
    pub protocol: Option<String>,
    #[schemars(with = "Option<SocketDirection>", required)]
    pub direction: Option<String>,
    #[schemars(required)]
    pub listen_addr: Option<String>,
    #[schemars(required)]
    pub listen_port: Option<u16>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SocketProtocol {
    Udp,
    Tcp,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SocketDirection {
    Inbound,
    Outbound,
    Both,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ResourcesSection {
    pub args: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
    #[schemars(with = "Option<Vec<String>>")]
    pub http_outbound: Option<JsonValue>,
    pub mounts: Option<Vec<ResourceMount>>,
    pub read_only_mounts: Option<Vec<ResourceMount>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceMount {
    #[schemars(required)]
    pub asset_dir: Option<String>,
    #[schemars(required)]
    pub guest_path: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityPolicy {
    #[serde(default)]
    pub privileged: bool,
    pub deps: Option<CapabilityDepsRule>,
    pub wasi: Option<CapabilityWasiRule>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CapabilityDepsRule {
    All(String),
    Table(BTreeMap<String, Vec<String>>),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CapabilityWasiRule {
    Bool(bool),
    Table(BTreeMap<String, Vec<String>>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingEntry {
    #[schemars(required)]
    pub name: Option<String>,
    #[schemars(required)]
    pub version: Option<String>,
    pub wit: Option<String>,
    pub oci: Option<String>,
    pub path: Option<String>,
    pub registry: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DependencyEntry {
    #[schemars(required)]
    pub version: Option<String>,
    #[schemars(with = "Option<DependencyKind>", required)]
    pub kind: Option<String>,
    pub wit: Option<String>,
    pub oci: Option<String>,
    pub path: Option<String>,
    pub registry: Option<String>,
    pub sha256: Option<String>,
    pub requires: Option<Vec<String>>,
    pub component: Option<DependencyComponentEntry>,
    pub capabilities: Option<CapabilityPolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DependencyComponentEntry {
    pub wit: Option<String>,
    pub oci: Option<String>,
    pub path: Option<String>,
    pub registry: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    Native,
    Wasm,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
    UnlessStopped,
}

pub fn decode_document(content: &str) -> Result<ImagoTomlDocument, toml::de::Error> {
    toml::from_str(content)
}

pub fn validate_for_build(document: &ImagoTomlDocument) -> Result<()> {
    let config = &document.config;

    if let Some(app_type) = &config.app_type {
        validate_allowed(
            app_type,
            &["cli", "http", "socket", "rpc"],
            "imago.toml key 'type' must be one of: cli, http, socket, rpc",
        )?;
    }
    if let Some(restart) = &config.restart {
        validate_allowed(
            restart,
            &["never", "on-failure", "always", "unless-stopped"],
            "imago.toml key 'restart' must be one of: never, on-failure, always, unless-stopped",
        )?;
    }

    if let Some(socket) = &config.socket {
        if let Some(protocol) = &socket.protocol {
            validate_allowed(
                protocol,
                &["udp", "tcp", "both"],
                "socket.protocol must be one of: udp, tcp, both",
            )?;
        }
        if let Some(direction) = &socket.direction {
            validate_allowed(
                direction,
                &["inbound", "outbound", "both"],
                "socket.direction must be one of: inbound, outbound, both",
            )?;
        }
    }

    if let Some(targets) = &config.target {
        for target in targets.values() {
            if let Some(remote) = &target.remote {
                validate_ssh_target_remote(remote)?;
            }
        }
    }

    if let Some(dependencies) = &config.dependencies {
        for (index, dependency) in dependencies.iter().enumerate() {
            if let Some(kind) = &dependency.kind {
                validate_allowed(
                    kind,
                    &["native", "wasm"],
                    &format!("dependencies[{index}].kind must be one of: native, wasm"),
                )?;
            }

            ensure_exactly_one_source(
                dependency.wit.as_ref(),
                dependency.oci.as_ref(),
                dependency.path.as_ref(),
                &format!("dependencies[{index}]"),
            )?;

            if let Some(component) = &dependency.component {
                ensure_exactly_one_source(
                    component.wit.as_ref(),
                    component.oci.as_ref(),
                    component.path.as_ref(),
                    &format!("dependencies[{index}].component"),
                )?;

                if matches!(dependency.kind.as_deref(), Some("native")) {
                    return Err(anyhow!(
                        "dependencies[{index}].component is only allowed when kind=\"wasm\""
                    ));
                }
            }
        }
    }

    if let Some(bindings) = &config.bindings {
        for (index, binding) in bindings.iter().enumerate() {
            ensure_exactly_one_source(
                binding.wit.as_ref(),
                binding.oci.as_ref(),
                binding.path.as_ref(),
                &format!("bindings[{index}]"),
            )?;
        }
    }

    if let Some(capabilities) = &config.capabilities {
        validate_capability_policy(capabilities, "capabilities")?;
    }

    if let Some(bindings) = &config.bindings {
        for (index, binding) in bindings.iter().enumerate() {
            let field = format!("bindings[{index}].name");
            if let Some(name) = &binding.name
                && name.trim().is_empty()
            {
                return Err(anyhow!("{field} must not be empty"));
            }
            let field = format!("bindings[{index}].version");
            if let Some(version) = &binding.version
                && version.trim().is_empty()
            {
                return Err(anyhow!("{field} must not be empty"));
            }
        }
    }

    if let Some(dependencies) = &config.dependencies {
        for (index, dependency) in dependencies.iter().enumerate() {
            let field = format!("dependencies[{index}].version");
            if let Some(version) = &dependency.version
                && version.trim().is_empty()
            {
                return Err(anyhow!("{field} must not be empty"));
            }

            if let Some(requires) = &dependency.requires {
                for (req_index, req) in requires.iter().enumerate() {
                    if req.trim().is_empty() {
                        return Err(anyhow!(
                            "dependencies[{index}].requires[{req_index}] must not be empty"
                        ));
                    }
                }
            }

            if let Some(capabilities) = &dependency.capabilities {
                validate_capability_policy(
                    capabilities,
                    &format!("dependencies[{index}].capabilities"),
                )?;
            }
        }
    }

    Ok(())
}

fn validate_capability_policy(policy: &CapabilityPolicy, field_name: &str) -> Result<()> {
    if let Some(deps) = &policy.deps {
        match deps {
            CapabilityDepsRule::All(value) => {
                if value.trim() != "*" {
                    return Err(anyhow!("{field_name}.deps must be \"*\" or a table"));
                }
            }
            CapabilityDepsRule::Table(table) => {
                validate_capability_rule_table(table, &format!("{field_name}.deps"))?
            }
        }
    }

    if let Some(wasi) = &policy.wasi {
        match wasi {
            CapabilityWasiRule::Bool(_) => {}
            CapabilityWasiRule::Table(table) => {
                validate_capability_rule_table(table, &format!("{field_name}.wasi"))?
            }
        }
    }

    Ok(())
}

fn validate_capability_rule_table(
    rules: &BTreeMap<String, Vec<String>>,
    field_name: &str,
) -> Result<()> {
    for (key, values) in rules {
        if key.trim().is_empty() {
            return Err(anyhow!("{field_name} contains an empty key"));
        }
        for (index, value) in values.iter().enumerate() {
            if value.trim().is_empty() {
                return Err(anyhow!("{field_name}.{key}[{index}] must not be empty"));
            }
        }
    }
    Ok(())
}

fn ensure_exactly_one_source(
    wit: Option<&String>,
    oci: Option<&String>,
    path: Option<&String>,
    field_base: &str,
) -> Result<()> {
    let count = [wit, oci, path].into_iter().flatten().count();
    if count == 0 {
        return Err(anyhow!(
            "{field_base} must define exactly one source key: `wit`, `oci`, or `path`"
        ));
    }
    if count > 1 {
        return Err(anyhow!(
            "{field_base} has multiple source keys; choose exactly one of `wit`, `oci`, or `path`"
        ));
    }
    Ok(())
}

fn validate_allowed(value: &str, allowed: &[&str], message: &str) -> Result<()> {
    let normalized = value.trim();
    if allowed.contains(&normalized) {
        Ok(())
    } else {
        Err(anyhow!("{message} (got: {value})"))
    }
}

fn validate_ssh_target_remote(raw: &str) -> Result<()> {
    if !raw.starts_with("ssh://") {
        return Err(anyhow!("target remote must use ssh:// scheme: {raw}"));
    }

    let parsed =
        Url::parse(raw).map_err(|err| anyhow!("target remote is invalid: {raw}: {err}"))?;
    if parsed.scheme() != "ssh" {
        return Err(anyhow!("target remote must use ssh:// scheme: {raw}"));
    }
    if parsed.password().is_some() {
        return Err(anyhow!(
            "target remote must not include a password for ssh targets"
        ));
    }
    if parsed.fragment().is_some() {
        return Err(anyhow!(
            "target remote must not include a fragment for ssh targets"
        ));
    }
    if !parsed.path().is_empty() && parsed.path() != "/" {
        return Err(anyhow!(
            "target remote must not include a path for ssh targets"
        ));
    }
    parsed
        .host_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("target remote must include a host for ssh targets"))?;

    let mut socket_seen = false;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "socket" => {
                if socket_seen {
                    return Err(anyhow!(
                        "target remote query 'socket' must not be specified more than once"
                    ));
                }
                let socket = value.trim();
                if socket.is_empty() {
                    return Err(anyhow!("target remote query 'socket' must not be empty"));
                }
                if !socket.starts_with('/') {
                    return Err(anyhow!(
                        "target remote query 'socket' must be an absolute path"
                    ));
                }
                socket_seen = true;
            }
            other => {
                return Err(anyhow!(
                    "target remote query '{}' is not supported for ssh targets",
                    other
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{decode_document, validate_for_build};

    fn decode_and_validate(content: &str) -> anyhow::Result<()> {
        let document = decode_document(content)?;
        validate_for_build(&document)
    }

    #[test]
    fn accepts_minimal_valid_config() {
        let result = decode_and_validate(
            r#"
name = "example-service"
main = "build/example-service.wasm"
type = "cli"

[target.default]
remote = "ssh://localhost?socket=/run/imago/imagod.sock"
"#,
        );
        assert!(result.is_ok(), "unexpected error: {result:?}");
    }

    #[test]
    fn rejects_binding_with_multiple_source_keys() {
        let result = decode_and_validate(
            r#"
name = "example-service"
main = "build/example-service.wasm"
type = "cli"

[target.default]
remote = "ssh://localhost?socket=/run/imago/imagod.sock"

[[bindings]]
name = "svc-a"
version = "1.0.0"
wit = "example:svc"
oci = "ghcr.io/acme/svc"
"#,
        );
        let err = result.expect_err("validation must fail");
        assert!(
            err.to_string().contains("multiple source keys"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_dependency_without_source_key() {
        let result = decode_and_validate(
            r#"
name = "example-service"
main = "build/example-service.wasm"
type = "cli"

[target.default]
remote = "ssh://localhost?socket=/run/imago/imagod.sock"

[[dependencies]]
version = "1.0.0"
kind = "wasm"
"#,
        );
        let err = result.expect_err("validation must fail");
        assert!(
            err.to_string()
                .contains("must define exactly one source key"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_invalid_restart_policy_enum_value() {
        let result = decode_and_validate(
            r#"
name = "example-service"
main = "build/example-service.wasm"
type = "cli"
restart = "sometimes"

[target.default]
remote = "ssh://localhost?socket=/run/imago/imagod.sock"
"#,
        );
        let err = result.expect_err("validation must fail");
        assert!(
            err.to_string().contains("imago.toml key 'restart'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_non_ssh_target_remote() {
        let result = decode_and_validate(
            r#"
name = "example-service"
main = "build/example-service.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        let err = result.expect_err("validation must fail");
        assert!(
            err.to_string().contains("ssh://"),
            "unexpected error: {err}"
        );
    }
}
