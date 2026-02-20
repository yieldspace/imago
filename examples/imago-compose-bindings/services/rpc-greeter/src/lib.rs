#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    world: "rpc-greeter"
});

#[cfg(target_arch = "wasm32")]
struct RpcGreeter;

#[cfg(target_arch = "wasm32")]
impl exports::acme::clock::api::Guest for RpcGreeter {
    fn now() -> String {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => duration.as_secs().to_string(),
            Err(error) => {
                let seconds = error.duration().as_secs();
                format!("-{seconds}")
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
export!(RpcGreeter);
