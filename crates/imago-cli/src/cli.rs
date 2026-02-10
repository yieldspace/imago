use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(name = "imago", version, about = "imago CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Commands {
    Deploy(DeployArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct DeployArgs {
    #[arg(long, value_name = "ENV_NAME")]
    pub env: Option<String>,

    #[arg(long, value_name = "TARGET_NAME")]
    pub target: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
    fn rejects_unknown_subcommand() {
        let err = Cli::try_parse_from(["imago", "unknown"]).expect_err("parse should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }
}
