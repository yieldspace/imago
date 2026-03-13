use nirvash_conformance::{ProtocolInputWitnessCodec, WitnessKind};
use nirvash_macros::{ActionVocabulary as FormalActionVocabulary, ProtocolInputWitness, FiniteModelDomain as FormalFiniteModelDomain};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalActionVocabulary, Default)]
enum Action {
    #[default]
    Start,
    Stop,
}

#[derive(Clone, Debug, ProtocolInputWitness)]
struct NewtypeInput(Action);

#[derive(Clone, Debug, ProtocolInputWitness)]
#[protocol_input_witness(action = Action)]
enum EnumInput {
    Start,
    Stop,
}

#[derive(Clone, Debug, Default, ProtocolInputWitness)]
#[protocol_input_witness(action = Action, field = action)]
struct StructInput {
    action: Action,
    dry_run: bool,
}

fn main() {
    let _ = <NewtypeInput as ProtocolInputWitnessCodec<Action>>::canonical_positive(&Action::Start);
    let _ = <EnumInput as ProtocolInputWitnessCodec<Action>>::positive_family(&Action::Stop);
    let _ = <StructInput as ProtocolInputWitnessCodec<Action>>::negative_family(&Action::Start);
    let _ = <StructInput as ProtocolInputWitnessCodec<Action>>::witness_name(
        &Action::Start,
        WitnessKind::CanonicalPositive,
        0,
    );
}
