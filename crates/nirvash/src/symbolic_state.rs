use std::{any::type_name, fmt, sync::Arc};

use crate::Signature;

type ReadRef<S, T> = dyn for<'a> Fn(&'a S) -> &'a T + Send + Sync + 'static;
type WriteValue<S, T> = dyn Fn(&mut S, T) + Send + Sync + 'static;

#[derive(Clone)]
pub struct SymbolicStateField<S> {
    path: String,
    type_name: &'static str,
    domain_size: usize,
    read_index: Arc<dyn Fn(&S) -> usize + Send + Sync + 'static>,
    write_index: Arc<dyn Fn(&mut S, usize) + Send + Sync + 'static>,
}

impl<S> SymbolicStateField<S> {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn type_name(&self) -> &'static str {
        self.type_name
    }

    pub const fn domain_size(&self) -> usize {
        self.domain_size
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
            .field("type_name", &self.type_name)
            .field("domain_size", &self.domain_size)
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

pub fn normalize_symbolic_state_path(path: &str) -> Option<&str> {
    if path == "self" {
        return None;
    }
    for prefix in ["state.", "prev.", "next.", "self."] {
        if let Some(stripped) = path.strip_prefix(prefix) {
            return (!stripped.is_empty()).then_some(stripped);
        }
    }
    Some(path)
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
    T: Signature + Clone + Eq + 'static,
    R: for<'a> Fn(&'a S) -> &'a T + Send + Sync + 'static,
    W: Fn(&mut S, T) + Send + Sync + 'static,
{
    SymbolicStateField {
        path: path.into(),
        type_name: type_name::<T>(),
        domain_size: T::bounded_domain().len(),
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
    T: Signature + Clone + Eq + 'static,
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
            type_name: self.type_name,
            domain_size: self.domain_size,
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
