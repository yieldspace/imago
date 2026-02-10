mod cli;
mod commands;

use clap::Parser;
use cli::{CertsCommands, CertsSubcommandArgs, Cli, Commands};
use commands::CommandResult;

fn dispatch(cli: Cli) -> CommandResult {
    match cli.command {
        Commands::Deploy(args) => commands::deploy::run(args),
        Commands::Certs(CertsSubcommandArgs { command }) => match command {
            CertsCommands::Generate(args) => commands::certs::run_generate(args),
        },
    }
}

fn main() {
    install_rustls_provider();
    let cli = Cli::parse();
    let result = dispatch(cli);

    if let Some(message) = &result.stderr {
        eprintln!("{message}");
    }

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::DeployArgs;

    #[test]
    fn dispatches_deploy_and_returns_non_zero() {
        let result = dispatch(Cli {
            command: Commands::Deploy(DeployArgs {
                env: None,
                target: None,
            }),
        });

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
    }

    #[test]
    fn dispatches_certs_generate_and_returns_zero() {
        let temp = std::env::temp_dir().join("imago-cli-dispatch-certs-generate");
        let _ = std::fs::remove_dir_all(&temp);

        let result = dispatch(Cli {
            command: Commands::Certs(CertsSubcommandArgs {
                command: CertsCommands::Generate(crate::cli::CertsGenerateArgs {
                    out_dir: temp.clone(),
                    server_name: "localhost".to_string(),
                    server_ip: "127.0.0.1".to_string(),
                    days: 1,
                    force: true,
                }),
            }),
        });

        assert_eq!(result.exit_code, 0);
        assert!(result.stderr.is_none());

        let _ = std::fs::remove_dir_all(temp);
    }
}
