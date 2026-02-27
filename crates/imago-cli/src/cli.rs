use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Build, update, and operate imago services.
#[derive(Debug, Parser, PartialEq, Eq)]
#[command(name = "imago", version, about = "imago CLI")]
pub struct Cli {
    /// Command to execute.
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Commands {
    /// Project lifecycle operations.
    Project(ProjectSubcommandArgs),
    /// Artifact lifecycle operations.
    Artifact(ArtifactSubcommandArgs),
    /// Dependency synchronization operations.
    Deps(DepsSubcommandArgs),
    /// Service runtime operations.
    Service(ServiceSubcommandArgs),
    /// Stack profile operations across multiple services.
    Stack(ComposeSubcommandArgs),
    /// Trust and certificate operations.
    Trust(TrustSubcommandArgs),
}

/// Project lifecycle subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ProjectSubcommandArgs {
    /// Project operation to run.
    #[command(subcommand)]
    pub command: ProjectCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum ProjectCommands {
    /// Generate imago.toml from a template.
    Init(InitArgs),
}

/// Artifact lifecycle subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ArtifactSubcommandArgs {
    /// Artifact operation to run.
    #[command(subcommand)]
    pub command: ArtifactCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum ArtifactCommands {
    /// Build project artifacts and manifest.
    Build(BuildArgs),
}

/// Dependency lifecycle subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct DepsSubcommandArgs {
    /// Dependency operation to run.
    #[command(subcommand)]
    pub command: DepsCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum DepsCommands {
    /// Resolve dependencies and refresh lock/cache state.
    Sync(UpdateArgs),
}

/// Service lifecycle subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ServiceSubcommandArgs {
    /// Service operation to run.
    #[command(subcommand)]
    pub command: ServiceCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum ServiceCommands {
    /// Build and deploy the current service to imagod.
    Deploy(DeployArgs),
    /// Start a deployed service instance.
    Start(RunArgs),
    /// Stop a running service instance.
    Stop(StopArgs),
    /// List deployed service states.
    Ls(PsArgs),
    /// Stream or tail service logs.
    Logs(LogsArgs),
}

/// Initialize a project with imago.toml.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct InitArgs {
    /// Destination directory. If omitted or ".", writes to current directory.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Template language ID (for example: rust, generic).
    #[arg(long, value_name = "LANG_ID")]
    pub lang: Option<String>,
}

/// Build artifacts for a service project.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BuildArgs {
    /// Target name defined in imago.toml [target.<name>].
    #[arg(long, value_name = "TARGET_NAME", default_value = "default")]
    pub target: String,
}

/// Resolve dependencies and update lock/cache files.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct UpdateArgs {}

/// Build and deploy the service to a remote imagod.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct DeployArgs {
    /// Target name defined in imago.toml [target.<name>].
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,

    /// Return immediately after deploy succeeds without following logs.
    #[arg(short = 'd', long)]
    pub detach: bool,
}

/// Start a deployed service.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct RunArgs {
    /// Service name. If omitted, uses the project default service name.
    #[arg(value_name = "SERVICE_NAME")]
    pub name: Option<String>,

    /// Target name defined in imago.toml [target.<name>].
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,

    /// Return immediately after run succeeds without following logs.
    #[arg(short = 'd', long)]
    pub detach: bool,
}

/// Stop a running service.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct StopArgs {
    /// Service name. If omitted, uses the project default service name.
    #[arg(value_name = "SERVICE_NAME")]
    pub name: Option<String>,

    /// Force stop even if graceful shutdown fails.
    #[arg(long)]
    pub force: bool,

    /// Target name defined in imago.toml [target.<name>].
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
}

/// List deployed service states.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct PsArgs {
    /// Target name defined in imago.toml [target.<name>].
    #[arg(long, value_name = "TARGET_NAME", default_value = "default")]
    pub target: String,
}

/// Compose profile subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeSubcommandArgs {
    /// Compose operation to run.
    #[command(subcommand)]
    pub command: ComposeCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum ComposeCommands {
    /// Build all services in a compose profile.
    Build(ComposeBuildArgs),
    /// Sync dependencies for all services in a compose profile.
    Sync(ComposeUpdateArgs),
    /// Deploy all services in a compose profile.
    Deploy(ComposeDeployArgs),
    /// Stream or tail logs for services in a compose profile.
    Logs(ComposeLogsArgs),
    /// List deployed service states in a compose profile.
    Ls(ComposePsArgs),
}

/// Build services for a compose profile.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeBuildArgs {
    /// Compose profile name.
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,

    /// Target name used for all services in this profile.
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: String,
}

/// Sync dependencies for services in a compose profile.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeUpdateArgs {
    /// Compose profile name.
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,
}

