use std::{fmt, fmt::Debug, marker::PhantomData};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedDomain<T> {
    values: Vec<T>,
}

impl<T> BoundedDomain<T> {
    pub fn new(values: Vec<T>) -> Self {
        Self { values }
    }

    pub fn singleton(value: T) -> Self {
        Self {
            values: vec![value],
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.values.iter()
    }

    pub fn into_vec(self) -> Vec<T> {
        self.values
    }

    pub fn push(&mut self, value: T) {
        self.values.push(value);
    }

    pub fn map<U, F>(&self, mut f: F) -> BoundedDomain<U>
    where
        F: FnMut(&T) -> U,
    {
        BoundedDomain::new(self.values.iter().map(&mut f).collect())
    }

    pub fn flat_map<U, F>(&self, mut f: F) -> BoundedDomain<U>
    where
        F: FnMut(&T) -> BoundedDomain<U>,
    {
        let mut values = Vec::new();
        for value in &self.values {
            values.extend(f(value).into_vec());
        }
        BoundedDomain::new(values)
    }

    pub fn product<U>(&self, other: &BoundedDomain<U>) -> BoundedDomain<(T, U)>
    where
        T: Clone,
        U: Clone,
    {
        let mut values = Vec::with_capacity(self.len().saturating_mul(other.len()));
        for lhs in &self.values {
            for rhs in &other.values {
                values.push((lhs.clone(), rhs.clone()));
            }
        }
        BoundedDomain::new(values)
    }

    pub fn filter<F>(&self, mut predicate: F) -> Self
    where
        T: Clone,
        F: FnMut(&T) -> bool,
    {
        BoundedDomain::new(
            self.values
                .iter()
                .filter(|value| predicate(value))
                .cloned()
                .collect(),
        )
    }

    pub fn unique(self) -> Self
    where
        T: PartialEq,
    {
        let mut values = Vec::with_capacity(self.values.len());
        for value in self.values {
            if !values.contains(&value) {
                values.push(value);
            }
        }
        BoundedDomain::new(values)
    }
}

impl<T> From<Vec<T>> for BoundedDomain<T> {
    fn from(values: Vec<T>) -> Self {
        Self::new(values)
    }
}

#[derive(Debug, Clone)]
pub struct ExprDomain<T> {
    label: &'static str,
    values: BoundedDomain<T>,
}

impl<T> ExprDomain<T> {
    pub fn new<D>(label: &'static str, values: D) -> Self
    where
        D: IntoBoundedDomain<T>,
    {
        Self {
            label,
            values: values.into_bounded_domain(),
        }
    }

    pub fn of_signature(label: &'static str) -> Self
    where
        T: Signature,
    {
        Self {
            label,
            values: T::bounded_domain(),
        }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.values.iter()
    }

    pub fn as_bounded_domain(&self) -> &BoundedDomain<T> {
        &self.values
    }

    pub fn into_bounded_domain(self) -> BoundedDomain<T> {
        self.values
    }

    pub fn map<U, F>(&self, label: &'static str, f: F) -> ExprDomain<U>
    where
        F: FnMut(&T) -> U,
    {
        ExprDomain {
            label,
            values: self.values.map(f),
        }
    }

    pub fn flat_map<U, F>(&self, label: &'static str, f: F) -> ExprDomain<U>
    where
        F: FnMut(&T) -> BoundedDomain<U>,
    {
        ExprDomain {
            label,
            values: self.values.flat_map(f),
        }
    }

    pub fn product<U>(&self, label: &'static str, other: &ExprDomain<U>) -> ExprDomain<(T, U)>
    where
        T: Clone,
        U: Clone,
    {
        ExprDomain {
            label,
            values: self.values.product(&other.values),
        }
    }

    pub fn filter<F>(&self, label: &'static str, predicate: F) -> Self
    where
        T: Clone,
        F: FnMut(&T) -> bool,
    {
        ExprDomain {
            label,
            values: self.values.filter(predicate),
        }
    }

    pub fn unique(self) -> Self
    where
        T: PartialEq,
    {
        Self {
            label: self.label,
            values: self.values.unique(),
        }
    }
}

impl<T, const N: usize> From<[T; N]> for BoundedDomain<T> {
    fn from(values: [T; N]) -> Self {
        Self::new(values.into_iter().collect())
    }
}

pub trait IntoBoundedDomain<T> {
    fn into_bounded_domain(self) -> BoundedDomain<T>;
}

impl<T> IntoBoundedDomain<T> for BoundedDomain<T> {
    fn into_bounded_domain(self) -> BoundedDomain<T> {
        self
    }
}

impl<T> IntoBoundedDomain<T> for Vec<T> {
    fn into_bounded_domain(self) -> BoundedDomain<T> {
        BoundedDomain::new(self)
    }
}

impl<T, const N: usize> IntoBoundedDomain<T> for [T; N] {
    fn into_bounded_domain(self) -> BoundedDomain<T> {
        BoundedDomain::from(self)
    }
}

pub fn into_bounded_domain<T, D>(values: D) -> BoundedDomain<T>
where
    D: IntoBoundedDomain<T>,
{
    values.into_bounded_domain()
}

pub fn bounded_vec_domain<T>(min_len: usize, max_len: usize) -> BoundedDomain<Vec<T>>
where
    T: Signature,
{
    let element_domain = T::bounded_domain().into_vec();
    let mut values = Vec::new();
    for len in min_len..=max_len {
        enumerate_vecs(
            &element_domain,
            len,
            &mut Vec::with_capacity(len),
            &mut values,
        );
    }
    BoundedDomain::new(values)
}

fn enumerate_vecs<T: Clone>(
    domain: &[T],
    remaining: usize,
    current: &mut Vec<T>,
    values: &mut Vec<Vec<T>>,
) {
    if remaining == 0 {
        values.push(current.clone());
        return;
    }
    for value in domain {
        current.push(value.clone());
        enumerate_vecs(domain, remaining - 1, current, values);
        current.pop();
    }
}

pub trait Signature: Sized + Clone + Debug + Eq + 'static {
    fn bounded_domain() -> BoundedDomain<Self>;

    fn invariant(&self) -> bool {
        true
    }
}

impl Signature for bool {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![false, true])
    }
}

impl<T> Signature for Option<T>
where
    T: Signature,
{
    fn bounded_domain() -> BoundedDomain<Self> {
        let mut values = Vec::with_capacity(T::bounded_domain().len() + 1);
        values.push(None);
        values.extend(T::bounded_domain().into_vec().into_iter().map(Some));
        BoundedDomain::new(values)
    }

    fn invariant(&self) -> bool {
        self.as_ref().is_none_or(Signature::invariant)
    }
}

pub struct OpaqueModelValue<Tag, const N: usize> {
    index: usize,
    _tag: PhantomData<Tag>,
}

impl<Tag, const N: usize> OpaqueModelValue<Tag, N> {
    pub const fn new(index: usize) -> Option<Self> {
        if index < N {
            Some(Self {
                index,
                _tag: PhantomData,
            })
        } else {
            None
        }
    }

    pub const fn index(self) -> usize {
        self.index
    }
}

impl<Tag, const N: usize> fmt::Debug for OpaqueModelValue<Tag, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OpaqueModelValue<{}, {}>({})",
            std::any::type_name::<Tag>(),
            N,
            self.index
        )
    }
}

impl<Tag, const N: usize> Clone for OpaqueModelValue<Tag, N> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Tag, const N: usize> Copy for OpaqueModelValue<Tag, N> {}

impl<Tag, const N: usize> PartialEq for OpaqueModelValue<Tag, N> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl<Tag, const N: usize> Eq for OpaqueModelValue<Tag, N> {}

impl<Tag, const N: usize> PartialOrd for OpaqueModelValue<Tag, N> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<Tag, const N: usize> Ord for OpaqueModelValue<Tag, N> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.index.cmp(&other.index)
    }
}

