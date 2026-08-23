//! External adapters module
//!
//! This module contains adapters for external services and infrastructure.
//! Adapters translate between the core domain and external concerns.
// INVARIANTS-NONE: stateless external-service drivers; all durable state lives behind the owning services.

pub mod api;
pub mod storage;

pub use api::ApiClient;
pub use storage::{FileStorage, InMemoryStorage, SqliteStorage, Storage, StorageError};
