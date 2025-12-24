mod dev;
mod run;

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
            SubCommand::Dev(ref dev) => dev.run(path).await?,
        }
        Ok(())
    }
}
