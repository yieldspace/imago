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
        "camera example: selected camera index={} id={} label={} vendor={:04x} product={:04x} bus={} address={}",
        camera.index,
        camera.id,
        camera.label,
        camera.vendor_id,
        camera.product_id,
        camera.bus,
        camera.address
    );

    let capture = match imago::camera::provider::open(camera.index) {
        Ok(capture) => capture,
        Err(err) => {
            eprintln!("camera example: open failed for {}: {err:?}", camera.index);
            return;
        }
    };

    println!("camera example: is_opened={}", capture.is_opened());
    echo_property(
        "frame-width",
        &capture,
        imago::camera::types::CaptureProperty::FrameWidth,
    );
    echo_property(
        "frame-height",
        &capture,
        imago::camera::types::CaptureProperty::FrameHeight,
    );
    echo_property("fps", &capture, imago::camera::types::CaptureProperty::Fps);
    echo_property(
        "fourcc",
        &capture,
        imago::camera::types::CaptureProperty::Fourcc,
    );

    refresh_property(&capture, imago::camera::types::CaptureProperty::FrameWidth);
    refresh_property(&capture, imago::camera::types::CaptureProperty::FrameHeight);
    refresh_property(&capture, imago::camera::types::CaptureProperty::Fps);

    print_frame("camera read frame", capture.read(5_000));
    match capture.grab(5_000) {
        Ok(true) => print_frame("camera retrieve frame", capture.retrieve()),
        Ok(false) => println!("camera example: grab returned false"),
        Err(err) => eprintln!("camera example: grab failed: {err:?}"),
    }

    capture.release();
    println!(
        "camera example: is_opened after release={}",
        capture.is_opened()
    );
    println!("camera example finished");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(target_arch = "wasm32")]
fn echo_property(
    label: &str,
    capture: &imago::camera::provider::VideoCapture,
    property: imago::camera::types::CaptureProperty,
) {
    match capture.get(property) {
        Ok(value) => println!("camera example: {label}={value}"),
        Err(err) => eprintln!("camera example: get({label}) failed: {err:?}"),
    }
}

#[cfg(target_arch = "wasm32")]
fn refresh_property(
    capture: &imago::camera::provider::VideoCapture,
    property: imago::camera::types::CaptureProperty,
) {
    let Ok(value) = capture.get(property) else {
        return;
    };
    match capture.set(property, value) {
        Ok(applied) => println!("camera example: set({property:?}) -> {applied}"),
        Err(err) => eprintln!("camera example: set({property:?}) failed: {err:?}"),
    }
}

#[cfg(target_arch = "wasm32")]
fn print_frame(
    label: &str,
    result: Result<imago::camera::types::Frame, imago::camera::types::CameraError>,
) {
    match result {
        Ok(frame) => {
            println!(
                "{label}: seq={} bytes={} format={} {}x{} stride={} timestamp_ns={}",
                frame.sequence,
                frame.bytes.len(),
                pixel_format_name(frame.format),
                frame.width_px,
                frame.height_px,
                frame.stride_bytes,
                frame.timestamp_ns
            );
        }
        Err(err) => {
            eprintln!("{label}: failed: {err:?}");
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn pixel_format_name(format: imago::camera::types::PixelFormat) -> &'static str {
    match format {
        imago::camera::types::PixelFormat::Rgba8 => "rgba8",
    }
}
