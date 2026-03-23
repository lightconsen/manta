//! Backward-compatibility shim for `SqliteMemoryStore`.
//!
//! `SqliteMemoryStore` is now a type alias for `DatabaseStore`, which provides
//! the full WAL + FTS5 + access-tracking implementation.  All existing call
//! sites continue to work without changes.

pub use super::db::DatabaseStore as SqliteMemoryStore;
