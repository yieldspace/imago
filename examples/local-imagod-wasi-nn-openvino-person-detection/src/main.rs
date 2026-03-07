#[cfg(any(target_arch = "wasm32", test))]
use std::convert::TryInto;

#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    world: "nn-imports",
    generate_all,
});

#[cfg(target_arch = "wasm32")]
use wasi::nn::{
    errors::Error as WasiNnError,
    graph::{self, ExecutionTarget, GraphEncoding},
    tensor::{Tensor, TensorType},
};

#[cfg(any(target_arch = "wasm32", test))]
const IMAGE_WIDTH: u32 = 512;
#[cfg(any(target_arch = "wasm32", test))]
const IMAGE_HEIGHT: u32 = 512;
#[cfg(target_arch = "wasm32")]
const OUTPUT_ROWS: u32 = 200;
#[cfg(any(target_arch = "wasm32", test))]
const OUTPUT_COLUMNS: u32 = 7;
#[cfg(target_arch = "wasm32")]
const CONFIDENCE_THRESHOLD: f32 = 0.5;

#[cfg(target_arch = "wasm32")]
const MODEL_XML_PATH: &str = "/app/assets/model.xml";
#[cfg(target_arch = "wasm32")]
const MODEL_BIN_PATH: &str = "/app/assets/model.bin";
#[cfg(target_arch = "wasm32")]
const IMAGE_PATH: &str = "/app/assets/people.ppm";
#[cfg(target_arch = "wasm32")]
const INPUT_NAME: &str = "image";
#[cfg(target_arch = "wasm32")]
const OUTPUT_NAME: &str = "detection_out";

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct PpmImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, PartialEq)]
struct Detection {
    confidence: f32,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        println!("build this example for wasm32-wasip2");
    }

    #[cfg(target_arch = "wasm32")]
    {
        if let Err(err) = run() {
            eprintln!("local-imagod-wasi-nn-openvino-person-detection-app failed: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn parse_p6_ppm(bytes: &[u8]) -> Result<PpmImage, String> {
    let (magic, offset) = next_ppm_token(bytes, 0)?;
    if magic != b"P6" {
        return Err(format!(
            "unsupported ppm magic: {}",
            String::from_utf8_lossy(magic)
        ));
    }
    let (width_token, offset) = next_ppm_token(bytes, offset)?;
    let (height_token, offset) = next_ppm_token(bytes, offset)?;
    let (max_value_token, offset) = next_ppm_token(bytes, offset)?;

    let width = parse_ascii_u32(width_token, "width")?;
    let height = parse_ascii_u32(height_token, "height")?;
    let max_value = parse_ascii_u32(max_value_token, "max value")?;
    if max_value != 255 {
        return Err(format!("unsupported ppm max value: {max_value}"));
    }

    let Some(separator) = bytes.get(offset) else {
        return Err("ppm header was truncated before pixel data".to_string());
    };
    if !separator.is_ascii_whitespace() {
        return Err("ppm header must be followed by whitespace".to_string());
    }
    let data_offset = if *separator == b'\r' && bytes.get(offset + 1) == Some(&b'\n') {
        offset + 2
    } else {
        offset + 1
    };

    let expected_len = width as usize * height as usize * 3;
    let data = bytes
        .get(data_offset..)
        .ok_or_else(|| "ppm pixel data was missing".to_string())?;
    if data.len() != expected_len {
        return Err(format!(
            "ppm pixel data length mismatch: expected {expected_len}, got {}",
            data.len()
        ));
    }

    Ok(PpmImage {
        width,
        height,
        data: data.to_vec(),
    })
}

#[cfg(any(target_arch = "wasm32", test))]
fn next_ppm_token(bytes: &[u8], mut offset: usize) -> Result<(&[u8], usize), String> {
    while offset < bytes.len() {
        match bytes[offset] {
            b'#' => {
                offset += 1;
                while offset < bytes.len() && bytes[offset] != b'\n' {
                    offset += 1;
                }
            }
            byte if byte.is_ascii_whitespace() => {
                offset += 1;
            }
            _ => break,
        }
    }

    let start = offset;
    while offset < bytes.len() && !bytes[offset].is_ascii_whitespace() && bytes[offset] != b'#' {
        offset += 1;
    }

    if start == offset {
        return Err("ppm token was missing".to_string());
    }
    Ok((&bytes[start..offset], offset))
}

#[cfg(any(target_arch = "wasm32", test))]
fn parse_ascii_u32(token: &[u8], field_name: &str) -> Result<u32, String> {
    let text = std::str::from_utf8(token)
        .map_err(|err| format!("invalid utf-8 in ppm {field_name}: {err}"))?;
    text.parse::<u32>()
        .map_err(|err| format!("invalid ppm {field_name} `{text}`: {err}"))
}

#[cfg(any(target_arch = "wasm32", test))]
fn ppm_to_tensor_data(image: &PpmImage) -> Result<Vec<u8>, String> {
    if image.width != IMAGE_WIDTH || image.height != IMAGE_HEIGHT {
        return Err(format!(
            "unexpected image size: expected {}x{}, got {}x{}",
            IMAGE_WIDTH, IMAGE_HEIGHT, image.width, image.height
        ));
    }

    let pixel_count = image.width as usize * image.height as usize;
    if image.data.len() != pixel_count * 3 {
        return Err(format!(
            "unexpected ppm pixel buffer size: expected {}, got {}",
            pixel_count * 3,
            image.data.len()
        ));
    }

    let mut tensor = Vec::with_capacity(pixel_count * 3 * std::mem::size_of::<f32>());
    for channel_offset in [2usize, 1, 0] {
        for pixel_index in 0..pixel_count {
            let value = image.data[pixel_index * 3 + channel_offset] as f32;
            tensor.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(tensor)
}

#[cfg(any(target_arch = "wasm32", test))]
fn parse_detection_output_bytes(
    bytes: &[u8],
    image_width: u32,
    image_height: u32,
    threshold: f32,
) -> Result<Vec<Detection>, String> {
    let row_byte_len = OUTPUT_COLUMNS as usize * std::mem::size_of::<f32>();
    if !bytes.len().is_multiple_of(row_byte_len) {
        return Err(format!(
            "detection tensor byte length was not divisible by {row_byte_len}: {}",
            bytes.len()
        ));
    }

    let mut detections = Vec::new();
    for row in bytes.chunks_exact(row_byte_len) {
        let image_id = decode_f32(&row[0..4])?;
        if image_id < 0.0 {
            continue;
        }

        let confidence = decode_f32(&row[8..12])?;
        if confidence < threshold {
            continue;
        }

        let xmin = decode_f32(&row[12..16])?.clamp(0.0, 1.0);
        let ymin = decode_f32(&row[16..20])?.clamp(0.0, 1.0);
        let xmax = decode_f32(&row[20..24])?.clamp(0.0, 1.0);
        let ymax = decode_f32(&row[24..28])?.clamp(0.0, 1.0);

        let left = (xmin * image_width as f32).floor() as u32;
        let top = (ymin * image_height as f32).floor() as u32;
        let right = (xmax * image_width as f32)
            .ceil()
            .clamp(0.0, image_width as f32) as u32;
        let bottom = (ymax * image_height as f32)
            .ceil()
            .clamp(0.0, image_height as f32) as u32;

        if right <= left || bottom <= top {
            continue;
        }

        detections.push(Detection {
            confidence,
            left,
            top,
            right,
            bottom,
        });
    }

    Ok(detections)
}

#[cfg(any(target_arch = "wasm32", test))]
fn decode_f32(bytes: &[u8]) -> Result<f32, String> {
    let raw: [u8; 4] = bytes
        .try_into()
        .map_err(|_| format!("failed to decode f32 from {} bytes", bytes.len()))?;
    Ok(f32::from_le_bytes(raw))
}

#[cfg(any(target_arch = "wasm32", test))]
fn select_output_index(output_names: &[String], preferred_name: &str) -> Option<usize> {
    output_names
        .iter()
        .position(|name| name == preferred_name)
        .or_else(|| (output_names.len() == 1).then_some(0))
}

#[cfg(target_arch = "wasm32")]
fn run() -> Result<(), String> {
    let model_xml = std::fs::read(MODEL_XML_PATH)
        .map_err(|err| format!("failed to read {MODEL_XML_PATH}: {err}"))?;
    let model_bin = std::fs::read(MODEL_BIN_PATH)
        .map_err(|err| format!("failed to read {MODEL_BIN_PATH}: {err}"))?;
    let image_bytes =
        std::fs::read(IMAGE_PATH).map_err(|err| format!("failed to read {IMAGE_PATH}: {err}"))?;
    let image = parse_p6_ppm(&image_bytes)?;
    let input_bytes = ppm_to_tensor_data(&image)?;
    let builders = vec![model_xml, model_bin];
    let graph = graph::load(&builders, GraphEncoding::Openvino, ExecutionTarget::Cpu)
        .map_err(format_wasi_nn_error)?;
    let context = graph
        .init_execution_context()
        .map_err(format_wasi_nn_error)?;
    let input_dimensions = [1, 3, IMAGE_HEIGHT, IMAGE_WIDTH];
    let input = Tensor::new(&input_dimensions, TensorType::Fp32, &input_bytes);
    let outputs = context
        .compute(vec![(INPUT_NAME.to_string(), input)])
        .map_err(format_wasi_nn_error)?;
    let available_output_names = outputs
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let output_index =
        select_output_index(&available_output_names, OUTPUT_NAME).ok_or_else(|| {
            format!("missing `{OUTPUT_NAME}` output; available outputs: {available_output_names:?}")
        })?;
    let (_, output_tensor) = outputs
        .into_iter()
        .nth(output_index)
        .expect("selected output index should exist");

    if output_tensor.ty() != TensorType::Fp32 {
        return Err(format!(
            "unexpected output tensor type: {:?}",
            output_tensor.ty()
        ));
    }
    let output_dimensions = output_tensor.dimensions();
    if output_dimensions != vec![1, 1, OUTPUT_ROWS, OUTPUT_COLUMNS] {
        return Err(format!(
            "unexpected output dimensions: {output_dimensions:?}"
        ));
    }

    let detections = parse_detection_output_bytes(
        &output_tensor.data(),
        image.width,
        image.height,
        CONFIDENCE_THRESHOLD,
    )?;
    if detections.is_empty() {
        return Err("no persons detected in input image".to_string());
    }

    println!("detected_persons={}", detections.len());
    for (index, detection) in detections.iter().enumerate() {
        println!(
            "bbox[{index}]=left={},top={},right={},bottom={},confidence={:.3}",
            detection.left, detection.top, detection.right, detection.bottom, detection.confidence,
        );
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn format_wasi_nn_error(error: WasiNnError) -> String {
    format!("{:?}: {}", error.code(), error.data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_p6_ppm_supports_comments() {
        let ppm = b"P6\n# generated test image\n2 1\n255\n\x00\x11\x22\x33\x44\x55";
        let image = parse_p6_ppm(ppm).expect("ppm should parse");
        assert_eq!(
            image,
            PpmImage {
                width: 2,
                height: 1,
                data: vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
            }
        );
    }

    #[test]
    fn ppm_to_tensor_data_rejects_unexpected_dimensions() {
        let image = PpmImage {
            width: 2,
            height: 1,
            data: vec![0, 1, 2, 3, 4, 5],
        };
        let err = ppm_to_tensor_data(&image).expect_err("unexpected size should fail");
        assert!(
            err.contains("unexpected image size"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_detection_output_filters_threshold_and_scales_boxes() {
        let mut bytes = Vec::new();
        for value in [
            0.0f32, 1.0, 0.80, 0.10, 0.20, 0.40, 0.60, -1.0, 1.0, 0.99, 0.0, 0.0, 0.0, 0.0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        let detections = parse_detection_output_bytes(&bytes, 512, 512, 0.5)
            .expect("detection tensor should parse");
        assert_eq!(
            detections,
            vec![Detection {
                confidence: 0.80,
                left: 51,
                top: 102,
                right: 205,
                bottom: 308,
            }]
        );
    }

    #[test]
    fn select_output_index_falls_back_to_only_output() {
        let names = vec!["0".to_string()];
        assert_eq!(select_output_index(&names, "detection_out"), Some(0));
    }

    #[test]
    fn select_output_index_prefers_named_output() {
        let names = vec!["0".to_string(), "detection_out".to_string()];
        assert_eq!(select_output_index(&names, "detection_out"), Some(1));
    }
}
