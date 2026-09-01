//! Core business logic module
//!
//! This module contains the domain models and core engine logic.
//! It is independent of external adapters and frameworks.
// INVARIANTS-NONE: this directory HOSTS the registry itself (core/invariants.rs); nothing here needs its own registration.

pub mod context;
pub mod invariants;
pub mod models;

pub use models::*;
