#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    generate_all,
});

#[cfg(target_arch = "wasm32")]
fn main() {
    let connection = match imago::node::rpc::local() {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("imago:node/rpc.local failed: {err}");
            return;
        }
    };

    loop {
        match acme::clock::api::now(&connection) {
            Ok(now) => println!("acme:clock/api.now => {now}"),
            Err(err) => eprintln!("acme:clock/api.now failed: {err}"),
        }

        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
