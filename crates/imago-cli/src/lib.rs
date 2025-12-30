mod dev;
mod run;

use crate::dev::DevSubCommand;
use crate::run::Runnable;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
pub struct Cli {
    #[clap(subcommand)]
    command: SubCommand,
    prefix: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum SubCommand {
    Dev(dev::DevCommands),
    /// Run a service defined in the configuration
    Run,
    /// Stop a running service
    Stop,
    /// View logs from a running service
    Logs,
    /// List running services
    Ps,
}

impl Cli {
    pub async fn run() -> anyhow::Result<()> {
        let cmd = Self::parse();

        let path = match cmd.prefix {
            None => std::env::current_dir()?,
            Some(ref path) => {
                if path.is_absolute() {
                    path.clone()
                } else {
                    std::env::current_dir()?.join(path)
                }
            }
        };

        match cmd.command {
            SubCommand::Dev(ref dev) => match dev.command {
                DevSubCommand::Build(ref cmd) => cmd.run(path).await?,
                DevSubCommand::Update => {}
            },
            _ => todo!(),
        }
        Ok(())
    }
}
