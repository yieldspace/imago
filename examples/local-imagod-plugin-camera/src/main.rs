#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    generate_all
});

#[cfg(target_arch = "wasm32")]
fn main() {
    println!("camera example started");

    let cameras = match imago::camera::provider::list_cameras() {
        Ok(cameras) => cameras,
        Err(err) => {
            eprintln!("camera example: list-cameras failed: {err:?}");
            return;
        }
    };

    println!("camera example: discovered {} camera(s)", cameras.len());
    let camera = match cameras.into_iter().next() {
        Some(camera) => camera,
        None => {
            println!("camera example: no cameras discovered");
            return;
        }
    };

    println!(
        "camera example: selected camera id={} label={} vendor={:04x} product={:04x} bus={} address={}",
        camera.id, camera.label, camera.vendor_id, camera.product_id, camera.bus, camera.address
    );

    let modes = match imago::camera::provider::list_modes(&camera.id) {
        Ok(modes) => modes,
        Err(err) => {
            eprintln!(
                "camera example: list-modes failed for {}: {err:?}",
                camera.id
            );
            return;
        }
    };

    let mode = match modes.into_iter().next() {
        Some(mode) => mode,
        None => {
            println!(
                "camera example: no capture mode was discovered for {}",
                camera.id
            );
            return;
        }
    };

    println!(
        "camera example: selected mode format={} {}x{} fps={}/{}",
        encoded_format_name(mode.format),
        mode.width_px,
        mode.height_px,
        mode.fps_num,
        mode.fps_den
    );

    let session = match imago::camera::provider::open_session(&camera.id, mode.clone()) {
        Ok(session) => session,
        Err(err) => {
            eprintln!(
                "camera example: open-session failed for {}: {err:?}",
                camera.id
            );
            return;
        }
    };

    let negotiated = session.current_mode();
    println!(
        "camera example: session opened with format={} {}x{} fps={}/{}",
        encoded_format_name(negotiated.format),
        negotiated.width_px,
        negotiated.height_px,
        negotiated.fps_num,
        negotiated.fps_den
    );

    print_frame("camera still frame", &session, 5_000);
    for index in 0..3 {
        let label = format!("camera stream frame[{index}]");
        print_frame(&label, &session, 5_000);
    }

    session.close();
    println!("camera example finished");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(target_arch = "wasm32")]
fn print_frame(label: &str, session: &imago::camera::provider::Session, timeout_ms: u32) {
    match session.next_frame(timeout_ms) {
        Ok(frame) => {
            println!(
                "{label}: seq={} bytes={} jpeg={} format={} {}x{} timestamp_ns={}",
                frame.sequence,
                frame.bytes.len(),
                is_jpeg(&frame.bytes),
                encoded_format_name(frame.format),
                frame.width_px,
                frame.height_px,
                frame.timestamp_ns
            );
        }
        Err(err) => {
            eprintln!("{label}: next_frame failed: {err:?}");
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn encoded_format_name(format: imago::camera::types::EncodedFormat) -> &'static str {
    match format {
        imago::camera::types::EncodedFormat::Mjpeg => "mjpeg",
    }
}

#[cfg(target_arch = "wasm32")]
fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9])
}
