use std::{any::type_name, fmt, marker::PhantomData, sync::Arc};

pub use inventory;

type ReadRef<S, T> = dyn for<'a> Fn(&'a S) -> &'a T + Send + Sync + 'static;
type WriteValue<S, T> = dyn Fn(&mut S, T) + Send + Sync + 'static;
type ReadIndex<S> = dyn Fn(&S) -> usize + Send + Sync + 'static;
type WriteIndex<S> = dyn Fn(&mut S, usize) + Send + Sync + 'static;
type SeedState<S> = dyn Fn() -> S + Send + Sync + 'static;

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

impl<T, const N: usize> From<[T; N]> for BoundedDomain<T> {
    fn from(values: [T; N]) -> Self {
        Self::new(values.into_iter().collect())
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

    pub fn of_finite_model_domain(label: &'static str) -> Self
    where
        T: FiniteModelDomain,
    {
        Self {
            label,
            values: T::finite_domain(),
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

pub trait FiniteModelDomain: Sized + Clone + fmt::Debug + Eq + 'static {
    fn finite_domain() -> BoundedDomain<Self>;

    fn bounded_domain() -> BoundedDomain<Self> {
        Self::finite_domain()
    }

    fn value_invariant(&self) -> bool {
        true
    }
}

impl FiniteModelDomain for bool {
    fn finite_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![false, true])
    }
}

impl<T> FiniteModelDomain for Option<T>
where
    T: FiniteModelDomain,
{
    fn finite_domain() -> BoundedDomain<Self> {
        let mut values = Vec::with_capacity(T::finite_domain().len() + 1);
        values.push(None);
        values.extend(T::finite_domain().into_vec().into_iter().map(Some));
        BoundedDomain::new(values)
    }

    fn value_invariant(&self) -> bool {
        self.as_ref().is_none_or(FiniteModelDomain::value_invariant)
    }
}

pub fn bounded_vec_domain<T>(min_len: usize, max_len: usize) -> BoundedDomain<Vec<T>>
where
    T: FiniteModelDomain,
{
    let element_domain = T::finite_domain().into_vec();
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

impl<Tag: 'static, const N: usize> FiniteModelDomain for OpaqueModelValue<Tag, N> {
    fn finite_domain() -> BoundedDomain<Self> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolicSortField {
    name: String,
    sort: SymbolicSort,
}

impl SymbolicSortField {
    pub fn new(name: impl Into<String>, sort: SymbolicSort) -> Self {
        Self {
            name: name.into(),
            sort,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn sort(&self) -> &SymbolicSort {
        &self.sort
    }

    pub const fn domain_size(&self) -> usize {
        self.sort.domain_size()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolicSort {
    Finite {
        type_name: &'static str,
        domain_size: usize,
    },
    Composite {
        type_name: &'static str,
        domain_size: usize,
        fields: Vec<SymbolicSortField>,
    },
    Option {
        type_name: &'static str,
        domain_size: usize,
        inner: Box<SymbolicSort>,
    },
    RelSet {
        type_name: &'static str,
        domain_size: usize,
        element: Box<SymbolicSort>,
    },
    Relation2 {
        type_name: &'static str,
        domain_size: usize,
        left: Box<SymbolicSort>,
        right: Box<SymbolicSort>,
    },
}

impl SymbolicSort {
    pub fn finite<T>() -> Self
    where
        T: FiniteModelDomain,
    {
        Self::Finite {
            type_name: type_name::<T>(),
            domain_size: T::finite_domain().len(),
        }
    }

    pub fn composite<T>(fields: Vec<SymbolicSortField>) -> Self {
        Self::Composite {
            type_name: type_name::<T>(),
            domain_size: saturating_product_domain_size(
                fields.iter().map(SymbolicSortField::domain_size),
            ),
            fields,
        }
    }

    pub fn option<T>() -> Self
    where
        Option<T>: FiniteModelDomain,
        T: SymbolicEncoding,
    {
        let inner = T::symbolic_sort();
        Self::Option {
            type_name: type_name::<Option<T>>(),
            domain_size: inner.domain_size().saturating_add(1),
            inner: Box::new(inner),
        }
    }

    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Finite { type_name, .. }
            | Self::Composite { type_name, .. }
            | Self::Option { type_name, .. }
            | Self::RelSet { type_name, .. }
            | Self::Relation2 { type_name, .. } => type_name,
        }
    }

    pub const fn domain_size(&self) -> usize {
        match self {
            Self::Finite { domain_size, .. }
            | Self::Composite { domain_size, .. }
            | Self::Option { domain_size, .. }
            | Self::RelSet { domain_size, .. }
            | Self::Relation2 { domain_size, .. } => *domain_size,
        }
    }

    pub fn fields(&self) -> &[SymbolicSortField] {
        match self {
            Self::Composite { fields, .. } => fields,
            _ => &[],
        }
    }
}

fn saturating_product_domain_size<I>(sizes: I) -> usize
where
    I: IntoIterator<Item = usize>,
{
    sizes
        .into_iter()
        .fold(1usize, |acc, size| acc.saturating_mul(size.max(1)))
}

#[derive(Clone)]
pub struct SymbolicStateField<S> {
    path: String,
    sort: SymbolicSort,
    read_index: Arc<ReadIndex<S>>,
    write_index: Arc<WriteIndex<S>>,
}

impl<S> SymbolicStateField<S> {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn sort(&self) -> &SymbolicSort {
        &self.sort
    }

    pub const fn type_name(&self) -> &'static str {
        self.sort.type_name()
    }

    pub const fn domain_size(&self) -> usize {
        self.sort.domain_size()
    }

    pub fn read_index(&self, state: &S) -> usize {
        (self.read_index)(state)
    }

    pub fn write_index(&self, state: &mut S, index: usize) {
        (self.write_index)(state, index);
    }
}

impl<S> fmt::Debug for SymbolicStateField<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SymbolicStateField")
            .field("path", &self.path)
            .field("sort", &self.sort)
            .finish()
    }
}

#[derive(Clone)]
pub struct SymbolicStateSchema<S> {
    fields: Vec<SymbolicStateField<S>>,
    seed: Arc<SeedState<S>>,
}

impl<S> SymbolicStateSchema<S> {
    pub fn new(
        fields: Vec<SymbolicStateField<S>>,
        seed: impl Fn() -> S + Send + Sync + 'static,
    ) -> Self {
        Self {
            fields,
            seed: Arc::new(seed),
        }
    }

    pub fn fields(&self) -> &[SymbolicStateField<S>] {
        &self.fields
    }

    pub fn seed_state(&self) -> S {
        (self.seed)()
    }

    pub fn field(&self, path: &str) -> Option<&SymbolicStateField<S>> {
        self.fields.iter().find(|field| field.path() == path)
    }

    pub fn has_path(&self, path: &str) -> bool {
        self.field(path).is_some()
    }

    pub fn read_indices(&self, state: &S) -> Vec<usize> {
        self.fields
            .iter()
            .map(|field| field.read_index(state))
            .collect()
    }

    pub fn rebuild_from_indices(&self, indices: &[usize]) -> S {
        assert_eq!(
            indices.len(),
            self.fields.len(),
            "symbolic state rebuild expected {} indices, got {}",
            self.fields.len(),
            indices.len()
        );
        let mut state = self.seed_state();
        for (field, index) in self.fields.iter().zip(indices.iter().copied()) {
            field.write_index(&mut state, index);
        }
        state
    }

    pub fn nested_fields<P>(
        &self,
        prefix: &str,
        read_parent: Arc<ReadRef<P, S>>,
        write_parent: Arc<WriteValue<P, S>>,
    ) -> Vec<SymbolicStateField<P>>
    where
        S: Clone + 'static,
        P: 'static,
    {
        self.fields
            .iter()
            .cloned()
            .map(|field| field.rebind(prefix, read_parent.clone(), write_parent.clone()))
            .collect()
    }
}

impl<S> fmt::Debug for SymbolicStateSchema<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SymbolicStateSchema")
            .field("fields", &self.fields)
            .finish()
    }
}

