#[tokio::main]
async fn main() {
    if let Err(err) = imagod::dispatch_from_env().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