/// Deploy services in a compose profile.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeDeployArgs {
    /// Compose profile name.
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,

    /// Target name used for all services in this profile.
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: String,
}

/// Stream or tail logs for services in a compose profile.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeLogsArgs {
    /// Compose profile name.
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,

    /// Target name used for all services in this profile.
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: String,

    /// Optional service name filter. If omitted, streams all running services.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Keep streaming logs until interrupted.
    #[arg(short = 'f', long)]
    pub follow: bool,

    /// Number of recent log lines to fetch before streaming.
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub tail: u32,

    /// Include per-log timestamp from server events.
    #[arg(long)]
    pub with_timestamp: bool,
}

/// List deployed service states in a compose profile.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposePsArgs {
    /// Compose profile name.
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,

    /// Target name used for all services in this profile.
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: String,
}

/// Stream or tail service logs.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct LogsArgs {
    /// Optional service name filter. If omitted, streams all running services.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Keep streaming logs until interrupted.
    #[arg(short = 'f', long)]
    pub follow: bool,

    /// Number of recent log lines to fetch before streaming.
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub tail: u32,

    /// Include per-log timestamp from server events.
    #[arg(long)]
    pub with_timestamp: bool,
}

/// Trust subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct TrustSubcommandArgs {
    /// Trust operation to run.
    #[command(subcommand)]
    pub command: TrustCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum TrustCommands {
    /// Manage binding certificate operations.
    Cert(TrustCertSubcommandArgs),
    /// Generate local development client key material.
    #[command(name = "client-key")]
    ClientKey(TrustClientKeySubcommandArgs),
}

/// Trust certificate subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct TrustCertSubcommandArgs {
    /// Trust certificate operation to run.
    #[command(subcommand)]
    pub command: TrustCertCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum TrustCertCommands {
    /// Upload a public key to a remote authority.
    Upload(BindingsCertUploadArgs),
    /// Copy a binding certificate from one authority to another.
    Replicate(BindingsCertDeployArgs),
}

/// Trust client key subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct TrustClientKeySubcommandArgs {
    /// Trust client key operation to run.
    #[command(subcommand)]
    pub command: TrustClientKeyCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum TrustClientKeyCommands {
    /// Generate a local client key for imago-cli authentication.
    Generate(CertsGenerateArgs),
}

/// Upload a binding public key.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsCertUploadArgs {
    /// Public key in 64-byte hex format.
    #[arg(value_name = "PUBLIC_KEY_HEX")]
    pub public_key: String,

    /// Destination remote authority (for example: rpc://node-a:4443).
    #[arg(long, value_name = "REMOTE_AUTHORITY")]
    pub to: String,
}

/// Deploy binding trust from one authority to another.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsCertDeployArgs {
    /// Destination remote authority.
    #[arg(long, value_name = "REMOTE_AUTHORITY")]
    pub to: String,

    /// Source remote authority.
    #[arg(long, value_name = "REMOTE_AUTHORITY")]
    pub from: String,
}

