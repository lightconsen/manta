# Adapters Module

External service adapters for storage and API clients.

## Design

- **`api.rs`** — `ApiClient` for making HTTP requests to external services with retry and timeout support.
- **`storage.rs`** — Storage abstraction layer with multiple backends:
  - `Storage` trait — Unified interface for key-value and structured storage
  - `InMemoryStorage` — Ephemeral HashMap-based storage for testing
  - `FileStorage` — Persistent file-based storage
  - `SqliteStorage` — SQLite-backed storage with connection pooling

## Key Types

```rust
pub trait Storage: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn set(&self, key: &str, value: &str) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<bool>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
}

pub struct ApiClient {
    client: reqwest::Client,
    base_url: String,
    timeout: Duration,
    retry_config: RetryConfig,
}

pub enum StorageError {
    NotFound(String),
    ConnectionError(String),
    SerializationError(String),
}
```

## Implemented Features

- Unified `Storage` trait with async operations
- In-memory, file, and SQLite storage backends
- HTTP API client with configurable timeout and retry logic
- Storage error types with structured context

