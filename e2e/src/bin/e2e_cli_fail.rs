use std::io::Write as _;

fn main() {
    println!("IMAGO_E2E_DEPLOY_FAIL_STDOUT");
    eprintln!("IMAGO_E2E_DEPLOY_FAIL_STDERR");
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    panic!("IMAGO_E2E_DEPLOY_FAIL_PANIC");
}
