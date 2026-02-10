use std::fmt;

use uuid::Uuid;

pub trait Validate {
    fn validate(&self) -> Result<(), ValidationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub field: &'static str,
    pub message: &'static str,
}

impl ValidationError {
    pub const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }

    pub const fn missing(field: &'static str) -> Self {
        Self::new(field, "missing required value")
    }

    pub const fn empty(field: &'static str) -> Self {
        Self::new(field, "must not be empty")
    }

    pub const fn invalid(field: &'static str, message: &'static str) -> Self {
        Self::new(field, message)
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid `{}`: {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

pub(crate) fn ensure_non_empty(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::empty(field));
    }

    Ok(())
}

pub(crate) fn ensure_uuid_not_nil(
    value: &Uuid,
    field: &'static str,
) -> Result<(), ValidationError> {
    if value.is_nil() {
        return Err(ValidationError::invalid(field, "must not be nil UUID"));
    }

    Ok(())
}

pub(crate) fn ensure_positive_u64(value: u64, field: &'static str) -> Result<(), ValidationError> {
    if value == 0 {
        return Err(ValidationError::invalid(field, "must be greater than zero"));
    }

    Ok(())
}
