#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    generate_all
});

#[cfg(target_arch = "wasm32")]
fn main() {
    use imago::experimental_gpio::digital;

    println!("experimental-gpio example started");

    match std::env::var("IMAGO_EXPERIMENTAL_GPIO_LABEL") {
        Ok(label) => match digital::get_digital_out(&label, &[]) {
            Ok(pin) => {
                let _ = pin.set_active();
                let _ = pin.set_inactive();
                println!("raw gpio smoke test completed for label: {label}");
            }
            Err(err) => {
                println!("raw gpio smoke test failed for label {label}: {err:?}");
            }
        },
        Err(_) => {
            println!("set IMAGO_EXPERIMENTAL_GPIO_LABEL to run a digital smoke test");
        }
    }

    std::thread::sleep(std::time::Duration::from_secs(1));
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
