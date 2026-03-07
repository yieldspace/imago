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
