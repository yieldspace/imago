#[tokio::main(flavor = "multi_thread")]
async fn main() {
    use clap::Parser;
    use imago_cli::{ParsedCli, dispatch};

    let cli = ParsedCli::parse();
    let result = dispatch(cli).await;

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }
}
