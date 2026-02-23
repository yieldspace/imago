use wit_bindgen::generate;

generate!({
    world: "nanokvm-probe",
    generate_all
});

fn main() {
    println!("nanokvm-probe: start");

    match imago::nanokvm::stream_config::get_server_config_yaml() {
        Ok(yaml) => println!("nanokvm-probe: stream-config ok bytes={}", yaml.len()),
        Err(err) => println!("nanokvm-probe: stream-config err={err}"),
    }

    match imago::nanokvm::device_status::get_usb_mode() {
        Ok(mode) => {
            let mode_text = match mode {
                imago::nanokvm::device_status::UsbMode::Normal => "normal",
                imago::nanokvm::device_status::UsbMode::HidOnly => "hid-only",
            };
            println!("nanokvm-probe: device-status ok usb-mode={mode_text}");
        }
        Err(err) => println!("nanokvm-probe: device-status err={err}"),
    }

    println!("nanokvm-probe: completed");
}
