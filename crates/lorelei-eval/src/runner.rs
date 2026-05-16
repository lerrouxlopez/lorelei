#![forbid(unsafe_code)]

use lorelei_core::error::LoreleiError;

/// Placeholder for a future eval runner that could execute golden suites.
pub struct EvalRunner;

impl EvalRunner {
    pub fn new() -> Self {
        Self
    }

    pub fn validate(&self) -> Result<(), LoreleiError> {
        Ok(())
    }
}

impl Default for EvalRunner {
    fn default() -> Self {
        Self::new()
    }
}
