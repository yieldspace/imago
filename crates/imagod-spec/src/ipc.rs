//! Transport-agnostic IPC contracts for manager/runner coordination.

use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Component, Path, PathBuf},
};

use crate::wire::{ErrorCode, Validate, ValidationError};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

fn validate_non_empty(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::empty(field));
    }

    Ok(())
}

fn validate_non_empty_fields(fields: &[(&str, &'static str)]) -> Result<(), ValidationError> {
    for &(value, field) in fields {
        validate_non_empty(value, field)?;
    }

    Ok(())
}

fn validate_non_empty_path(path: &Path, field: &'static str) -> Result<(), ValidationError> {
    if path.as_os_str().is_empty() {
        return Err(ValidationError::empty(field));
    }

    Ok(())
}

fn validate_positive_u64(value: u64, field: &'static str) -> Result<(), ValidationError> {
    if value == 0 {
        return Err(ValidationError::invalid(field, "must be greater than zero"));
    }

    Ok(())
}

/// Arbitrary resource policy map propagated to runtime/native plugins.
pub type ResourceMap = BTreeMap<String, JsonValue>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Binding rule that allows one service to invoke an interface on another service.
pub struct ServiceBinding {
    /// Name of the destination service.
    pub name: String,
    /// WIT interface identifier that is allowed for the destination service.
    pub wit: String,
}

impl Validate for ServiceBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_fields(&[(&self.name, "name"), (&self.wit, "wit")])
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    nirvash_macros::FiniteModelDomain,
    nirvash_macros::SymbolicEncoding,
)]
/// Runtime application type carried from manifest into runner bootstrap.
pub enum RunnerAppType {
    /// Runs a one-shot CLI-style component.
    #[serde(rename = "cli")]
    Cli,
    /// Runs the RPC service entrypoint.
    #[serde(rename = "rpc")]
    Rpc,
    /// Runs the HTTP service entrypoint.
    #[serde(rename = "http")]
    Http,
    /// Runs the socket service entrypoint.
    #[serde(rename = "socket")]
    Socket,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
/// Socket runtime protocol policy.
pub enum RunnerSocketProtocol {
    #[serde(rename = "udp")]
    Udp,
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "both")]
    Both,
}

impl RunnerSocketProtocol {
    /// Returns true when UDP is allowed by this protocol policy.
    pub fn allows_udp(self) -> bool {
        matches!(self, Self::Udp | Self::Both)
    }

