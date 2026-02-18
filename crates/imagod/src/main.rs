//! `imagod` binary entrypoint.
//!
//! This binary runs in manager mode by default and switches to runner mode
//! when `--runner` is passed by the manager supervisor.

use std::path::PathBuf;

mod manager_runtime;
mod runner_runtime;
mod shutdown;

#[tokio::main]
async fn main() {
    if let Err(err) = dispatch().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn dispatch() -> Result<(), anyhow::Error> {
    install_rustls_provider();
    let cli = parse_cli_args()?;
    match cli.mode {
        RunMode::Runner => runner_runtime::run_runner().await,
        RunMode::Manager => manager_runtime::run_manager(cli.config_path).await,
    }
}

fn install_rustls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return;
    }

    let provider = web_transport_quinn::crypto::default_provider();
    if let Some(provider) = std::sync::Arc::into_inner(provider) {
        let _ = provider.install_default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Manager,
    Runner,
}

#[derive(Debug, Clone)]
struct CliArgs {
    config_path: Option<PathBuf>,
    mode: RunMode,
}

fn parse_cli_args() -> Result<CliArgs, anyhow::Error> {
    let mut args = std::env::args().skip(1);
    let mut config: Option<PathBuf> = None;
    let mut mode = RunMode::Manager;

    while let Some(arg) = args.next() {
        if arg == "--runner" {
            mode = RunMode::Runner;
            continue;
        }
        if arg == "--config" {
            let path = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--config requires a file path argument"))?;
            config = Some(PathBuf::from(path));
            continue;
        }

        if let Some(path) = arg.strip_prefix("--config=") {
            config = Some(PathBuf::from(path));
            continue;
        }
    }

    Ok(CliArgs {
        config_path: config,
        mode,
    })
}