pub trait SymbolicEncoding {
    fn symbolic_sort() -> SymbolicSort;

    fn symbolic_state_schema() -> Option<SymbolicStateSchema<Self>>
    where
        Self: Sized,
    {
        None
    }
}

impl SymbolicEncoding for bool {
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::finite::<Self>()
    }
}

impl<T> SymbolicEncoding for Option<T>
where
    T: SymbolicEncoding,
    Option<T>: FiniteModelDomain,
{
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::option::<T>()
    }
}

pub struct RegisteredSymbolicStateSchema {
    pub state_type_id: fn() -> std::any::TypeId,
    pub build: fn() -> Box<dyn std::any::Any>,
}

inventory::collect!(RegisteredSymbolicStateSchema);

pub fn lookup_symbolic_state_schema<S>() -> Option<SymbolicStateSchema<S>>
where
    S: 'static,
{
    let state_type_id = std::any::TypeId::of::<S>();
    let mut matched = inventory::iter::<RegisteredSymbolicStateSchema>
        .into_iter()
        .filter(|entry| (entry.state_type_id)() == state_type_id);
    let entry = matched.next()?;
    assert!(
        matched.next().is_none(),
        "duplicate symbolic state schema registration for `{}`",
        type_name::<S>()
    );
    let value = (entry.build)();
    Some(
        *value
            .downcast::<SymbolicStateSchema<S>>()
            .unwrap_or_else(|_| {
                panic!(
                    "registered symbolic state schema for `{}` has an unexpected type",
                    type_name::<S>()
                )
            }),
    )
}

