#[derive(Clone, Copy, Debug, PartialEq, Eq, nirvash_macros::Signature)]
enum Payload {
    First,
    Second,
}

fn payload_domain() -> Vec<Payload> {
    vec![Payload::First]
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    nirvash_macros::Signature,
    nirvash_macros::ActionVocabulary
)]
enum DemoAction {
    Tick,
    Wrap(#[sig(domain = payload_domain)] Payload),
}

fn main() {
    let vocabulary = <DemoAction as nirvash::ActionVocabulary>::action_vocabulary();
    assert_eq!(
        vocabulary,
        vec![DemoAction::Tick, DemoAction::Wrap(Payload::First)]
    );
}
