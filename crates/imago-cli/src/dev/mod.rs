mod build;

use crate::dev::build::BuildCommand;
use clap::{Args, Subcommand};

#[derive(Args, Debug, Clone)]
pub struct DevCommands {
    #[clap(subcommand)]
    pub command: DevSubCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum DevSubCommand {
    Build(BuildCommand),
    Update,
}