    /// Returns true when TCP is allowed by this protocol policy.
    pub fn allows_tcp(self) -> bool {
        matches!(self, Self::Tcp | Self::Both)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
/// Socket runtime traffic direction policy.
pub enum RunnerSocketDirection {
    #[serde(rename = "inbound")]
    Inbound,
    #[serde(rename = "outbound")]
    Outbound,
    #[serde(rename = "both")]
    Both,
}

impl RunnerSocketDirection {
    /// Returns true when inbound socket operations are allowed.
    pub fn allows_inbound(self) -> bool {
        matches!(self, Self::Inbound | Self::Both)
    }

    /// Returns true when outbound socket operations are allowed.
    pub fn allows_outbound(self) -> bool {
        matches!(self, Self::Outbound | Self::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Socket runtime execution settings used when `app_type=socket`.
pub struct RunnerSocketConfig {
    /// Allowed socket protocol set.
    pub protocol: RunnerSocketProtocol,
    /// Allowed socket traffic direction.
    pub direction: RunnerSocketDirection,
    /// Bind address required for inbound socket operations.
    pub listen_addr: String,
    /// Bind port required for inbound socket operations.
    pub listen_port: u16,
}

impl Validate for RunnerSocketConfig {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty(&self.listen_addr, "listen_addr")?;

        if self.direction.allows_inbound() && self.listen_port == 0 {
            return Err(ValidationError::invalid(
                "listen_port",
                "must be greater than zero",
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// WASI preopened directory configuration passed from manager to runner.
pub struct RunnerWasiMount {
    /// Absolute host path to the mounted directory.
    pub host_path: PathBuf,
    /// Absolute guest path exposed to component.
    pub guest_path: String,
    /// Whether this mount is read-only.
    #[serde(default)]
    pub read_only: bool,
}

impl Validate for RunnerWasiMount {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_path(&self.host_path, "host_path")?;
        validate_non_empty(&self.guest_path, "guest_path")?;
        if !self.guest_path.starts_with('/') {
            return Err(ValidationError::invalid(
                "guest_path",
                "must be an absolute path",
            ));
        }
        if self.guest_path.contains('\\') {
            return Err(ValidationError::invalid(
                "guest_path",
                "must not contain backslashes",
            ));
        }

        let path = Path::new(self.guest_path.as_str());
        for component in path.components() {
            match component {
                Component::RootDir | Component::Normal(_) => {}
                Component::CurDir | Component::ParentDir => {
                    return Err(ValidationError::invalid(
                        "guest_path",
                        "must not contain path traversal",
                    ));
                }
                _ => {
                    return Err(ValidationError::invalid(
                        "guest_path",
                        "contains unsupported path components",
                    ));
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
/// Normalized outbound rule for `wasi:http` egress authorization.
pub enum WasiHttpOutboundRule {
    /// Allows requests to one host regardless of port.
    Host { host: String },
    /// Allows requests to one host and one port.
    HostPort { host: String, port: u16 },
    /// Allows requests whose IP-literal host is contained in this CIDR range.
    Cidr {
        #[serde(with = "ip_addr_string_serde")]
        network: IpAddr,
        prefix_len: u8,
    },
}

impl WasiHttpOutboundRule {
    /// Parses one user-facing outbound rule (`host`, `host:port`, `CIDR`) and returns normalized form.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let value = raw.trim();
        if value.is_empty() {
            return Err("rule must not be empty".to_string());
        }
        if value.contains('*') {
            return Err(format!("wildcard is not supported: {value}"));
        }
        if value.chars().any(|ch| ch.is_whitespace()) {
            return Err(format!("rule must not contain whitespace: {value}"));
        }
        if value.contains('/') {
            return parse_wasi_http_outbound_cidr(value);
        }
        parse_wasi_http_outbound_host_or_port(value)
    }

    /// Returns true when this rule authorizes a request host+port pair.
    pub fn matches_authority(&self, request_host: &str, request_port: u16) -> bool {
        let normalized_request_host = normalize_wasi_http_outbound_host(request_host);
        let request_ip = request_host.parse::<IpAddr>().ok();
        match self {
            Self::Host { host } => match &normalized_request_host {
                Some(v) => v.eq_ignore_ascii_case(host),
                None => false,
            },
            Self::HostPort { host, port } => {
                if *port != request_port {
                    return false;
                }
                match &normalized_request_host {
                    Some(v) => v.eq_ignore_ascii_case(host),
                    None => false,
                }
            }
            Self::Cidr {
                network,
                prefix_len,
            } => request_ip
                .map(|ip| ip_in_cidr(ip, *network, *prefix_len))
                .unwrap_or(false),
        }
    }
}

impl Validate for WasiHttpOutboundRule {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Host { host } => {
                let normalized = normalize_wasi_http_outbound_host(host).ok_or_else(|| {
                    ValidationError::invalid("wasi_http_outbound.host", "contains invalid host")
                })?;
                if &normalized != host {
                    return Err(ValidationError::invalid(
                        "wasi_http_outbound.host",
                        "must be normalized",
                    ));
                }
            }
            Self::HostPort { host, port } => {
                let normalized = normalize_wasi_http_outbound_host(host).ok_or_else(|| {
                    ValidationError::invalid("wasi_http_outbound.host", "contains invalid host")
                })?;
                if &normalized != host {
                    return Err(ValidationError::invalid(
                        "wasi_http_outbound.host",
                        "must be normalized",
                    ));
                }
                if *port == 0 {
                    return Err(ValidationError::invalid(
                        "wasi_http_outbound.port",
                        "must be between 1 and 65535",
                    ));
                }
            }
            Self::Cidr {
                network,
                prefix_len,
            } => {
                let max_prefix = match network {
                    IpAddr::V4(_) => 32,
                    IpAddr::V6(_) => 128,
                };
                if *prefix_len > max_prefix {
                    return Err(ValidationError::invalid(
                        "wasi_http_outbound.prefix_len",
                        "is out of range for network address family",
                    ));
                }
                let normalized = cidr_network_ip(*network, *prefix_len);
                if normalized != *network {
                    return Err(ValidationError::invalid(
                        "wasi_http_outbound.network",
                        "must be normalized to network address",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn parse_wasi_http_outbound_cidr(value: &str) -> Result<WasiHttpOutboundRule, String> {
    let (ip_text, prefix_text) = value
        .split_once('/')
        .ok_or_else(|| format!("invalid CIDR rule: {value}"))?;
    if ip_text.is_empty() || prefix_text.is_empty() || prefix_text.contains('/') {
        return Err(format!("invalid CIDR rule: {value}"));
    }
    let ip = ip_text
        .parse::<IpAddr>()
        .map_err(|err| format!("invalid CIDR IP '{ip_text}': {err}"))?;
    let prefix_len = prefix_text
        .parse::<u8>()
        .map_err(|err| format!("invalid CIDR prefix '{prefix_text}': {err}"))?;
    let max_prefix = match ip {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if prefix_len > max_prefix {
        return Err(format!(
            "CIDR prefix must be in range 0..={max_prefix}: {prefix_len}"
        ));
    }
    Ok(WasiHttpOutboundRule::Cidr {
        network: cidr_network_ip(ip, prefix_len),
        prefix_len,
    })
}

fn parse_wasi_http_outbound_host_or_port(value: &str) -> Result<WasiHttpOutboundRule, String> {
    if value.starts_with('[') {
        let close_index = value
            .find(']')
            .ok_or_else(|| format!("invalid bracketed host rule: {value}"))?;
        let host_text = &value[1..close_index];
        let host_ip = host_text
            .parse::<Ipv6Addr>()
            .map_err(|err| format!("invalid IPv6 host '{host_text}': {err}"))?;
        let rest = &value[(close_index + 1)..];
        if rest.is_empty() {
            return Ok(WasiHttpOutboundRule::Host {
                host: host_ip.to_string(),
            });
        }
        let port_text = rest
            .strip_prefix(':')
            .ok_or_else(|| format!("invalid bracketed host: {value}"))?;
        let port = parse_wasi_http_outbound_port(port_text)?;
        return Ok(WasiHttpOutboundRule::HostPort {
            host: host_ip.to_string(),
            port,
        });
    }

    if value.matches(':').count() > 1 {
        let ip = value
            .parse::<IpAddr>()
            .map_err(|err| format!("IPv6 host with port must use [ipv6]:port: {value} ({err})"))?;
        return Ok(WasiHttpOutboundRule::Host {
            host: ip.to_string(),
        });
    }

    if let Some((host_text, port_text)) = value.rsplit_once(':')
        && port_text.chars().all(|ch| ch.is_ascii_digit())
    {
        let host = normalize_wasi_http_outbound_host(host_text)
            .ok_or_else(|| format!("invalid host in host:port rule: {host_text}"))?;
        let port = parse_wasi_http_outbound_port(port_text)?;
        return Ok(WasiHttpOutboundRule::HostPort { host, port });
    }

    let host = normalize_wasi_http_outbound_host(value)
        .ok_or_else(|| format!("invalid host rule: {value}"))?;
    Ok(WasiHttpOutboundRule::Host { host })
}

fn parse_wasi_http_outbound_port(text: &str) -> Result<u16, String> {
    let port = text
        .parse::<u16>()
        .map_err(|err| format!("port must be in range 1..=65535 (got '{text}'): {err}"))?;
    if port == 0 {
        return Err("port must be in range 1..=65535 (got 0)".to_string());
    }
    Ok(port)
}

mod ip_addr_string_serde {
    use std::net::IpAddr;

    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S>(value: &IpAddr, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<IpAddr, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value
            .parse::<IpAddr>()
            .map_err(|err| D::Error::custom(format!("invalid IP address: {err}")))
    }
}

fn normalize_wasi_http_outbound_host(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.contains('*')
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(|ch| ch.is_whitespace())
    {
        return None;
    }
    if value.starts_with('[') || value.ends_with(']') {
        return None;
    }
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    if value.contains(':') {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

fn cidr_network_ip(ip: IpAddr, prefix_len: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let mask = if prefix_len == 0 {
                0
            } else {
                u32::MAX << u32::from(32_u8.saturating_sub(prefix_len))
            };
            IpAddr::V4(Ipv4Addr::from(bits & mask))
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let mask = if prefix_len == 0 {
                0
            } else {
                u128::MAX << u32::from(128_u8.saturating_sub(prefix_len))
            };
            IpAddr::V6(Ipv6Addr::from(bits & mask))
        }
    }
}

fn ip_in_cidr(ip: IpAddr, network: IpAddr, prefix_len: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip_v4), IpAddr::V4(net_v4)) => {
            let ip_bits = u32::from(ip_v4);
            let net_bits = u32::from(net_v4);
            let mask = if prefix_len == 0 {
                0
            } else {
                u32::MAX << u32::from(32_u8.saturating_sub(prefix_len))
            };
            (ip_bits & mask) == (net_bits & mask)
        }
        (IpAddr::V6(ip_v6), IpAddr::V6(net_v6)) => {
            let ip_bits = u128::from(ip_v6);
            let net_bits = u128::from(net_v6);
            let mask = if prefix_len == 0 {
                0
            } else {
                u128::MAX << u32::from(128_u8.saturating_sub(prefix_len))
            };
            (ip_bits & mask) == (net_bits & mask)
        }
        _ => false,
    }
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    nirvash_macros::FiniteModelDomain,
    nirvash_macros::SymbolicEncoding,
)]
#[serde(rename_all = "lowercase")]
/// Plugin delivery kind used by manifest/bootstrap dependency definitions.
pub enum PluginKind {
    /// Loads a native host plugin.
    Native,
    /// Loads a Wasm component plugin.
    Wasm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Wasm plugin component descriptor.
pub struct PluginComponent {
    /// Component path. In manifest this is release-relative; in runner bootstrap this is absolute.
    pub path: PathBuf,
    /// Hex-encoded SHA-256 digest for component bytes.
    pub sha256: String,
    /// Imported component instance interface names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imports: Option<Vec<String>>,
    /// Exported component instance interface names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exports: Option<Vec<String>>,
}

impl Validate for PluginComponent {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_path(&self.path, "path")?;
        validate_non_empty(&self.sha256, "sha256")?;
        if let Some(imports) = &self.imports {
            for import in imports {
                validate_non_empty(import, "imports")?;
            }
        }
        if let Some(exports) = &self.exports {
            for export in exports {
                validate_non_empty(export, "exports")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
/// Function-level capability policy used by app/plugin callers.
pub struct CapabilityPolicy {
    /// When true, all dependency and WASI calls are allowed.
    #[serde(default)]
    pub privileged: bool,
    /// Dependency plugin function permissions.
    #[serde(default)]
    pub deps: BTreeMap<String, Vec<String>>,
    /// WASI interface function permissions.
    #[serde(default)]
    pub wasi: BTreeMap<String, Vec<String>>,
}

impl Validate for CapabilityPolicy {
    fn validate(&self) -> Result<(), ValidationError> {
        for (dependency, functions) in &self.deps {
            validate_non_empty(dependency, "deps.key")?;
            for function in functions {
                validate_non_empty(function, "deps.function")?;
            }
        }

        for (interface, functions) in &self.wasi {
            validate_non_empty(interface, "wasi.key")?;
            for function in functions {
                validate_non_empty(function, "wasi.function")?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// One plugin dependency definition passed to runner runtime.
pub struct PluginDependency {
    /// Canonical package name.
    pub name: String,
    /// Dependency version string.
    pub version: String,
    /// Delivery kind.
    pub kind: PluginKind,
    /// WIT source identifier.
    pub wit: String,
    /// Additional plugin package dependencies required by this plugin.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Wasm component descriptor for `kind=wasm`.
    #[serde(default)]
    pub component: Option<PluginComponent>,
    /// Capability policy used when this plugin is the caller.
    #[serde(default)]
    pub capabilities: CapabilityPolicy,
}

impl Validate for PluginDependency {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_fields(&[
            (&self.name, "name"),
            (&self.version, "version"),
            (&self.wit, "wit"),
        ])?;
        for requirement in &self.requires {
            validate_non_empty(requirement, "requires")?;
        }
        self.capabilities.validate()?;

        if matches!(self.kind, PluginKind::Wasm) {
            let component = self
                .component
                .as_ref()
                .ok_or(ValidationError::missing("component"))?;
            component.validate()?;
        } else if let Some(component) = &self.component {
            component.validate()?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Bootstrap payload sent from manager to runner via child stdin.
pub struct RunnerBootstrap {
    /// Ephemeral runner identifier generated by manager.
    pub runner_id: String,
    /// Service name owned by this runner.
    pub service_name: String,
    /// Release hash to be executed.
    pub release_hash: String,
    /// Runtime execution model selected from manifest `type`.
    pub app_type: RunnerAppType,
    /// TCP port used by local HTTP ingress when `app_type=http`.
    pub http_port: Option<u16>,
    /// Max accepted HTTP request body size in bytes when `app_type=http`.
    pub http_max_body_bytes: Option<u64>,
    /// Compatibility worker-count value forwarded from manager configuration.
    pub http_worker_count: u32,
    /// Effective HTTP request queue capacity for ingress dispatch.
    pub http_worker_queue_capacity: u32,
    /// Socket runtime configuration when `app_type=socket`.
    pub socket: Option<RunnerSocketConfig>,
    /// Absolute path to the component file.
    pub component_path: PathBuf,
    /// CLI arguments passed to WASI command.
    pub args: Vec<String>,
    /// Environment variables passed to the runtime.
    pub envs: std::collections::BTreeMap<String, String>,
    /// WASI preopened directory mounts.
    #[serde(default)]
    pub wasi_mounts: Vec<RunnerWasiMount>,
    /// Allowed outbound rules for `wasi:http` requests.
    #[serde(default)]
    pub wasi_http_outbound: Vec<WasiHttpOutboundRule>,
    /// Arbitrary resource policy map available to runtime/native plugins.
    #[serde(default)]
    pub resources: ResourceMap,
    /// Allowed outbound bindings for this service.
    pub bindings: Vec<ServiceBinding>,
    /// Plugin dependencies available for this app/plugin execution context.
    #[serde(default)]
    pub plugin_dependencies: Vec<PluginDependency>,
    /// App-level capability policy.
    #[serde(default)]
    pub capabilities: CapabilityPolicy,
    /// Manager control socket endpoint.
    pub manager_control_endpoint: PathBuf,
    /// Runner inbound socket endpoint.
    pub runner_endpoint: PathBuf,
    /// Shared secret used for manager-auth proof generation.
    pub manager_auth_secret: String,
    /// Secret used by manager to sign invocation tokens for this runner.
    pub invocation_secret: String,
    /// Epoch tick interval used by the runner runtime loop.
    pub epoch_tick_interval_ms: u64,
    /// Wasmtime linear-memory reservation size in bytes.
    pub wasm_memory_reservation_bytes: u64,
    /// Wasmtime extra reservation size for linear-memory growth in bytes.
    pub wasm_memory_reservation_for_growth_bytes: u64,
    /// Wasmtime linear-memory guard size in bytes.
    pub wasm_memory_guard_size_bytes: u64,
    /// Whether Wasmtime reserves a guard region before linear memory.
    pub wasm_guard_before_linear_memory: bool,
    /// Whether Wasmtime compiles modules in parallel.
    pub wasm_parallel_compilation: bool,
}

impl Validate for RunnerBootstrap {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_fields(&[
            (&self.runner_id, "runner_id"),
            (&self.service_name, "service_name"),
            (&self.release_hash, "release_hash"),
            (&self.manager_auth_secret, "manager_auth_secret"),
            (&self.invocation_secret, "invocation_secret"),
        ])?;
        validate_non_empty_path(&self.component_path, "component_path")?;
        validate_non_empty_path(&self.manager_control_endpoint, "manager_control_endpoint")?;
        validate_non_empty_path(&self.runner_endpoint, "runner_endpoint")?;
        validate_positive_u64(self.epoch_tick_interval_ms, "epoch_tick_interval_ms")?;
        validate_positive_u64(
            self.wasm_memory_reservation_bytes,
            "wasm_memory_reservation_bytes",
        )?;
        validate_positive_u64(
            self.wasm_memory_reservation_for_growth_bytes,
            "wasm_memory_reservation_for_growth_bytes",
        )?;
        if !(1..=4).contains(&self.http_worker_count) {
            return Err(ValidationError::invalid(
                "http_worker_count",
                "must be between 1 and 4",
            ));
        }
        if !(1..=16).contains(&self.http_worker_queue_capacity) {
            return Err(ValidationError::invalid(
                "http_worker_queue_capacity",
                "must be between 1 and 16",
            ));
        }

        match self.app_type {
            RunnerAppType::Http => {
                let port = self
                    .http_port
                    .ok_or(ValidationError::missing("http_port"))?;
                if port == 0 {
                    return Err(ValidationError::invalid(
                        "http_port",
                        "must be greater than zero",
                    ));
                }

                if let Some(max_body_bytes) = self.http_max_body_bytes {
                    validate_positive_u64(max_body_bytes, "http_max_body_bytes")?;
                }
            }
            RunnerAppType::Socket => {
                let socket = self
                    .socket
                    .as_ref()
                    .ok_or(ValidationError::missing("socket"))?;
                socket.validate()?;
            }
            RunnerAppType::Cli | RunnerAppType::Rpc => {}
        }

        for binding in &self.bindings {
            binding.validate()?;
        }
        for mount in &self.wasi_mounts {
            mount.validate()?;
        }
        for rule in &self.wasi_http_outbound {
            rule.validate()?;
        }
        for key in self.resources.keys() {
            validate_non_empty(key, "resources.key")?;
        }
        for dependency in &self.plugin_dependencies {
            dependency.validate()?;
        }
        self.capabilities.validate()?;

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Control-plane request from runner to manager.
pub enum ControlRequest {
    /// Registers a newly spawned runner endpoint.
    RegisterRunner {
        /// Runner identifier.
        runner_id: String,
        /// Service name attached to runner.
        service_name: String,
        /// Release hash announced by runner.
        release_hash: String,
        /// Endpoint where runner accepts inbound requests.
        runner_endpoint: PathBuf,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Marks the runner as ready to execute workload.
    RunnerReady {
        /// Runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Sends runner liveness information.
    Heartbeat {
        /// Runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Resolves destination runner endpoint and invocation token.
    ResolveInvocationTarget {
        /// Caller runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
        /// Destination service name.
        target_service: String,
        /// Requested WIT interface identifier.
        wit: String,
    },
    /// Establishes one manager-side remote RPC connection handle.
    RpcConnectRemote {
        /// Caller runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
        /// Remote authority in `rpc://host:port` format.
        authority: String,
    },
    /// Invokes one function through a manager-side remote RPC connection.
    RpcInvokeRemote {
        /// Caller runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
        /// Manager-issued remote connection id.
        connection_id: String,
        /// Destination service name.
        target_service: String,
        /// Requested WIT interface identifier.
        interface_id: String,
        /// Target function name.
        function: String,
        /// CBOR-encoded invoke payload.
        #[serde(default)]
        args_cbor: Vec<u8>,
    },
    /// Closes one manager-side remote RPC connection handle.
    RpcDisconnectRemote {
        /// Caller runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
        /// Manager-issued remote connection id.
        connection_id: String,
    },
}

impl Validate for ControlRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::RegisterRunner {
                runner_id,
                service_name,
                release_hash,
                runner_endpoint,
                manager_auth_proof,
            } => {
                validate_non_empty_fields(&[
                    (runner_id, "runner_id"),
                    (service_name, "service_name"),
                    (release_hash, "release_hash"),
                    (manager_auth_proof, "manager_auth_proof"),
                ])?;
                validate_non_empty_path(runner_endpoint, "runner_endpoint")
            }
            Self::RunnerReady {
                runner_id,
                manager_auth_proof,
            }
            | Self::Heartbeat {
                runner_id,
                manager_auth_proof,
            } => validate_non_empty_fields(&[
                (runner_id, "runner_id"),
                (manager_auth_proof, "manager_auth_proof"),
            ]),
            Self::ResolveInvocationTarget {
                runner_id,
                manager_auth_proof,
                target_service,
                wit,
            } => validate_non_empty_fields(&[
                (runner_id, "runner_id"),
                (manager_auth_proof, "manager_auth_proof"),
                (target_service, "target_service"),
                (wit, "wit"),
            ]),
            Self::RpcConnectRemote {
                runner_id,
                manager_auth_proof,
                authority,
            } => validate_non_empty_fields(&[
                (runner_id, "runner_id"),
                (manager_auth_proof, "manager_auth_proof"),
                (authority, "authority"),
            ]),
            Self::RpcInvokeRemote {
                runner_id,
                manager_auth_proof,
                connection_id,
                target_service,
                interface_id,
                function,
                ..
            } => validate_non_empty_fields(&[
                (runner_id, "runner_id"),
                (manager_auth_proof, "manager_auth_proof"),
                (connection_id, "connection_id"),
                (target_service, "target_service"),
                (interface_id, "interface_id"),
                (function, "function"),
            ]),
            Self::RpcDisconnectRemote {
                runner_id,
                manager_auth_proof,
                connection_id,
            } => validate_non_empty_fields(&[
                (runner_id, "runner_id"),
                (manager_auth_proof, "manager_auth_proof"),
                (connection_id, "connection_id"),
            ]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Control-plane response returned by manager.
pub enum ControlResponse {
    /// Generic success response for one-way commands.
    Ack,
    /// Resolved target endpoint plus short-lived authorization token.
    ResolvedInvocationTarget {
        /// Destination runner endpoint.
        endpoint: PathBuf,
        /// Authorization token scoped for one invocation target.
        token: String,
    },
    /// One remote connection handle has been created by manager.
    RpcRemoteConnected {
        /// Manager-issued remote connection id.
        connection_id: String,
    },
    /// One remote invoke call result.
    RpcRemoteInvokeResult {
        /// CBOR-encoded invoke result payload.
        #[serde(default)]
        result_cbor: Vec<u8>,
    },
    /// Structured control-plane failure.
    Error(IpcErrorPayload),
}

impl Validate for ControlResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Ack => Ok(()),
            Self::ResolvedInvocationTarget { endpoint, token } => {
                validate_non_empty_path(endpoint, "endpoint")?;
                validate_non_empty(token, "token")
            }
            Self::RpcRemoteConnected { connection_id } => {
                validate_non_empty(connection_id, "connection_id")
            }
            Self::RpcRemoteInvokeResult { .. } => Ok(()),
            Self::Error(error) => error.validate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Runner inbound request issued by manager or other runners.
pub enum RunnerInboundRequest {
    /// Requests graceful runner shutdown.
    ShutdownRunner {
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Requests interface function invocation on the runner.
    Invoke {
        /// Interface identifier to invoke.
        interface_id: String,
        /// Function name inside the interface.
        function: String,
        /// Raw CBOR payload for invocation arguments.
        payload_cbor: Vec<u8>,
        /// Manager-signed authorization token.
        token: String,
    },
}

impl Validate for RunnerInboundRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::ShutdownRunner { manager_auth_proof } => {
                validate_non_empty(manager_auth_proof, "manager_auth_proof")
            }
            Self::Invoke {
                interface_id,
                function,
                token,
                ..
            } => validate_non_empty_fields(&[
                (interface_id, "interface_id"),
                (function, "function"),
                (token, "token"),
            ]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Runner inbound response.
pub enum RunnerInboundResponse {
    /// Generic success response.
    Ack,
    /// Invocation result payload encoded as CBOR.
    InvokeResult {
        /// Raw CBOR payload returned by invocation.
        payload_cbor: Vec<u8>,
    },
    /// Structured runner-side failure.
    Error(IpcErrorPayload),
}

impl Validate for RunnerInboundResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Ack => Ok(()),
            Self::InvokeResult { .. } => Ok(()),
            Self::Error(error) => error.validate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Wire-safe error payload for IPC responses.
pub struct IpcErrorPayload {
    /// Protocol error code.
    pub code: ErrorCode,
    /// Stage identifier for diagnostics.
    pub stage: String,
    /// Human-readable message.
    pub message: String,
}

impl Validate for IpcErrorPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_fields(&[(&self.stage, "stage"), (&self.message, "message")])
    }
}

impl IpcErrorPayload {
    /// Returns true when this payload is retryable according to the wire code.
    pub const fn retryable_by_default(&self) -> bool {
        matches!(self.code, ErrorCode::Busy | ErrorCode::OperationTimeout)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Claims embedded in manager-signed invocation tokens.
pub struct InvocationTokenClaims {
    /// Calling service name.
    pub source_service: String,
    /// Destination service name.
    pub target_service: String,
    /// Authorized WIT interface identifier.
    pub wit: String,
    /// Expiration epoch (seconds).
    pub exp: u64,
    /// Unique nonce to prevent token reuse patterns.
    pub nonce: String,
}

impl Validate for InvocationTokenClaims {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_fields(&[
            (&self.source_service, "source_service"),
            (&self.target_service, "target_service"),
            (&self.wit, "wit"),
            (&self.nonce, "nonce"),
        ])?;
        validate_positive_u64(self.exp, "exp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_lower::FiniteModelDomain;

    #[test]
    fn runner_app_type_domain_matches_expected_order() {
        assert_eq!(
            RunnerAppType::bounded_domain().into_vec(),
            vec![
                RunnerAppType::Cli,
                RunnerAppType::Rpc,
                RunnerAppType::Http,
                RunnerAppType::Socket,
            ]
        );
    }

    #[test]
    fn plugin_kind_domain_matches_expected_order() {
        assert_eq!(
            PluginKind::bounded_domain().into_vec(),
            vec![PluginKind::Native, PluginKind::Wasm]
        );
    }
}
