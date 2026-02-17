wit_bindgen::generate!({
        path: "wit",
        world: "plugin-imports",
        generate_all
    });

fn main() {
    let message = chikoski::hello::greet::hello();
    println!("message: {}", message);
}
