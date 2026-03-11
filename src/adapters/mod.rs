//! External adapters module
//!
//! This module contains adapters for external services and infrastructure.
//! Adapters translate between the core domain and external concerns.

pub mod api;
pub mod storage;

pub use api::ApiClient;
pub use storage::{Storage, StorageError};
