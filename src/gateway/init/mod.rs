//! Gateway initialization helpers.
//!
//! Each module exposes async `init_*` functions that construct a subsystem
//! used by [`crate::gateway::Gateway::new`]. The functions are grouped by
//! domain to keep the monolithic constructor readable.

pub mod agents;
pub mod devices;
pub mod pipelines;
pub mod security;
pub mod services;
pub mod storage;
pub mod tools;
