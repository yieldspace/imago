use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum PrupError {
    #[error("config validation failed: {0}")]
    ConfigValidation(String),
}
