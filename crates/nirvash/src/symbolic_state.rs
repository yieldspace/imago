use std::{any::type_name, fmt, sync::Arc};

use crate::Signature;

type ReadRef<S, T> = dyn for<'a> Fn(&'a S) -> &'a T + Send + Sync + 'static;
type WriteValue<S, T> = dyn Fn(&mut S, T) + Send + Sync + 'static;

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
        T: Signature,
    {
        Self::Finite {
            type_name: type_name::<T>(),
            domain_size: T::bounded_domain().len(),
        }
    }

    pub fn composite<T>(fields: Vec<SymbolicSortField>) -> Self
    where
        T: Signature,
    {
        Self::Composite {
            type_name: type_name::<T>(),
            domain_size: T::bounded_domain().len(),
            fields,
        }
    }

    pub fn option<T>() -> Self
    where
        Option<T>: Signature,
        T: SymbolicSortSpec,
    {
        Self::Option {
            type_name: type_name::<Option<T>>(),
            domain_size: <Option<T> as Signature>::bounded_domain().len(),
            inner: Box::new(T::symbolic_sort()),
        }
    }

    pub fn rel_set<T>() -> Self
    where
        crate::RelSet<T>: Signature,
        T: SymbolicSortSpec,
    {
        Self::RelSet {
            type_name: type_name::<crate::RelSet<T>>(),
            domain_size: <crate::RelSet<T> as Signature>::bounded_domain().len(),
            element: Box::new(T::symbolic_sort()),
        }
    }

    pub fn relation2<A, B>() -> Self
    where
        crate::Relation2<A, B>: Signature,
        A: SymbolicSortSpec,
        B: SymbolicSortSpec,
    {
        Self::Relation2 {
            type_name: type_name::<crate::Relation2<A, B>>(),
            domain_size: <crate::Relation2<A, B> as Signature>::bounded_domain().len(),
            left: Box::new(A::symbolic_sort()),
            right: Box::new(B::symbolic_sort()),
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

pub trait SymbolicSortSpec: Signature {
    fn symbolic_sort() -> SymbolicSort;
}

impl SymbolicSortSpec for bool {
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::finite::<Self>()
    }
}

impl<T> SymbolicSortSpec for Option<T>
where
    T: SymbolicSortSpec,
    Option<T>: Signature,
{
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::option::<T>()
    }
}

impl<T> SymbolicSortSpec for crate::RelSet<T>
where
    T: SymbolicSortSpec,
    crate::RelSet<T>: Signature,
{
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::rel_set::<T>()
    }
}

impl<A, B> SymbolicSortSpec for crate::Relation2<A, B>
where
    A: SymbolicSortSpec,
    B: SymbolicSortSpec,
    crate::Relation2<A, B>: Signature,
{
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::relation2::<A, B>()
    }
}

#[derive(Clone)]
pub struct SymbolicStateField<S> {
    path: String,
    sort: SymbolicSort,
    read_index: Arc<dyn Fn(&S) -> usize + Send + Sync + 'static>,
    write_index: Arc<dyn Fn(&mut S, usize) + Send + Sync + 'static>,
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
    seed: Arc<dyn Fn() -> S + Send + Sync + 'static>,
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

pub trait SymbolicStateSpec: Signature {
    fn symbolic_state_schema() -> SymbolicStateSchema<Self>;
}

fn is_symbolic_path_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_lowercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub fn normalize_symbolic_state_path<'a>(path: &'a str) -> Option<&'a str> {
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
    T: Signature + Clone,
{
    symbolic_leaf_value::<T>(0)
}

pub fn symbolic_leaf_value<T>(index: usize) -> T
where
    T: Signature + Clone,
{
    let domain = T::bounded_domain().into_vec();
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
    T: Signature + Eq,
{
    let domain = T::bounded_domain().into_vec();
    domain
        .iter()
        .position(|candidate| candidate == value)
        .unwrap_or_else(|| {
            panic!(
                "symbolic state value {:?} is not in the bounded domain of {}",
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
    T: SymbolicSortSpec + Clone + Eq + 'static,
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
    T: SymbolicSortSpec + Clone + Eq + 'static,
    R: for<'a> Fn(&'a S) -> &'a T + Send + Sync + 'static,
    W: Fn(&mut S, T) + Send + Sync + 'static,
    S: 'static,
{
    let read = Arc::new(read) as Arc<ReadRef<S, T>>;
    let write = Arc::new(write) as Arc<WriteValue<S, T>>;
    if let Some(schema) = crate::registry::lookup_symbolic_state_schema::<T>() {
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