fn is_symbolic_path_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_lowercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub fn normalize_symbolic_state_path(path: &str) -> Option<&str> {
    if matches!(path, "self" | "state" | "prev" | "next" | "action") {
        return None;
    }
    if path.strip_prefix("action.").is_some() {
        return None;
    }
    for prefix in ["state.", "prev.", "next.", "self."] {
        if let Some(stripped) = path.strip_prefix(prefix) {
            return (!stripped.is_empty()).then_some(stripped);
        }
    }
    if path.contains("::")
        || path.contains('(')
        || path.contains(')')
        || path.contains(' ')
        || path.contains('!')
        || path.contains('&')
        || path.contains('|')
        || path.contains('+')
        || path.contains('=')
        || path.contains('<')
        || path.contains('>')
        || path.contains(',')
        || path.contains('[')
        || path.contains(']')
        || path.contains('{')
        || path.contains('}')
    {
        return None;
    }
    path.split('.')
        .all(is_symbolic_path_segment)
        .then_some(path)
}

pub fn symbolic_seed_value<T>() -> T
where
    T: FiniteModelDomain + Clone,
{
    symbolic_leaf_value::<T>(0)
}

pub fn symbolic_leaf_value<T>(index: usize) -> T
where
    T: FiniteModelDomain + Clone,
{
    let domain = T::finite_domain().into_vec();
    domain.get(index).cloned().unwrap_or_else(|| {
        panic!(
            "symbolic state index {index} out of bounds for {} (domain size {})",
            type_name::<T>(),
            domain.len()
        )
    })
}

pub fn symbolic_leaf_index<T>(value: &T) -> usize
where
    T: FiniteModelDomain + Eq,
{
    let domain = T::finite_domain().into_vec();
    domain
        .iter()
        .position(|candidate| candidate == value)
        .unwrap_or_else(|| {
            panic!(
                "symbolic state value {:?} is not in the finite domain of {}",
                value,
                type_name::<T>()
            )
        })
}

pub fn symbolic_leaf_field<S, T, R, W>(
    path: impl Into<String>,
    read: R,
    write: W,
) -> SymbolicStateField<S>
where
    T: SymbolicEncoding + FiniteModelDomain + Clone + Eq + 'static,
    R: for<'a> Fn(&'a S) -> &'a T + Send + Sync + 'static,
    W: Fn(&mut S, T) + Send + Sync + 'static,
{
    SymbolicStateField {
        path: path.into(),
        sort: T::symbolic_sort(),
        read_index: Arc::new(move |state| symbolic_leaf_index::<T>(read(state))),
        write_index: Arc::new(move |state, index| write(state, symbolic_leaf_value::<T>(index))),
    }
}

