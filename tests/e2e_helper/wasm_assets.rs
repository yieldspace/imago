use anyhow::{Result, anyhow, bail};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub enum WasmArtifact {
    CliBase,
    CliFail,
    Http,
    RpcCaller,
    RpcCallee,
    HttpOutboundCli,
}

pub fn wasm_path(artifact: WasmArtifact) -> Result<PathBuf> {
    let key = artifact_env_key(artifact);
    let value = std::env::var_os(key).ok_or_else(|| anyhow!("missing env var: {key}"))?;
    let path = PathBuf::from(value);
    if !path.is_file() {
        bail!("wasm artifact is missing: {}", path.display());
    }
    Ok(path)
}

pub fn wasm_file_name(artifact: WasmArtifact) -> &'static str {
    match artifact {
        WasmArtifact::CliBase => "e2e_cli_base.wasm",
        WasmArtifact::CliFail => "e2e_cli_fail.wasm",
        WasmArtifact::Http => "e2e_http.wasm",
        WasmArtifact::RpcCaller => "e2e_rpc_caller.wasm",
        WasmArtifact::RpcCallee => "e2e_rpc_callee.wasm",
        WasmArtifact::HttpOutboundCli => "e2e_http_outbound_cli.wasm",
    }
}

fn artifact_env_key(artifact: WasmArtifact) -> &'static str {
    match artifact {
        WasmArtifact::CliBase => "IMAGO_E2E_WASM_CLI_BASE",
        WasmArtifact::CliFail => "IMAGO_E2E_WASM_CLI_FAIL",
        WasmArtifact::Http => "IMAGO_E2E_WASM_HTTP",
        WasmArtifact::RpcCaller => "IMAGO_E2E_WASM_RPC_CALLER",
        WasmArtifact::RpcCallee => "IMAGO_E2E_WASM_RPC_CALLEE",
        WasmArtifact::HttpOutboundCli => "IMAGO_E2E_WASM_HTTP_OUTBOUND_CLI",
    }
}
