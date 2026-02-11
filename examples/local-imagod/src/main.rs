fn main() {
    let mode = std::env::var("IMAGO_EXAMPLE").unwrap_or_else(|_| "unset".to_string());
    loop {
        println!("local-imagod-app started (IMAGO_EXAMPLE={mode})");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

