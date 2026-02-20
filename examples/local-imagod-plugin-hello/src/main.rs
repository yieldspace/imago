#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "wit",
    generate_all
});

fn main() {
    println!("calling sizumita:ferris/says.say ...");
    #[cfg(target_arch = "wasm32")]
    sizumita::ferris::says::say("hello from imago");
    println!("done: called sizumita:ferris/says.say");
}
