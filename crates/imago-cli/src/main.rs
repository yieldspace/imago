mod cli;
mod commands;

use clap::Parser;
use cli::{CertsCommands, CertsSubcommandArgs, Cli, Commands};
use commands::CommandResult;

fn dispatch(cli: Cli) -> CommandResult {
    match cli.command {
        Commands::Build(args) => commands::build::run(args),
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
    use crate::cli::{BuildArgs, DeployArgs};

    #[test]
    fn dispatches_build_and_returns_non_zero_without_imago_toml() {
        let result = dispatch(Cli {
            command: Commands::Build(BuildArgs {
                env: None,
                target: "default".to_string(),
            }),
        });

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
    }

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
        let unique = format!(
            "imago-cli-dispatch-certs-generate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let temp = std::env::temp_dir().join(unique);
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
