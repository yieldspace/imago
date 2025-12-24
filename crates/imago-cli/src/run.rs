use std::path::PathBuf;

pub trait Runnable {
    async fn run(&self, prefix: PathBuf) -> anyhow::Result<()>;
}
