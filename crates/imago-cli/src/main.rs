mod cli;
mod commands;
mod lockfile;

use clap::Parser;
use cli::{
    ArtifactCommands, ArtifactSubcommandArgs, Cli, Commands, DepsCommands, DepsSubcommandArgs,
    ProjectCommands, ProjectSubcommandArgs, ServiceCommands, ServiceSubcommandArgs,
    TrustCertCommands, TrustCertSubcommandArgs, TrustClientKeyCommands,
    TrustClientKeySubcommandArgs, TrustCommands, TrustSubcommandArgs,
};
use commands::CommandResult;

async fn dispatch_async(cli: Cli) -> CommandResult {
    match cli.command {
        Commands::Project(ProjectSubcommandArgs { command }) => match command {
            ProjectCommands::Init(args) => commands::init::run(args),
        },
        Commands::Artifact(ArtifactSubcommandArgs { command }) => match command {
            ArtifactCommands::Build(args) => commands::build::run(args),
        },
        Commands::Deps(DepsSubcommandArgs { command }) => match command {
            DepsCommands::Sync(args) => commands::update::run(args).await,
        },
        Commands::Service(ServiceSubcommandArgs { command }) => match command {
            ServiceCommands::Deploy(args) => commands::deploy::run(args).await,
            ServiceCommands::Start(args) => commands::run::run(args).await,
            ServiceCommands::Stop(args) => commands::stop::run(args).await,
            ServiceCommands::Ls(args) => commands::ps::run(args).await,
            ServiceCommands::Logs(args) => commands::logs::run(args).await,
        },
        Commands::Stack(args) => commands::compose::run(args).await,
        Commands::Trust(TrustSubcommandArgs { command }) => match command {
            TrustCommands::Cert(TrustCertSubcommandArgs { command }) => match command {
                TrustCertCommands::Upload(args) => {
                    commands::certs::run_bindings_cert_upload(args).await
                }
                TrustCertCommands::Replicate(args) => {
                    commands::certs::run_bindings_cert_deploy(args).await
                }
            },
            TrustCommands::ClientKey(TrustClientKeySubcommandArgs { command }) => match command {
                TrustClientKeyCommands::Generate(args) => commands::certs::run_generate(args),
            },
        },
    }
}

