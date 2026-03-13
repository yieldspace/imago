use std::{
    any::{Any, TypeId, type_name},
    fmt,
    marker::PhantomData,
};

use serde::{Deserialize, Serialize};

use crate::{BoundedDomain, FiniteModelDomain, SymbolicEncoding, SymbolicSort};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationError {
    message: String,
}

impl RelationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RelationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RelationError {}

pub trait RelAtom: FiniteModelDomain {
    fn rel_index(&self) -> usize {
        Self::finite_domain()
            .into_vec()
            .into_iter()
            .position(|candidate| candidate == self.clone())
            .expect("RelAtom value must belong to FiniteModelDomain::finite_domain()")
    }

    fn rel_from_index(index: usize) -> Option<Self> {
        Self::finite_domain().into_vec().into_iter().nth(index)
    }

    fn rel_label(&self) -> String {
        format!("{self:?}")
    }

    fn rel_domain_len() -> usize {
        Self::finite_domain().len()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RelSet<T> {
    bits: Vec<u64>,
    _marker: PhantomData<T>,
}

impl<T> Default for RelSet<T>
where
    T: RelAtom,
{
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> RelSet<T>
where
    T: RelAtom,
{
    pub fn empty() -> Self {
        Self {
            bits: zero_bits(domain_len::<T>()),
            _marker: PhantomData,
        }
    }

    pub fn from_items<I>(items: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        let mut set = Self::empty();
        for item in items {
            set.insert(item);
        }
        set
    }

    pub fn insert(&mut self, item: T) {
        set_bit(&mut self.bits, item.rel_index());
    }

    pub fn remove(&mut self, item: &T) {
        clear_bit(&mut self.bits, item.rel_index());
    }

    pub fn contains(&self, item: &T) -> bool {
        get_bit(&self.bits, item.rel_index())
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            bits: zip_bits(&self.bits, &other.bits, |left, right| left | right),
            _marker: PhantomData,
        }
    }

    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            bits: zip_bits(&self.bits, &other.bits, |left, right| left & right),
            _marker: PhantomData,
        }
    }

    pub fn difference(&self, other: &Self) -> Self {
        Self {
            bits: zip_bits(&self.bits, &other.bits, |left, right| left & !right),
            _marker: PhantomData,
        }
    }

    pub fn subset_of(&self, other: &Self) -> bool {
        self.bits
            .iter()
            .zip(other.bits.iter())
            .all(|(left, right)| left & !right == 0)
    }

    pub fn cardinality(&self) -> usize {
        self.bits
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    pub fn some(&self) -> bool {
        self.cardinality() > 0
    }

    pub fn no(&self) -> bool {
        !self.some()
    }

    pub fn one(&self) -> bool {
        self.cardinality() == 1
    }

    pub fn lone(&self) -> bool {
        self.cardinality() <= 1
    }

    pub fn items(&self) -> Vec<T> {
        (0..domain_len::<T>())
            .filter(|index| get_bit(&self.bits, *index))
            .filter_map(T::rel_from_index)
            .collect()
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.items()
    }

    pub fn relation_summary(&self, name: &str) -> RelationFieldSummary {
        <Self as RelationField>::relation_summary(self, name)
    }
}

impl<T> fmt::Debug for RelSet<T>
where
    T: RelAtom,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let labels = self
            .items()
            .into_iter()
            .map(|item| item.rel_label())
            .collect::<Vec<_>>();
        if labels.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&labels.join(" + "))
        }
    }
}

impl<T> FiniteModelDomain for RelSet<T>
where
    T: RelAtom,
{
    fn finite_domain() -> BoundedDomain<Self> {
        let mut values = vec![Self::empty()];
        for item in T::finite_domain().into_vec() {
            let mut with_item = values.clone();
            for set in &mut with_item {
                set.insert(item.clone());
            }
            values.extend(with_item);
        }
        BoundedDomain::new(values)
    }
}

