wit_bindgen::generate!({
    path: "wit",
    generate_all
});

fn main() {
    println!("calling sizumita:ferris/says.say ...");
    sizumita::ferris::says::say("hello from imago");
    println!("done: called sizumita:ferris/says.say");
}
