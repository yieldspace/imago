fn main() {
    println!("Hello, world!");
    print_env("IMAGO_E2E_ENV_OVERRIDE");
    print_env("IMAGO_E2E_ENV_ONLY");
    print_env("IMAGO_E2E_ENV_TOML_ONLY");
}

fn print_env(key: &str) {
    let value = std::env::var(key).unwrap_or_else(|_| "<unset>".to_string());
    println!("{key}={value}");
}
