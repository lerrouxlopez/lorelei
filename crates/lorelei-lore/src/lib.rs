//! Persistent memory ("The Lore") scaffolding.

#![forbid(unsafe_code)]

pub mod retrieval;
pub mod schema;
pub mod store;

pub mod echo;
pub mod embedding;
pub mod pg;
pub mod qdrant;
pub mod docs;
