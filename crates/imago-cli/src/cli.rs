use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Build, update, and operate imago services.
#[derive(Debug, Parser, PartialEq, Eq)]
#[command(name = "imago", version, about = "imago CLI")]
pub struct Cli {
    /// Emit JSON Lines output instead of rich/plain terminal UI.
    #[arg(long, global = true)]
    pub json: bool,

    /// Command to execute.
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Commands {
    /// Build project artifacts and manifest.
    Build(BuildArgs),
    /// Resolve dependencies and refresh lock/cache state.
    Update(UpdateArgs),
    /// Build and deploy the current service to imagod.
    Deploy(DeployArgs),
    /// Run compose profile operations across multiple services.
    Compose(ComposeSubcommandArgs),
    /// Start a deployed service instance.
    Run(RunArgs),
    /// Stop a running service instance.
    Stop(StopArgs),
    /// List deployed service states.
    Ps(PsArgs),
    /// Stream or tail service logs.
    Logs(LogsArgs),
    /// Manage binding certificates and trust data.
    Bindings(BindingsSubcommandArgs),
    /// Generate local development certificates.
    Certs(CertsSubcommandArgs),
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
    /// Update dependencies for all services in a compose profile.
    Update(ComposeUpdateArgs),
    /// Deploy all services in a compose profile.
    Deploy(ComposeDeployArgs),
    /// Stream or tail logs for services in a compose profile.
    Logs(ComposeLogsArgs),
    /// List deployed service states in a compose profile.
    Ps(ComposePsArgs),
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

/// Update dependencies for services in a compose profile.
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
    #[arg(long)]
    pub follow: bool,

    /// Number of recent log lines to fetch before streaming.
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub tail: u32,
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
    #[arg(long)]
    pub follow: bool,

    /// Number of recent log lines to fetch before streaming.
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub tail: u32,
}

/// Bindings subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsSubcommandArgs {
    /// Bindings operation to run.
    #[command(subcommand)]
    pub command: BindingsCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum BindingsCommands {
    /// Manage binding certificate operations.
    Cert(BindingsCertSubcommandArgs),
}

/// Binding certificate subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsCertSubcommandArgs {
    /// Binding certificate operation to run.
    #[command(subcommand)]
    pub command: BindingsCertCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum BindingsCertCommands {
    /// Upload a public key to a remote authority.
    Upload(BindingsCertUploadArgs),
    /// Copy a binding certificate from one authority to another.
    Deploy(BindingsCertDeployArgs),
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

/// Certificate utility subcommands.
#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct CertsSubcommandArgs {
    /// Certificate operation to run.
    #[command(subcommand)]
    pub command: CertsCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum CertsCommands {
    /// Generate a local client key for imago-cli authentication.
    Generate(CertsGenerateArgs),
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
    fn parses_build_without_options() {
        let cli = Cli::try_parse_from(["imago", "build"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Build(BuildArgs {
                    target: "default".to_string(),
                }),
            }
        );
    }

    #[test]
    fn parses_build_with_target() {
        let cli = Cli::try_parse_from(["imago", "build", "--target", "edge"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Build(BuildArgs {
                    target: "edge".to_string(),
                }),
            }
        );
    }

    #[test]
    fn rejects_build_env_option() {
        let err = Cli::try_parse_from(["imago", "build", "--env", "prod"])
            .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_update_without_options() {
        let cli = Cli::try_parse_from(["imago", "update"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Update(UpdateArgs {}),
            }
        );
    }

    #[test]
    fn parses_deploy_without_options() {
        let cli = Cli::try_parse_from(["imago", "deploy"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Deploy(DeployArgs { target: None }),
            }
        );
    }

    #[test]
    fn rejects_deploy_env_option() {
        let err = Cli::try_parse_from(["imago", "deploy", "--env", "prod"])
            .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_deploy_with_target() {
        let cli = Cli::try_parse_from(["imago", "deploy", "--target", "default"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Deploy(DeployArgs {
                    target: Some("default".to_string()),
                }),
            }
        );
    }

    #[test]
    fn parses_compose_build_with_profile_and_target() {
        let cli = Cli::try_parse_from([
            "imago",
            "compose",
            "build",
            "nanokvm-mini",
            "--target",
            "nanokvm-cube",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Build(ComposeBuildArgs {
                        profile: "nanokvm-mini".to_string(),
                        target: "nanokvm-cube".to_string(),
                    }),
                }),
            }
        );
    }

    #[test]
    fn compose_build_requires_target() {
        let err = Cli::try_parse_from(["imago", "compose", "build", "nanokvm-mini"])
            .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parses_compose_update_with_profile() {
        let cli = Cli::try_parse_from(["imago", "compose", "update", "nanokvm-mini"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Update(ComposeUpdateArgs {
                        profile: "nanokvm-mini".to_string(),
                    }),
                }),
            }
        );
    }

    #[test]
    fn parses_compose_deploy_with_profile_and_target() {
        let cli = Cli::try_parse_from([
            "imago",
            "compose",
            "deploy",
            "nanokvm-mini",
            "--target",
            "nanokvm-cube",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Deploy(ComposeDeployArgs {
                        profile: "nanokvm-mini".to_string(),
                        target: "nanokvm-cube".to_string(),
                    }),
                }),
            }
        );
    }

    #[test]
    fn compose_deploy_requires_target() {
        let err = Cli::try_parse_from(["imago", "compose", "deploy", "nanokvm-mini"])
            .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parses_compose_logs_with_profile_target_and_flags() {
        let cli = Cli::try_parse_from([
            "imago",
            "compose",
            "logs",
            "nanokvm-mini",
            "--target",
            "nanokvm-cube",
            "--name",
            "svc-a",
            "--follow",
            "--tail",
            "50",
            "--json",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: true,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Logs(ComposeLogsArgs {
                        profile: "nanokvm-mini".to_string(),
                        target: "nanokvm-cube".to_string(),
                        name: Some("svc-a".to_string()),
                        follow: true,
                        tail: 50,
                    }),
                }),
            }
        );
    }

    #[test]
    fn compose_logs_requires_target() {
        let err = Cli::try_parse_from(["imago", "compose", "logs", "nanokvm-mini"])
            .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parses_compose_ps_with_profile_and_target() {
        let cli = Cli::try_parse_from([
            "imago",
            "compose",
            "ps",
            "nanokvm-mini",
            "--target",
            "nanokvm-cube",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Compose(ComposeSubcommandArgs {
                    command: ComposeCommands::Ps(ComposePsArgs {
                        profile: "nanokvm-mini".to_string(),
                        target: "nanokvm-cube".to_string(),
                    }),
                }),
            }
        );
    }

    #[test]
    fn compose_ps_requires_target() {
        let err = Cli::try_parse_from(["imago", "compose", "ps", "nanokvm-mini"])
            .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parses_logs_with_defaults() {
        let cli = Cli::try_parse_from(["imago", "logs"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Logs(LogsArgs {
                    name: None,
                    follow: false,
                    tail: 200,
                }),
            }
        );
    }

    #[test]
    fn parses_run_with_defaults() {
        let cli = Cli::try_parse_from(["imago", "run"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Run(RunArgs {
                    name: None,
                    target: None,
                }),
            }
        );
    }

    #[test]
    fn parses_run_with_name_target() {
        let cli = Cli::try_parse_from(["imago", "run", "svc-a", "--target", "edge"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Run(RunArgs {
                    name: Some("svc-a".to_string()),
                    target: Some("edge".to_string()),
                }),
            }
        );
    }

    #[test]
    fn rejects_run_env_option() {
        let err =
            Cli::try_parse_from(["imago", "run", "--env", "prod"]).expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_stop_with_defaults() {
        let cli = Cli::try_parse_from(["imago", "stop"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Stop(StopArgs {
                    name: None,
                    force: false,
                    target: None,
                }),
            }
        );
    }

    #[test]
    fn parses_stop_with_name_force_target() {
        let cli = Cli::try_parse_from(["imago", "stop", "svc-a", "--force", "--target", "edge"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Stop(StopArgs {
                    name: Some("svc-a".to_string()),
                    force: true,
                    target: Some("edge".to_string()),
                }),
            }
        );
    }

    #[test]
    fn rejects_stop_env_option() {
        let err =
            Cli::try_parse_from(["imago", "stop", "--env", "prod"]).expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_ps_with_default_target() {
        let cli = Cli::try_parse_from(["imago", "ps"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Ps(PsArgs {
                    target: "default".to_string(),
                }),
            }
        );
    }

    #[test]
    fn parses_ps_with_target() {
        let cli =
            Cli::try_parse_from(["imago", "ps", "--target", "edge"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Ps(PsArgs {
                    target: "edge".to_string(),
                }),
            }
        );
    }

    #[test]
    fn parses_logs_with_name_and_flags() {
        let cli = Cli::try_parse_from(["imago", "logs", "svc-a", "--follow", "--tail", "50"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Logs(LogsArgs {
                    name: Some("svc-a".to_string()),
                    follow: true,
                    tail: 50,
                }),
            }
        );
    }

    #[test]
    fn parses_logs_with_json_flag() {
        let cli = Cli::try_parse_from(["imago", "logs", "svc-a", "--tail", "10", "--json"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: true,
                command: Commands::Logs(LogsArgs {
                    name: Some("svc-a".to_string()),
                    follow: false,
                    tail: 10,
                }),
            }
        );
    }

    #[test]
    fn rejects_unknown_subcommand() {
        let err = Cli::try_parse_from(["imago", "unknown"]).expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn parses_bindings_cert_upload() {
        let cli = Cli::try_parse_from([
            "imago",
            "bindings",
            "cert",
            "upload",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--to",
            "rpc://node-a:4443",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
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
            }
        );
    }

    #[test]
    fn parses_bindings_cert_deploy() {
        let cli = Cli::try_parse_from([
            "imago",
            "bindings",
            "cert",
            "deploy",
            "--to",
            "rpc://node-a:4443",
            "--from",
            "rpc://node-b:4443",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
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
            }
        );
    }

    #[test]
    fn parses_certs_generate_with_defaults() {
        let cli =
            Cli::try_parse_from(["imago", "certs", "generate"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Certs(CertsSubcommandArgs {
                    command: CertsCommands::Generate(CertsGenerateArgs {
                        out_dir: PathBuf::from("certs"),
                        force: false,
                    }),
                }),
            }
        );
    }

    #[test]
    fn parses_certs_generate_with_overrides() {
        let cli = Cli::try_parse_from([
            "imago",
            "certs",
            "generate",
            "--out-dir",
            "tmp-certs",
            "--force",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                json: false,
                command: Commands::Certs(CertsSubcommandArgs {
                    command: CertsCommands::Generate(CertsGenerateArgs {
                        out_dir: PathBuf::from("tmp-certs"),
                        force: true,
                    }),
                }),
            }
        );
    }

    #[test]
    fn rejects_certs_generate_server_name_option() {
        let err = Cli::try_parse_from([
            "imago",
            "certs",
            "generate",
            "--server-name",
            "imagod.local",
        ])
        .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn rejects_certs_generate_server_ip_option() {
        let err =
            Cli::try_parse_from(["imago", "certs", "generate", "--server-ip", "192.168.10.2"])
                .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn rejects_certs_generate_days_option() {
        let err = Cli::try_parse_from(["imago", "certs", "generate", "--days", "30"])
            .expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn root_help_includes_non_empty_command_descriptions() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains("Build project artifacts and manifest"));
        assert!(help.contains("Build and deploy the current service to imagod"));
        assert!(help.contains("Run compose profile operations across multiple services"));
    }

    #[test]
    fn deploy_help_includes_target_help_text() {
        let err = Cli::try_parse_from(["imago", "deploy", "--help"]).expect_err("help should exit");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();

        assert!(help.contains("--target <TARGET_NAME>"));
        assert!(help.contains("Target name defined in imago.toml [target.<name>]"));
    }

    #[test]
    fn compose_logs_help_includes_follow_and_tail_help_text() {
        let err = Cli::try_parse_from(["imago", "compose", "logs", "--help"])
            .expect_err("help should exit");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();

        assert!(help.contains("--follow"));
        assert!(help.contains("Keep streaming logs until interrupted"));
        assert!(help.contains("--tail <N>"));
        assert!(help.contains("Number of recent log lines to fetch before streaming"));
        assert!(help.contains("--target <TARGET_NAME>"));
        assert!(help.contains("Target name used for all services in this profile"));
    }

    #[test]
    fn bindings_cert_deploy_help_includes_from_to_help_text() {
        let err = Cli::try_parse_from(["imago", "bindings", "cert", "deploy", "--help"])
            .expect_err("help should exit");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();

        assert!(help.contains("--to <REMOTE_AUTHORITY>"));
        assert!(help.contains("Destination remote authority"));
        assert!(help.contains("--from <REMOTE_AUTHORITY>"));
        assert!(help.contains("Source remote authority"));
    }
}
