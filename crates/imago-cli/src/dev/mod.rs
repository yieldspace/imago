use crate::run::Runnable;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug, Clone)]
pub struct DevCommands {
    #[clap(subcommand)]
    command: DevSubCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum DevSubCommand {
    Build,
}

impl Runnable for DevCommands {
    async fn run(&self, prefix: PathBuf) -> anyhow::Result<()> {
        self.command.run(prefix).await
    }
}

impl Runnable for DevSubCommand {
    async fn run(&self, prefix: PathBuf) -> anyhow::Result<()> {
        match self {
            DevSubCommand::Build => {
                println!("Building development environment at {:?}", prefix);
                // Implement build logic here
            }
        }
        Ok(())
    }
}
