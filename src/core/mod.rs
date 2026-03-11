//! Core business logic module
//!
//! This module contains the domain models and core engine logic.
//! It is independent of external adapters and frameworks.

pub mod engine;
pub mod models;

pub use engine::Engine;
pub use models::*;
