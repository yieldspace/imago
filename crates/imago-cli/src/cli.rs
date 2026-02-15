use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(name = "imago", version, about = "imago CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Commands {
    Build(BuildArgs),
    Deploy(DeployArgs),
    Run(RunArgs),
    Stop(StopArgs),
    Logs(LogsArgs),
    Certs(CertsSubcommandArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct BuildArgs {
    #[arg(long, value_name = "ENV_NAME")]
    pub env: Option<String>,

    #[arg(long, value_name = "TARGET_NAME", default_value = "default")]
    pub target: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct DeployArgs {
    #[arg(long, value_name = "ENV_NAME")]
    pub env: Option<String>,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct RunArgs {
    #[arg(value_name = "SERVICE_NAME")]
    pub name: Option<String>,

    #[arg(long, value_name = "ENV_NAME")]
    pub env: Option<String>,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct StopArgs {
    #[arg(value_name = "SERVICE_NAME")]
    pub name: Option<String>,

    #[arg(long)]
    pub force: bool,

    #[arg(long, value_name = "ENV_NAME")]
    pub env: Option<String>,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
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
                command: Commands::Build(BuildArgs {
                    env: None,
                    target: "default".to_string(),
                }),
            }
        );
    }

    #[test]
    fn parses_build_with_env_and_target() {
        let cli = Cli::try_parse_from(["imago", "build", "--env", "prod", "--target", "edge"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Build(BuildArgs {
                    env: Some("prod".to_string()),
                    target: "edge".to_string(),
                }),
            }
        );
    }

    #[test]
    fn parses_deploy_without_options() {
        let cli = Cli::try_parse_from(["imago", "deploy"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Deploy(DeployArgs {
                    env: None,
                    target: None,
                }),
            }
        );
    }

    #[test]
    fn parses_deploy_with_env() {
        let cli = Cli::try_parse_from(["imago", "deploy", "--env", "prod"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Deploy(DeployArgs {
                    env: Some("prod".to_string()),
                    target: None,
                }),
            }
        );
    }

    #[test]
    fn parses_deploy_with_target() {
        let cli = Cli::try_parse_from(["imago", "deploy", "--target", "default"])
            .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Deploy(DeployArgs {
                    env: None,
                    target: Some("default".to_string()),
                }),
            }
        );
    }

    #[test]
    fn parses_logs_with_defaults() {
        let cli = Cli::try_parse_from(["imago", "logs"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
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
                command: Commands::Run(RunArgs {
                    name: None,
                    env: None,
                    target: None,
                }),
            }
        );
    }

    #[test]
    fn parses_run_with_name_env_target() {
        let cli =
            Cli::try_parse_from(["imago", "run", "svc-a", "--env", "prod", "--target", "edge"])
                .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Run(RunArgs {
                    name: Some("svc-a".to_string()),
                    env: Some("prod".to_string()),
                    target: Some("edge".to_string()),
                }),
            }
        );
    }

    #[test]
    fn parses_stop_with_defaults() {
        let cli = Cli::try_parse_from(["imago", "stop"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Stop(StopArgs {
                    name: None,
                    force: false,
                    env: None,
                    target: None,
                }),
            }
        );
    }

    #[test]
    fn parses_stop_with_name_force_env_target() {
        let cli = Cli::try_parse_from([
            "imago", "stop", "svc-a", "--force", "--env", "prod", "--target", "edge",
        ])
        .expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
                command: Commands::Stop(StopArgs {
                    name: Some("svc-a".to_string()),
                    force: true,
                    env: Some("prod".to_string()),
                    target: Some("edge".to_string()),
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
                command: Commands::Logs(LogsArgs {
                    name: Some("svc-a".to_string()),
                    follow: true,
                    tail: 50,
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
    fn parses_certs_generate_with_defaults() {
        let cli =
            Cli::try_parse_from(["imago", "certs", "generate"]).expect("parse should succeed");

        assert_eq!(
            cli,
            Cli {
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