/// Generate a local client key for imago-cli authentication.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct CertsGenerateArgs {
    /// Output directory for generated key files.
    #[arg(long, value_name = "PATH", default_value = "certs")]
    pub out_dir: PathBuf,

    /// Overwrite existing files in the output directory.
    #[arg(long)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn parses_project_init_with_path_and_lang() {
        let cli =
            Cli::try_parse_from(["imago", "project", "init", "services/api", "--lang", "rust"])
                .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Project(ProjectSubcommandArgs {
                    command: ProjectCommands::Init(InitArgs {
                        path: Some(PathBuf::from("services/api")),
                        lang: Some("rust".to_string()),
                    }),
                }),
            }
        );
    }

    #[test]
    fn parses_artifact_build_with_target() {
        let cli = Cli::try_parse_from(["imago", "artifact", "build", "--target", "edge"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Artifact(ArtifactSubcommandArgs {
                    command: ArtifactCommands::Build(BuildArgs {
                        target: "edge".to_string(),
                    }),
                }),
            }
        );
    }

    #[test]
    fn parses_deps_sync_without_options() {
        let cli = Cli::try_parse_from(["imago", "deps", "sync"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Deps(DepsSubcommandArgs {
                    command: DepsCommands::Sync(UpdateArgs {}),
                }),
            }
        );
    }

    #[test]
    fn parses_service_commands() {
        let deploy =
            Cli::try_parse_from(["imago", "service", "deploy", "--target", "default", "-d"])
                .expect("parse should succeed");
        assert_eq!(
            deploy,
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Deploy(DeployArgs {
                        target: Some("default".to_string()),
                        detach: true,
                    }),
                }),
            }
        );

        let start = Cli::try_parse_from(["imago", "service", "start", "svc-a", "--target", "edge"])
            .expect("parse should succeed");
        assert_eq!(
            start,
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Start(RunArgs {
                        name: Some("svc-a".to_string()),
                        target: Some("edge".to_string()),
                        detach: false,
                    }),
                }),
            }
        );

        let ls = Cli::try_parse_from(["imago", "service", "ls"]).expect("parse should succeed");
        assert_eq!(
            ls,
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Ls(PsArgs {
                        target: "default".to_string(),
                    }),
                }),
            }
        );

        let logs = Cli::try_parse_from([
            "imago", "service", "logs", "svc-a", "--follow", "--tail", "50",
        ])
        .expect("parse should succeed");
        assert_eq!(
            logs,
            Cli {
                command: Commands::Service(ServiceSubcommandArgs {
                    command: ServiceCommands::Logs(LogsArgs {
                        name: Some("svc-a".to_string()),
                        follow: true,
                        tail: 50,
                        with_timestamp: false,
                    }),
                }),
            }
        );
    }

    #[test]
    fn parses_stack_commands_with_new_names() {
        let sync = Cli::try_parse_from(["imago", "stack", "sync", "nanokvm-mini"])
            .expect("parse should succeed");
        assert_eq!(
            sync,
            Cli {
                command: Commands::Stack(ComposeSubcommandArgs {
                    command: ComposeCommands::Sync(ComposeUpdateArgs {
                        profile: "nanokvm-mini".to_string(),
                    }),
                }),
            }
        );

        let ls = Cli::try_parse_from(["imago", "stack", "ls", "nanokvm-mini", "--target", "edge"])
            .expect("parse should succeed");
        assert_eq!(
            ls,
            Cli {
                command: Commands::Stack(ComposeSubcommandArgs {
                    command: ComposeCommands::Ls(ComposePsArgs {
                        profile: "nanokvm-mini".to_string(),
                        target: "edge".to_string(),
                    }),
                }),
            }
        );
    }

    #[test]
    fn parses_trust_cert_and_client_key_commands() {
        let upload = Cli::try_parse_from([
            "imago",
            "trust",
            "cert",
            "upload",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--to",
            "rpc://node-a:4443",
        ])
        .expect("parse should succeed");
        assert_eq!(
            upload,
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
            }
        );

        let replicate = Cli::try_parse_from([
            "imago",
            "trust",
            "cert",
            "replicate",
            "--to",
            "rpc://node-a:4443",
            "--from",
            "rpc://node-b:4443",
        ])
        .expect("parse should succeed");
        assert_eq!(
            replicate,
            Cli {
                command: Commands::Trust(TrustSubcommandArgs {
                    command: TrustCommands::Cert(TrustCertSubcommandArgs {
                        command: TrustCertCommands::Replicate(BindingsCertDeployArgs {
                            to: "rpc://node-a:4443".to_string(),
                            from: "rpc://node-b:4443".to_string(),
                        }),
                    }),
                }),
            }
        );

        let generate = Cli::try_parse_from(["imago", "trust", "client-key", "generate", "--force"])
            .expect("parse should succeed");
        assert_eq!(
            generate,
            Cli {
                command: Commands::Trust(TrustSubcommandArgs {
                    command: TrustCommands::ClientKey(TrustClientKeySubcommandArgs {
                        command: TrustClientKeyCommands::Generate(CertsGenerateArgs {
                            out_dir: PathBuf::from("certs"),
                            force: true,
                        }),
                    }),
                }),
            }
        );
    }

    #[test]
    fn rejects_v1_command_names() {
        let err = Cli::try_parse_from(["imago", "build"]).expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);

        let err =
            Cli::try_parse_from(["imago", "compose", "deploy"]).expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn root_help_includes_v2_command_descriptions() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains("Project lifecycle operations"));
        assert!(help.contains("Artifact lifecycle operations"));
        assert!(help.contains("Service runtime operations"));
        assert!(help.contains("Stack profile operations across multiple services"));
        assert!(help.contains("Trust and certificate operations"));
    }

    #[test]
    fn service_logs_help_includes_follow_tail_timestamp() {
        let err = Cli::try_parse_from(["imago", "service", "logs", "--help"])
            .expect_err("help should exit");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();

        assert!(help.contains("-f, --follow"));
        assert!(help.contains("--tail <N>"));
        assert!(help.contains("--with-timestamp"));
    }

    #[test]
    fn stack_logs_help_includes_target_help_text() {
        let err = Cli::try_parse_from(["imago", "stack", "logs", "--help"])
            .expect_err("help should exit");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();

        assert!(help.contains("--target <TARGET_NAME>"));
        assert!(help.contains("Target name used for all services in this profile"));
    }
}
