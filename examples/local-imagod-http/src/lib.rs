#![cfg(target_arch = "wasm32")]
use std::io::Write as _;

use wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

wasi::http::proxy::export!(Example);

struct Example;

impl wasi::exports::http::incoming_handler::Guest for Example {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let response = OutgoingResponse::new(Fields::new());
        response
            .set_status_code(200)
            .expect("status should be valid");
        let body = response.body().expect("response body should be created");

        ResponseOutparam::set(response_out, Ok(response));

        let mut out = body.write().expect("body writer should be available");
        out.write_all(b"hello from local-imagod-http\n")
            .expect("body write should succeed");
        out.flush().expect("body flush should succeed");
        drop(out);

        OutgoingBody::finish(body, None).expect("body finish should succeed");
    }
}