impl<Tag, const N: usize> std::hash::Hash for OpaqueModelValue<Tag, N> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index.hash(state);
    }
}

impl<Tag: 'static, const N: usize> Signature for OpaqueModelValue<Tag, N> {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(
            (0..N)
                .map(|index| Self {
                    index,
                    _tag: PhantomData,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedDomain, ExprDomain, Signature};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Atom {
        A,
        B,
    }

    impl Signature for Atom {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::A, Self::B])
        }
    }

    #[test]
    fn expr_domain_combinators_preserve_label_and_values() {
        let atoms = ExprDomain::of_signature("atoms");
        let flags = ExprDomain::new("flags", [false, true]);
        let pairs = atoms.product("atom_flag_pairs", &flags);
        let filtered = pairs.filter("only_true", |(_, flag)| *flag).unique();
        let duplicated =
            atoms.flat_map("duplicated", |atom| BoundedDomain::new(vec![*atom, *atom]));
        let mapped = atoms.map("labels", |atom| match atom {
            Atom::A => "a",
            Atom::B => "b",
        });

        assert_eq!(atoms.label(), "atoms");
        assert_eq!(pairs.label(), "atom_flag_pairs");
        assert_eq!(filtered.label(), "only_true");
        assert_eq!(
            filtered.into_bounded_domain().into_vec(),
            vec![(Atom::A, true), (Atom::B, true)]
        );
        assert_eq!(duplicated.label(), "duplicated");
        assert_eq!(
            duplicated.into_bounded_domain().into_vec(),
            vec![Atom::A, Atom::A, Atom::B, Atom::B]
        );
        assert_eq!(mapped.label(), "labels");
        assert_eq!(mapped.into_bounded_domain().into_vec(), vec!["a", "b"]);
    }
}
