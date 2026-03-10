use nirvash_macros::ProtocolInputWitness;

#[derive(Clone, Debug, ProtocolInputWitness)]
enum BadInput {
    Start,
}

fn main() {}