pub fn symbolic_state_fields<S, T, R, W>(
    path: &'static str,
    read: R,
    write: W,
) -> Vec<SymbolicStateField<S>>
where
    T: SymbolicEncoding + FiniteModelDomain + Clone + Eq + 'static,
    R: for<'a> Fn(&'a S) -> &'a T + Send + Sync + 'static,
    W: Fn(&mut S, T) + Send + Sync + 'static,
    S: 'static,
{
    let read = Arc::new(read) as Arc<ReadRef<S, T>>;
    let write = Arc::new(write) as Arc<WriteValue<S, T>>;
    if let Some(schema) = lookup_symbolic_state_schema::<T>() {
        return schema.nested_fields(path, read, write);
    }
    vec![symbolic_leaf_field(
        path,
        move |state| read(state),
        move |state, value| write(state, value),
    )]
}

impl<S> SymbolicStateField<S> {
    fn rebind<P>(
        self,
        prefix: &str,
        read_parent: Arc<ReadRef<P, S>>,
        write_parent: Arc<WriteValue<P, S>>,
    ) -> SymbolicStateField<P>
    where
        S: Clone + 'static,
        P: 'static,
    {
        let path = if prefix.is_empty() || prefix == "self" {
            self.path.clone()
        } else if self.path == "self" {
            prefix.to_owned()
        } else {
            format!("{prefix}.{}", self.path)
        };
        let read_index = self.read_index.clone();
        let write_index = self.write_index.clone();
        let read_parent_for_read = read_parent.clone();
        let read_parent_for_write = read_parent.clone();
        SymbolicStateField {
            path,
            sort: self.sort,
            read_index: Arc::new(move |state| {
                let child = read_parent_for_read(state);
                read_index(child)
            }),
            write_index: Arc::new(move |state, index| {
                let mut child = read_parent_for_write(state).clone();
                write_index(&mut child, index);
                write_parent(state, child);
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoundedDomain, ExprDomain, FiniteModelDomain, SymbolicEncoding, SymbolicSort,
        SymbolicSortField,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Atom {
        A,
        B,
    }

    impl FiniteModelDomain for Atom {
        fn finite_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::A, Self::B])
        }
    }

    impl SymbolicEncoding for Atom {
        fn symbolic_sort() -> SymbolicSort {
            SymbolicSort::finite::<Self>()
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct PanicDomain;

    impl FiniteModelDomain for PanicDomain {
        fn finite_domain() -> BoundedDomain<Self> {
            panic!("composite/option symbolic_sort must not enumerate finite_domain")
        }
    }

    impl SymbolicEncoding for PanicDomain {
        fn symbolic_sort() -> SymbolicSort {
            SymbolicSort::Finite {
                type_name: std::any::type_name::<Self>(),
                domain_size: 7,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CompositeDomain;

    impl FiniteModelDomain for CompositeDomain {
        fn finite_domain() -> BoundedDomain<Self> {
            panic!("composite symbolic_sort must not enumerate finite_domain")
        }
    }

    impl SymbolicEncoding for CompositeDomain {
        fn symbolic_sort() -> SymbolicSort {
            SymbolicSort::composite::<Self>(vec![
                SymbolicSortField::new("atom", SymbolicSort::finite::<Atom>()),
                SymbolicSortField::new("flag", SymbolicSort::finite::<bool>()),
            ])
        }
    }

    #[test]
    fn expr_domain_combinators_preserve_label_and_values() {
        let atoms = ExprDomain::of_finite_model_domain("atoms");
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

    #[test]
    fn composite_symbolic_sort_uses_field_sizes_without_enumerating_domain() {
        let sort = CompositeDomain::symbolic_sort();
        assert_eq!(sort.domain_size(), 4);
    }

    #[test]
    fn option_symbolic_sort_uses_inner_size_without_enumerating_domain() {
        let sort = SymbolicSort::option::<PanicDomain>();
        assert_eq!(sort.domain_size(), 8);
    }
}
