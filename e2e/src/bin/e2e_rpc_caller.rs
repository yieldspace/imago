use wit_bindgen::generate;

generate!({
    world: "rpc-caller",
    generate_all
});

fn main() {
    loop {
        let connection = match std::env::var("IMAGO_RPC_ADDR").ok() {
            Some(addr) if !addr.trim().is_empty() => imago::node::rpc::connect(addr.trim()),
            _ => imago::node::rpc::local(),
        };

        match connection {
            Ok(connection) => match acme::clock::api::now(&connection) {
                Ok(now) => println!("acme:clock/api.now => {now}"),
                Err(err) => eprintln!("acme:clock/api.now failed: {err}"),
            },
            Err(err) => eprintln!("imago:node/rpc connection failed: {err}"),
        }

        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}
