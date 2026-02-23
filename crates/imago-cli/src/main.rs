mod cli;
mod commands;

use clap::Parser;
use cli::{
    BindingsCertCommands, BindingsCertSubcommandArgs, BindingsCommands, BindingsSubcommandArgs,
    CertsCommands, CertsSubcommandArgs, Cli, Commands,
};
use commands::CommandResult;

async fn dispatch_async(cli: Cli) -> CommandResult {
    match cli.command {
        Commands::Build(args) => commands::build::run(args),
        Commands::Update(args) => commands::update::run(args).await,
        Commands::Deploy(args) => commands::deploy::run(args).await,
        Commands::Compose(args) => commands::compose::run(args).await,
        Commands::Run(args) => commands::run::run(args).await,
        Commands::Stop(args) => commands::stop::run(args).await,
        Commands::Ps(args) => commands::ps::run(args).await,
        Commands::Logs(args) => commands::logs::run(args).await,
        Commands::Bindings(BindingsSubcommandArgs { command }) => match command {
            BindingsCommands::Cert(BindingsCertSubcommandArgs { command }) => match command {
                BindingsCertCommands::Upload(args) => {
                    commands::certs::run_bindings_cert_upload(args).await
                }
                BindingsCertCommands::Deploy(args) => {
                    commands::certs::run_bindings_cert_deploy(args).await
                }
            },
        },
        Commands::Certs(CertsSubcommandArgs { command }) => match command {
            CertsCommands::Generate(args) => commands::certs::run_generate(args),
        },
    }
}

#[cfg(test)]
async fn dispatch_with_project_root_async(
    cli: Cli,
    project_root: &std::path::Path,
) -> CommandResult {
    match cli.command {
        Commands::Build(args) => commands::build::run_with_project_root(args, project_root),
        Commands::Update(args) => commands::update::run_with_project_root(args, project_root).await,
        Commands::Deploy(args) => commands::deploy::run_with_project_root(args, project_root).await,
        Commands::Compose(args) => {
            commands::compose::run_with_project_root(args, project_root).await
        }
        Commands::Run(args) => commands::run::run_with_project_root(args, project_root).await,
        Commands::Stop(args) => commands::stop::run_with_project_root(args, project_root).await,
        Commands::Ps(args) => commands::ps::run_with_project_root(args, project_root).await,
        Commands::Logs(args) => commands::logs::run_with_project_root(args, project_root).await,
        Commands::Bindings(BindingsSubcommandArgs { command }) => match command {
            BindingsCommands::Cert(BindingsCertSubcommandArgs { command }) => match command {
                BindingsCertCommands::Upload(args) => {
                    commands::certs::run_bindings_cert_upload_with_project_root(args, project_root)
                        .await
                }
                BindingsCertCommands::Deploy(args) => {
                    commands::certs::run_bindings_cert_deploy_with_project_root(args, project_root)
                        .await
                }
            },
        },
        Commands::Certs(CertsSubcommandArgs { command }) => match command {
            CertsCommands::Generate(args) => commands::certs::run_generate(args),
        },
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    install_rustls_provider();
    let cli = Cli::parse();
    let _ = commands::ui::initialize(cli.json);
    commands::ui::emit_startup_banner(env!("CARGO_PKG_VERSION"));
    let result = dispatch_async(cli).await;
    commands::ui::finalize_result(&result);

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
    use crate::cli::{
        BindingsCertCommands, BindingsCertDeployArgs, BindingsCertSubcommandArgs,
        BindingsCertUploadArgs, BindingsCommands, BindingsSubcommandArgs, BuildArgs,
        ComposeBuildArgs, ComposeCommands, ComposeDeployArgs, ComposeLogsArgs, ComposePsArgs,
        ComposeSubcommandArgs, ComposeUpdateArgs, DeployArgs, PsArgs, RunArgs, StopArgs,
    };
    use std::path::PathBuf;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-cli-main-tests-{}-{}-{}",
            test_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    #[tokio::test]
    async fn dispatches_build_and_returns_non_zero_without_imago_toml() {
        let root = new_temp_dir("dispatch-build");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Build(BuildArgs {
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_deploy_and_returns_non_zero() {
        let root = new_temp_dir("dispatch-deploy");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Deploy(DeployArgs { target: None }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_run_and_returns_non_zero_without_imago_toml() {
        let root = new_temp_dir("dispatch-run");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Run(RunArgs {
                    name: None,
                    target: None,
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_stop_and_returns_non_zero_without_imago_toml() {
        let root = new_temp_dir("dispatch-stop");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Stop(StopArgs {
                    name: None,
                    force: false,
                    target: None,
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_ps_and_returns_non_zero_without_imago_toml() {
        let root = new_temp_dir("dispatch-ps");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Ps(PsArgs {
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_compose_deploy_and_returns_non_zero_without_imago_compose_toml() {
        let root = new_temp_dir("dispatch-compose");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Deploy(ComposeDeployArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_compose_build_and_returns_non_zero_without_imago_compose_toml() {
        let root = new_temp_dir("dispatch-compose-build");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Build(ComposeBuildArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.expect("stderr should be present");
        assert!(stderr.contains("hints:"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_compose_update_and_returns_non_zero_without_imago_compose_toml() {
        let root = new_temp_dir("dispatch-compose-update");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Update(ComposeUpdateArgs {
                        profile: "mini".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_compose_logs_and_returns_non_zero_without_imago_compose_toml() {
        let root = new_temp_dir("dispatch-compose-logs");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Logs(ComposeLogsArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                        name: None,
                        follow: false,
                        tail: 200,
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_compose_ps_and_returns_non_zero_without_imago_compose_toml() {
        let root = new_temp_dir("dispatch-compose-ps");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Ps(ComposePsArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_certs_generate_and_returns_zero() {
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

        let result = dispatch_async(Cli {
            json: false,
            command: Commands::Certs(CertsSubcommandArgs {
                command: CertsCommands::Generate(crate::cli::CertsGenerateArgs {
                    out_dir: temp.clone(),
                    force: true,
                }),
            }),
        })
        .await;

        assert_eq!(result.exit_code, 0);
        assert!(result.stderr.is_none());

        let _ = std::fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn dispatches_bindings_cert_upload_and_returns_non_zero() {
        let root = new_temp_dir("dispatch-bindings-upload");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Bindings(BindingsSubcommandArgs {
                    command: BindingsCommands::Cert(BindingsCertSubcommandArgs {
                        command: BindingsCertCommands::Upload(BindingsCertUploadArgs {
                            public_key:
                                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                                    .to_string(),
                            to: "rpc://node-a:4443".to_string(),
                        }),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be present")
                .contains("failed to load target configuration")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_bindings_cert_deploy_and_returns_non_zero() {
        let root = new_temp_dir("dispatch-bindings-deploy");
        let result = dispatch_with_project_root_async(
            Cli {
                json: false,
                command: Commands::Bindings(BindingsSubcommandArgs {
                    command: BindingsCommands::Cert(BindingsCertSubcommandArgs {
                        command: BindingsCertCommands::Deploy(BindingsCertDeployArgs {
                            to: "rpc://node-a:4443".to_string(),
                            from: "rpc://node-b:4443".to_string(),
                        }),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be present")
                .contains("failed to load target configuration")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
