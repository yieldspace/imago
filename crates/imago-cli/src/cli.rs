use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(name = "imago", version, about = "imago CLI")]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Commands {
    Build(BuildArgs),
    Update(UpdateArgs),
    Deploy(DeployArgs),
    Compose(ComposeSubcommandArgs),
    Run(RunArgs),
    Stop(StopArgs),
    Logs(LogsArgs),
    Bindings(BindingsSubcommandArgs),
    Certs(CertsSubcommandArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BuildArgs {
    #[arg(long, value_name = "TARGET_NAME", default_value = "default")]
    pub target: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct UpdateArgs {}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct DeployArgs {
    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct RunArgs {
    #[arg(value_name = "SERVICE_NAME")]
    pub name: Option<String>,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct StopArgs {
    #[arg(value_name = "SERVICE_NAME")]
    pub name: Option<String>,

    #[arg(long)]
    pub force: bool,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeSubcommandArgs {
    #[command(subcommand)]
    pub command: ComposeCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum ComposeCommands {
    Build(ComposeBuildArgs),
    Update(ComposeUpdateArgs),
    Deploy(ComposeDeployArgs),
    Logs(ComposeLogsArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeBuildArgs {
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeUpdateArgs {
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeDeployArgs {
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ComposeLogsArgs {
    #[arg(value_name = "PROFILE_NAME")]
    pub profile: String,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: String,

    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long)]
    pub follow: bool,

    #[arg(long, value_name = "N", default_value_t = 200)]
    pub tail: u32,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct LogsArgs {
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long)]
    pub follow: bool,

    #[arg(long, value_name = "N", default_value_t = 200)]
    pub tail: u32,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsSubcommandArgs {
    #[command(subcommand)]
    pub command: BindingsCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum BindingsCommands {
    Cert(BindingsCertSubcommandArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsCertSubcommandArgs {
    #[command(subcommand)]
    pub command: BindingsCertCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum BindingsCertCommands {
    Upload(BindingsCertUploadArgs),
    Deploy(BindingsCertDeployArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsCertUploadArgs {
    #[arg(value_name = "PUBLIC_KEY_HEX")]
    pub public_key: String,

    #[arg(long, value_name = "REMOTE_AUTHORITY")]
    pub to: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BindingsCertDeployArgs {
    #[arg(long, value_name = "REMOTE_AUTHORITY")]
    pub to: String,

    #[arg(long, value_name = "REMOTE_AUTHORITY")]
    pub from: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct CertsSubcommandArgs {
    #[command(subcommand)]
    pub command: CertsCommands,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum CertsCommands {
    Generate(CertsGenerateArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct CertsGenerateArgs {
    #[arg(long, value_name = "PATH", default_value = "certs")]
    pub out_dir: PathBuf,

    #[arg(long, value_name = "DNS_NAME", default_value = "localhost")]
    pub server_name: String,

    #[arg(long, value_name = "IP_ADDR", default_value = "127.0.0.1")]
    pub server_ip: String,

    #[arg(long, value_name = "DAYS", default_value_t = 3650)]
    pub days: u32,

    #[arg(long)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
                        server_name: "localhost".to_string(),
                        server_ip: "127.0.0.1".to_string(),
                        days: 3650,
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
            "--server-name",
            "imagod.local",
            "--server-ip",
            "192.168.10.2",
            "--days",
            "30",
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
                        server_name: "imagod.local".to_string(),
                        server_ip: "192.168.10.2".to_string(),
                        days: 30,
                        force: true,
                    }),
                }),
            }
        );
    }
}
