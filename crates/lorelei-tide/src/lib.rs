//! Orchestration flow ("The Tide") scaffolding.
//!
//! Multi-agent orchestration is explicitly out of scope for v1; this crate is
//! reserved for future coordination and flow control primitives.

#![forbid(unsafe_code)]

pub mod config;
pub mod flow;
pub mod runtime;