impl<T> SymbolicEncoding for RelSet<T>
where
    T: RelAtom + SymbolicEncoding,
{
    fn symbolic_sort() -> SymbolicSort {
        let element_sort = T::symbolic_sort();
        SymbolicSort::RelSet {
            type_name: type_name::<Self>(),
            domain_size: power_set_domain_size(element_sort.domain_size()),
            element: Box::new(element_sort),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Relation2<A, B> {
    bits: Vec<u64>,
    cols: usize,
    _marker: PhantomData<(A, B)>,
}

impl<A, B> Default for Relation2<A, B>
where
    A: RelAtom,
    B: RelAtom,
{
    fn default() -> Self {
        Self::empty()
    }
}

impl<A, B> Relation2<A, B>
where
    A: RelAtom,
    B: RelAtom,
{
    pub fn empty() -> Self {
        let cols = domain_len::<B>();
        Self {
            bits: zero_bits(domain_len::<A>().saturating_mul(cols)),
            cols,
            _marker: PhantomData,
        }
    }

    pub fn from_pairs<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (A, B)>,
    {
        let mut relation = Self::empty();
        for (left, right) in pairs {
            relation.insert(left, right);
        }
        relation
    }

    pub fn insert(&mut self, left: A, right: B) {
        let bit_index = self.bit_index(left.rel_index(), right.rel_index());
        set_bit(&mut self.bits, bit_index);
    }

    pub fn remove(&mut self, left: &A, right: &B) {
        let bit_index = self.bit_index(left.rel_index(), right.rel_index());
        clear_bit(&mut self.bits, bit_index);
    }

    pub fn contains(&self, left: &A, right: &B) -> bool {
        get_bit(
            &self.bits,
            self.bit_index(left.rel_index(), right.rel_index()),
        )
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            bits: zip_bits(&self.bits, &other.bits, |left, right| left | right),
            cols: self.cols,
            _marker: PhantomData,
        }
    }

    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            bits: zip_bits(&self.bits, &other.bits, |left, right| left & right),
            cols: self.cols,
            _marker: PhantomData,
        }
    }

    pub fn difference(&self, other: &Self) -> Self {
        Self {
            bits: zip_bits(&self.bits, &other.bits, |left, right| left & !right),
            cols: self.cols,
            _marker: PhantomData,
        }
    }

    pub fn subset_of(&self, other: &Self) -> bool {
        self.bits
            .iter()
            .zip(other.bits.iter())
            .all(|(left, right)| left & !right == 0)
    }

    pub fn cardinality(&self) -> usize {
        self.bits
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    pub fn some(&self) -> bool {
        self.cardinality() > 0
    }

    pub fn no(&self) -> bool {
        !self.some()
    }

    pub fn one(&self) -> bool {
        self.cardinality() == 1
    }

    pub fn lone(&self) -> bool {
        self.cardinality() <= 1
    }

    pub fn domain(&self) -> RelSet<A> {
        RelSet::from_items(self.pairs().into_iter().map(|(left, _)| left))
    }

    pub fn range(&self) -> RelSet<B> {
        RelSet::from_items(self.pairs().into_iter().map(|(_, right)| right))
    }

    pub fn transpose(&self) -> Relation2<B, A> {
        Relation2::from_pairs(self.pairs().into_iter().map(|(left, right)| (right, left)))
    }

    pub fn join<C>(&self, other: &Relation2<B, C>) -> Relation2<A, C>
    where
        C: RelAtom,
    {
        let mut result = Relation2::empty();
        for (left, middle) in self.pairs() {
            for right in other.right_values(&middle) {
                result.insert(left.clone(), right);
            }
        }
        result
    }

    pub fn transitive_closure_checked(&self) -> Result<Self, RelationError> {
        if type_name::<A>() != type_name::<B>() {
            return Err(RelationError::new(format!(
                "transitive closure requires identical atom types, got `{}` and `{}`",
                type_name::<A>(),
                type_name::<B>()
            )));
        }
        let mut closure = self.clone();
        let size = domain_len::<A>().min(domain_len::<B>());
        for pivot in 0..size {
            for left in 0..size {
                if !get_bit(&closure.bits, closure.bit_index(left, pivot)) {
                    continue;
                }
                for right in 0..size {
                    if get_bit(&closure.bits, closure.bit_index(pivot, right)) {
                        let bit_index = closure.bit_index(left, right);
                        set_bit(&mut closure.bits, bit_index);
                    }
                }
            }
        }
        Ok(closure)
    }

    pub fn relation_summary(&self, name: &str) -> RelationFieldSummary {
        <Self as RelationField>::relation_summary(self, name)
    }

    pub fn pairs(&self) -> Vec<(A, B)> {
        let mut pairs = Vec::new();
        for left_index in 0..domain_len::<A>() {
            let Some(left) = A::rel_from_index(left_index) else {
                continue;
            };
            for right_index in 0..domain_len::<B>() {
                if !get_bit(&self.bits, self.bit_index(left_index, right_index)) {
                    continue;
                }
                if let Some(right) = B::rel_from_index(right_index) {
                    pairs.push((left.clone(), right));
                }
            }
        }
        pairs
    }

    pub fn to_vec(&self) -> Vec<(A, B)> {
        self.pairs()
    }

    fn right_values(&self, left: &A) -> Vec<B> {
        (0..domain_len::<B>())
            .filter(|right_index| {
                get_bit(&self.bits, self.bit_index(left.rel_index(), *right_index))
            })
            .filter_map(B::rel_from_index)
            .collect()
    }

    fn bit_index(&self, left_index: usize, right_index: usize) -> usize {
        left_index
            .saturating_mul(self.cols)
            .saturating_add(right_index)
    }
}

impl<T> Relation2<T, T>
where
    T: RelAtom,
{
    pub fn transitive_closure(&self) -> Result<Self, RelationError> {
        let mut closure = self.clone();
        for pivot in 0..domain_len::<T>() {
            let Some(pivot_atom) = T::rel_from_index(pivot) else {
                continue;
            };
            for left in 0..domain_len::<T>() {
                let Some(left_atom) = T::rel_from_index(left) else {
                    continue;
                };
                if !closure.contains(&left_atom, &pivot_atom) {
                    continue;
                }
                for right in 0..domain_len::<T>() {
                    let Some(right_atom) = T::rel_from_index(right) else {
                        continue;
                    };
                    if closure.contains(&pivot_atom, &right_atom) {
                        closure.insert(left_atom.clone(), right_atom);
                    }
                }
            }
        }
        Ok(closure)
    }
}

impl<A, B> fmt::Debug for Relation2<A, B>
where
    A: RelAtom,
    B: RelAtom,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pairs = self
            .pairs()
            .into_iter()
            .map(|(left, right)| format!("{}->{}", left.rel_label(), right.rel_label()))
            .collect::<Vec<_>>();
        if pairs.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&pairs.join(" + "))
        }
    }
}

