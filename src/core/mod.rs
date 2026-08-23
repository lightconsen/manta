//! Core business logic module
//!
//! This module contains the domain models and core engine logic.
//! It is independent of external adapters and frameworks.
// INVARIANTS-NONE: this directory HOSTS the registry itself (core/invariants.rs); nothing here needs its own registration.

pub mod context;
pub mod engine;
pub mod events;
pub mod invariants;
pub mod models;

pub use engine::Engine;
pub use engine::EngineMetrics;
pub use events::*;
pub use models::*;
