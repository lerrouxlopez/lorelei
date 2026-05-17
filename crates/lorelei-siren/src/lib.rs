//! Safety policy ("The Siren").
//!
//! Deterministic checks must run before any LLM-based policy evaluation.

#![forbid(unsafe_code)]

pub mod policy;