impl<A, B> FiniteModelDomain for Relation2<A, B>
where
    A: RelAtom,
    B: RelAtom,
{
    fn finite_domain() -> BoundedDomain<Self> {
        let pair_domain = A::finite_domain()
            .into_vec()
            .into_iter()
            .flat_map(|left| {
                B::finite_domain()
                    .into_vec()
                    .into_iter()
                    .map(move |right| (left.clone(), right))
            })
            .collect::<Vec<_>>();

        let mut values = vec![Self::empty()];
        for (left, right) in pair_domain {
            let mut with_pair = values.clone();
            for relation in &mut with_pair {
                relation.insert(left.clone(), right.clone());
            }
            values.extend(with_pair);
        }
        BoundedDomain::new(values)
    }
}

impl<A, B> SymbolicEncoding for Relation2<A, B>
where
    A: RelAtom + SymbolicEncoding,
    B: RelAtom + SymbolicEncoding,
{
    fn symbolic_sort() -> SymbolicSort {
        let left = A::symbolic_sort();
        let right = B::symbolic_sort();
        SymbolicSort::Relation2 {
            type_name: type_name::<Self>(),
            domain_size: power_set_domain_size(
                left.domain_size().saturating_mul(right.domain_size()),
            ),
            left: Box::new(left),
            right: Box::new(right),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RelationFieldKind {
    Set,
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationFieldSchema {
    pub name: String,
    pub kind: RelationFieldKind,
    pub from_type: String,
    pub to_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationFieldSummary {
    pub name: String,
    pub notation: String,
}

pub trait RelationField {
    fn relation_schema(name: &str) -> RelationFieldSchema
    where
        Self: Sized;

    fn relation_summary(&self, name: &str) -> RelationFieldSummary;
}

impl<T> RelationField for RelSet<T>
where
    T: RelAtom,
{
    fn relation_schema(name: &str) -> RelationFieldSchema {
        RelationFieldSchema {
            name: name.to_owned(),
            kind: RelationFieldKind::Set,
            from_type: short_type_name::<T>(),
            to_type: None,
        }
    }

    fn relation_summary(&self, name: &str) -> RelationFieldSummary {
        RelationFieldSummary {
            name: name.to_owned(),
            notation: format!("{name} = {self:?}"),
        }
    }
}

impl<A, B> RelationField for Relation2<A, B>
where
    A: RelAtom,
    B: RelAtom,
{
    fn relation_schema(name: &str) -> RelationFieldSchema {
        RelationFieldSchema {
            name: name.to_owned(),
            kind: RelationFieldKind::Binary,
            from_type: short_type_name::<A>(),
            to_type: Some(short_type_name::<B>()),
        }
    }

    fn relation_summary(&self, name: &str) -> RelationFieldSummary {
        RelationFieldSummary {
            name: name.to_owned(),
            notation: format!("{name} = {self:?}"),
        }
    }
}

pub trait RelationalState {
    fn relation_schema() -> Vec<RelationFieldSchema>
    where
        Self: Sized;

    fn relation_summary(&self) -> Vec<RelationFieldSummary>;
}

pub struct RegisteredRelationalState {
    pub state_type_id: fn() -> TypeId,
    pub relation_schema: fn() -> Vec<RelationFieldSchema>,
    pub relation_summary: fn(&dyn Any) -> Vec<RelationFieldSummary>,
}

inventory::collect!(RegisteredRelationalState);

pub fn collect_relational_state_schema<T>() -> Vec<RelationFieldSchema>
where
    T: 'static,
{
    let type_id = TypeId::of::<T>();
    inventory::iter::<RegisteredRelationalState>
        .into_iter()
        .find(|entry| (entry.state_type_id)() == type_id)
        .map(|entry| (entry.relation_schema)())
        .unwrap_or_default()
}

pub fn collect_relational_state_summary<T>(value: &T) -> Vec<RelationFieldSummary>
where
    T: 'static,
{
    let type_id = TypeId::of::<T>();
    inventory::iter::<RegisteredRelationalState>
        .into_iter()
        .find(|entry| (entry.state_type_id)() == type_id)
        .map(|entry| (entry.relation_summary)(value as &dyn Any))
        .unwrap_or_default()
}

fn domain_len<T>() -> usize
where
    T: RelAtom,
{
    T::finite_domain().len()
}

fn short_type_name<T>() -> String {
    type_name::<T>()
        .rsplit("::")
        .next()
        .unwrap_or(type_name::<T>())
        .to_owned()
}

fn power_set_domain_size(exponent: usize) -> usize {
    if exponent >= usize::BITS as usize {
        usize::MAX
    } else {
        1usize << exponent
    }
}

fn zero_bits(size: usize) -> Vec<u64> {
    vec![0; size.div_ceil(64)]
}

fn set_bit(bits: &mut [u64], index: usize) {
    let word = index / 64;
    let offset = index % 64;
    if let Some(slot) = bits.get_mut(word) {
        *slot |= 1_u64 << offset;
    }
}

fn clear_bit(bits: &mut [u64], index: usize) {
    let word = index / 64;
    let offset = index % 64;
    if let Some(slot) = bits.get_mut(word) {
        *slot &= !(1_u64 << offset);
    }
}

fn get_bit(bits: &[u64], index: usize) -> bool {
    let word = index / 64;
    let offset = index % 64;
    bits.get(word)
        .is_some_and(|slot| (*slot & (1_u64 << offset)) != 0)
}

fn zip_bits<F>(left: &[u64], right: &[u64], mut op: F) -> Vec<u64>
where
    F: FnMut(u64, u64) -> u64,
{
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| op(*left, *right))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoundedDomain, FiniteModelDomain};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Node {
        A,
        B,
        C,
    }

    impl FiniteModelDomain for Node {
        fn finite_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::A, Self::B, Self::C])
        }
    }

    impl RelAtom for Node {}

    impl SymbolicEncoding for Node {
        fn symbolic_sort() -> SymbolicSort {
            SymbolicSort::finite::<Self>()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Color {
        Red,
        Blue,
    }

    impl FiniteModelDomain for Color {
        fn finite_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Red, Self::Blue])
        }
    }

    impl RelAtom for Color {}

    impl SymbolicEncoding for Color {
        fn symbolic_sort() -> SymbolicSort {
            SymbolicSort::finite::<Self>()
        }
    }

    #[test]
    fn rel_set_supports_basic_algebra() {
        let left = RelSet::from_items([Node::A, Node::B]);
        let right = RelSet::from_items([Node::B, Node::C]);

        let union = left.union(&right);
        assert!(union.contains(&Node::A));
        assert!(union.contains(&Node::C));

        let intersection = left.intersection(&right);
        assert_eq!(intersection.items(), vec![Node::B]);

        let difference = left.difference(&right);
        assert_eq!(difference.items(), vec![Node::A]);
        assert!(difference.one());
        assert!(!difference.no());
    }

    #[test]
    fn relation2_supports_join_and_transpose() {
        let edges = Relation2::from_pairs([(Node::A, Node::B), (Node::B, Node::C)]);
        let colors = Relation2::from_pairs([(Node::B, Color::Red), (Node::C, Color::Blue)]);

        let joined = edges.join(&colors);
        assert!(joined.contains(&Node::A, &Color::Red));
        assert!(joined.contains(&Node::B, &Color::Blue));

        let transposed = edges.transpose();
        assert!(transposed.contains(&Node::B, &Node::A));
        assert!(transposed.contains(&Node::C, &Node::B));
    }

    #[test]
    fn relation2_supports_domain_range_and_cardinality() {
        let edges = Relation2::from_pairs([(Node::A, Node::B), (Node::A, Node::C)]);

        assert_eq!(edges.domain().items(), vec![Node::A]);
        assert_eq!(edges.range().items(), vec![Node::B, Node::C]);
        assert_eq!(edges.cardinality(), 2);
        assert!(edges.some());
        assert!(!edges.lone());
    }

    #[test]
    fn homogeneous_relation_transitive_closure_adds_reachable_pairs() {
        let edges = Relation2::from_pairs([(Node::A, Node::B), (Node::B, Node::C)]);

        let closure = edges.transitive_closure().expect("closure");
        assert!(closure.contains(&Node::A, &Node::C));
    }

    #[test]
    fn heterogeneous_relation_transitive_closure_fails_closed() {
        let relation = Relation2::from_pairs([(Node::A, Color::Red)]);

        let error = relation
            .transitive_closure_checked()
            .expect_err("must fail");
        assert!(
            error
                .to_string()
                .contains("transitive closure requires identical atom types")
        );
    }

    #[test]
    fn relation_debug_uses_alloy_style_notation() {
        let relation = Relation2::from_pairs([(Node::A, Node::B), (Node::B, Node::C)]);
        assert_eq!(format!("{relation:?}"), "A->B + B->C");

        let set = RelSet::from_items([Node::A, Node::C]);
        assert_eq!(format!("{set:?}"), "A + C");
    }

    #[test]
    fn rel_set_signature_domain_covers_all_subsets() {
        let domain = RelSet::<Node>::bounded_domain().into_vec();
        assert_eq!(domain.len(), 8);
        assert!(domain.contains(&RelSet::empty()));
        assert!(domain.contains(&RelSet::from_items([Node::A, Node::C])));
    }

    #[test]
    fn relation2_signature_domain_covers_all_pair_subsets() {
        let domain = Relation2::<Node, Color>::bounded_domain().into_vec();
        assert_eq!(domain.len(), 64);
        assert!(domain.contains(&Relation2::empty()));
        assert!(domain.contains(&Relation2::from_pairs([
            (Node::A, Color::Red),
            (Node::C, Color::Blue),
        ])));
    }

    #[test]
    fn relation_symbolic_sort_domain_sizes_are_computed_combinatorially() {
        let set_sort = RelSet::<Node>::symbolic_sort();
        assert_eq!(set_sort.domain_size(), 8);

        let relation_sort = Relation2::<Node, Color>::symbolic_sort();
        assert_eq!(relation_sort.domain_size(), 64);
    }
}
