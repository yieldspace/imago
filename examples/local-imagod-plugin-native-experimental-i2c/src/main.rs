#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    generate_all
});

#[cfg(target_arch = "wasm32")]
fn main() {
    let delay = imago::experimental_i2c::provider::open_delay();
    delay.delay_ns(5_000_000);
    println!("experimental-i2c delay-ns completed: 5000000");

    if std::env::var("IMAGO_EXPERIMENTAL_I2C_TRY_OPEN_DEFAULT").as_deref() == Ok("1") {
        match imago::experimental_i2c::provider::open_default_i2c() {
            Ok(i2c_bus) => match i2c_bus.read(0x00, 0) {
                Ok(_) => println!("experimental-i2c open-default-i2c succeeded"),
                Err(err) => println!("experimental-i2c test read failed: {err:?}"),
            },
            Err(err) => {
                println!("experimental-i2c open-default-i2c failed: {err}");
            }
        }
    }

    std::thread::sleep(std::time::Duration::from_secs(1));
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
