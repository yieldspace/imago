fn main() {
    let mode = std::env::var("IMAGO_EXAMPLE").unwrap_or_else(|_| "unset".to_string());
    println!("local-imagod-app started (IMAGO_EXAMPLE={mode})");
}

