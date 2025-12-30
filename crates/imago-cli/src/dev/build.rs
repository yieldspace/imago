use crate::run::Runnable;
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug, Clone)]
pub struct BuildCommand {}

impl Runnable for BuildCommand {
    async fn run(&self, prefix: PathBuf) -> anyhow::Result<()> {
        Ok(())
    }
}
