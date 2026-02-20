#![no_main]

use wit_bindgen::generate;

generate!({
    path: "wit/rpc-greeter",
    world: "acme:clock/rpc-greeter",
    generate_all,
});

struct Greeter;

impl exports::acme::clock::api::Guest for Greeter {
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

export!(Greeter);