#[cfg(test)]
async fn dispatch_with_project_root_async(
    cli: Cli,
    project_root: &std::path::Path,
) -> CommandResult {
    match cli.command {
        Commands::Project(ProjectSubcommandArgs { command }) => match command {
            ProjectCommands::Init(args) => commands::init::run_with_cwd(args, project_root),
        },
        Commands::Artifact(ArtifactSubcommandArgs { command }) => match command {
            ArtifactCommands::Build(args) => {
                commands::build::run_with_project_root(args, project_root)
            }
        },
        Commands::Deps(DepsSubcommandArgs { command }) => match command {
            DepsCommands::Sync(args) => {
                commands::update::run_with_project_root(args, project_root).await
            }
        },
        Commands::Service(ServiceSubcommandArgs { command }) => match command {
            ServiceCommands::Deploy(args) => {
                commands::deploy::run_with_project_root(args, project_root).await
            }
            ServiceCommands::Start(args) => {
                commands::run::run_with_project_root(args, project_root).await
            }
            ServiceCommands::Stop(args) => {
                commands::stop::run_with_project_root(args, project_root).await
            }
            ServiceCommands::Ls(args) => {
                commands::ps::run_with_project_root(args, project_root).await
            }
            ServiceCommands::Logs(args) => {
                commands::logs::run_with_project_root(args, project_root).await
            }
        },
        Commands::Stack(args) => commands::compose::run_with_project_root(args, project_root).await,
        Commands::Trust(TrustSubcommandArgs { command }) => match command {
            TrustCommands::Cert(TrustCertSubcommandArgs { command }) => match command {
                TrustCertCommands::Upload(args) => {
                    commands::certs::run_bindings_cert_upload_with_project_root(args, project_root)
                        .await
                }
                TrustCertCommands::Replicate(args) => {
                    commands::certs::run_bindings_cert_deploy_with_project_root(args, project_root)
                        .await
                }
            },
            TrustCommands::ClientKey(TrustClientKeySubcommandArgs { command }) => match command {
                TrustClientKeyCommands::Generate(args) => commands::certs::run_generate(args),
            },
        },
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    install_rustls_provider();
    let cli = Cli::parse();

    let _ = commands::ui::initialize();
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
        ArtifactCommands, ArtifactSubcommandArgs, BindingsCertDeployArgs, BindingsCertUploadArgs,
        BuildArgs, CertsGenerateArgs, ComposeBuildArgs, ComposeCommands, ComposeDeployArgs,
        ComposeLogsArgs, ComposePsArgs, ComposeSubcommandArgs, ComposeUpdateArgs, DeployArgs,
        DepsCommands, DepsSubcommandArgs, InitArgs, LogsArgs, ProjectCommands,
        ProjectSubcommandArgs, PsArgs, RunArgs, ServiceCommands, ServiceSubcommandArgs, StopArgs,
        TrustCertCommands, TrustCertSubcommandArgs, TrustClientKeyCommands,
        TrustClientKeySubcommandArgs, TrustCommands, TrustSubcommandArgs, UpdateArgs,
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
    async fn dispatches_project_init_and_creates_imago_toml() {
        let root = new_temp_dir("dispatch-init-success");
        let result = dispatch_with_project_root_async(
            Cli {
                command: Commands::Project(ProjectSubcommandArgs {
                    command: ProjectCommands::Init(InitArgs {
                        path: Some(PathBuf::from("svc-a")),
                        template: Some("rust".to_string()),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.command, "project.init");
        assert!(result.stderr.is_none());
        assert!(root.join("svc-a").join("imago.toml").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_artifact_build_and_returns_non_zero_without_imago_toml() {
        let root = new_temp_dir("dispatch-build");
        let result = dispatch_with_project_root_async(
            Cli {
                command: Commands::Artifact(ArtifactSubcommandArgs {
                    command: ArtifactCommands::Build(BuildArgs {
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert_eq!(result.command, "artifact.build");
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_deps_sync_and_returns_non_zero_without_imago_toml() {
        let root = new_temp_dir("dispatch-deps-sync");
        let result = dispatch_with_project_root_async(
            Cli {
                command: Commands::Deps(DepsSubcommandArgs {
                    command: DepsCommands::Sync(UpdateArgs {}),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert_eq!(result.command, "deps.sync");
        assert!(result.stderr.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_service_subcommands_and_returns_non_zero_without_project() {
        let root = new_temp_dir("dispatch-service");

        let deploy = dispatch_with_project_root_async(
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Deploy(DeployArgs {
                        target: None,
                        detach: false,
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(deploy.exit_code, 2);
        assert_eq!(deploy.command, "service.deploy");

        let start = dispatch_with_project_root_async(
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Start(RunArgs {
                        name: None,
                        target: None,
                        detach: false,
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(start.exit_code, 2);
        assert_eq!(start.command, "service.start");

        let stop = dispatch_with_project_root_async(
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Stop(StopArgs {
                        name: None,
                        force: false,
                        target: None,
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(stop.exit_code, 2);
        assert_eq!(stop.command, "service.stop");

        let ls = dispatch_with_project_root_async(
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Ls(PsArgs {
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(ls.exit_code, 2);
        assert_eq!(ls.command, "service.ls");

        let logs = dispatch_with_project_root_async(
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Logs(LogsArgs {
                        name: None,
                        follow: false,
                        tail: 200,
                        with_timestamp: false,
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(logs.exit_code, 2);
        assert_eq!(logs.command, "service.logs");

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_stack_subcommands_and_returns_non_zero_without_imago_compose_toml() {
        let root = new_temp_dir("dispatch-stack");

        let deploy = dispatch_with_project_root_async(
            Cli {
                command: Commands::Stack(ComposeSubcommandArgs {
                    command: ComposeCommands::Deploy(ComposeDeployArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(deploy.exit_code, 2);

        let build = dispatch_with_project_root_async(
            Cli {
                command: Commands::Stack(ComposeSubcommandArgs {
                    command: ComposeCommands::Build(ComposeBuildArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(build.exit_code, 2);

        let sync = dispatch_with_project_root_async(
            Cli {
                command: Commands::Stack(ComposeSubcommandArgs {
                    command: ComposeCommands::Sync(ComposeUpdateArgs {
                        profile: "mini".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(sync.exit_code, 2);

        let logs = dispatch_with_project_root_async(
            Cli {
                command: Commands::Stack(ComposeSubcommandArgs {
                    command: ComposeCommands::Logs(ComposeLogsArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                        name: None,
                        follow: false,
                        tail: 200,
                        with_timestamp: false,
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(logs.exit_code, 2);

        let ls = dispatch_with_project_root_async(
            Cli {
                command: Commands::Stack(ComposeSubcommandArgs {
                    command: ComposeCommands::Ls(ComposePsArgs {
                        profile: "mini".to_string(),
                        target: "default".to_string(),
                    }),
                }),
            },
            &root,
        )
        .await;
        assert_eq!(ls.exit_code, 2);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatches_trust_client_key_generate_and_returns_zero() {
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
            command: Commands::Trust(TrustSubcommandArgs {
                command: TrustCommands::ClientKey(TrustClientKeySubcommandArgs {
                    command: TrustClientKeyCommands::Generate(CertsGenerateArgs {
                        out_dir: temp.clone(),
                        force: true,
                    }),
                }),
            }),
        })
        .await;

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.command, "trust.client-key.generate");
        assert!(result.stderr.is_none());

        let _ = std::fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn dispatches_trust_cert_upload_and_returns_non_zero() {
        let root = new_temp_dir("dispatch-trust-upload");
        let result = dispatch_with_project_root_async(
            Cli {
                command: Commands::Trust(TrustSubcommandArgs {
                    command: TrustCommands::Cert(TrustCertSubcommandArgs {
                        command: TrustCertCommands::Upload(BindingsCertUploadArgs {
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
        assert_eq!(result.command, "trust.cert.upload");
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
    async fn dispatches_trust_cert_replicate_and_returns_non_zero() {
        let root = new_temp_dir("dispatch-trust-replicate");
        let result = dispatch_with_project_root_async(
            Cli {
                command: Commands::Trust(TrustSubcommandArgs {
                    command: TrustCommands::Cert(TrustCertSubcommandArgs {
                        command: TrustCertCommands::Replicate(BindingsCertDeployArgs {
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
        assert_eq!(result.command, "trust.cert.replicate");
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
