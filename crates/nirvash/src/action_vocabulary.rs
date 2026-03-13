pub trait ActionVocabulary: Sized {
    fn action_vocabulary() -> Vec<Self>;
}
