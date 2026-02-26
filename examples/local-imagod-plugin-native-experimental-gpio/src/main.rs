#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    generate_all
});

#[cfg(target_arch = "wasm32")]
fn main() {
    imago::experimental_gpio::delay::delay_ms(5);
    println!("experimental-gpio delay-ms completed: 5");

    if std::env::var("IMAGO_EXPERIMENTAL_GPIO_TRY_DIGITAL").as_deref() == Ok("1") {
        match imago::experimental_gpio::digital::get_digital_out("GPIO17", &[]) {
            Ok(pin) => {
                let _ = pin.set_active();
                let _ = pin.set_inactive();
                println!("experimental-gpio digital smoke test completed");
            }
            Err(err) => {
                println!("experimental-gpio digital smoke test failed: {err:?}");
            }
        }
    }

    std::thread::sleep(std::time::Duration::from_secs(1));
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
