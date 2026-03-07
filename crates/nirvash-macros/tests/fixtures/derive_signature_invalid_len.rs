use nirvash_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
struct InvalidLen {
    #[sig(len = "0..=2")]
    ready: bool,
}

fn main() {}
