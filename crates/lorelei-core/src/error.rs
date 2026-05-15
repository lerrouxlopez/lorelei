#![forbid(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LoreleiError {
    #[error("validation error for {field}: {message}")]
    Validation {
        field: &'static str,
        message: String,
    },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("shell error: {0}")]
    Shell(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl LoreleiError {
    pub fn validation(field: &'static str, message: impl Into<String>) -> Self {
        Self::Validation {
            field,
            message: message.into(),
        }
    }
}
