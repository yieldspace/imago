use wasi::http::outgoing_handler;
use wasi::http::types::{ErrorCode, Fields, OutgoingRequest, Scheme};

const RESULT_KEY: &str = "IMAGO_E2E_HTTP_OUTBOUND_RESULT";
const ERROR_KEY: &str = "IMAGO_E2E_HTTP_OUTBOUND_ERROR";
const DEFAULT_AUTHORITY: &str = "127.0.0.2:18080";

fn main() {
    let authority = std::env::var("IMAGO_E2E_HTTP_TARGET_AUTHORITY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_AUTHORITY.to_string());

    let request = OutgoingRequest::new(Fields::new());
    if let Err(err) = request.set_scheme(Some(&Scheme::Http)) {
        print_error(&format!("set_scheme failed: {err:?}"));
        return;
    }
    if let Err(err) = request.set_authority(Some(&authority)) {
        print_error(&format!("set_authority failed: {err:?}"));
        return;
    }
    if let Err(err) = request.set_path_with_query(Some("/")) {
        print_error(&format!("set_path_with_query failed: {err:?}"));
        return;
    }

    match outgoing_handler::handle(request, None) {
        Ok(_future) => {
            println!("{RESULT_KEY}=allowed");
        }
        Err(err) if matches!(err, ErrorCode::HttpRequestDenied) => {
            println!("{RESULT_KEY}=denied");
        }
        Err(err) => {
            print_error(&format!("handle failed: {err:?}"));
        }
    }
}

fn print_error(message: &str) {
    println!("{RESULT_KEY}=error");
    println!("{ERROR_KEY}={message}");
}
