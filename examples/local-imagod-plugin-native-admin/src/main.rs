wit_bindgen::generate!({
    path: "wit",
    generate_all
});

fn main() {
    let service_name = imago::admin::runtime::service_name();
    let release_hash = imago::admin::runtime::release_hash();
    let runner_id = imago::admin::runtime::runner_id();
    let app_type = imago::admin::runtime::app_type();

    println!("imago-admin service-name={service_name}");
    println!("imago-admin release-hash={release_hash}");
    println!("imago-admin runner-id={runner_id}");
    println!("imago-admin app-type={app_type}");

    std::thread::sleep(std::time::Duration::from_secs(5));
}
