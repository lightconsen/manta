# Utils Module

Shared utility modules used across Syscity.

## Design

Provides cross-cutting concerns: request batching, structured logging, connection pooling, and performance profiling. Each sub-module is independently usable and has a global singleton accessor.

- **`batch`** — Request batching and deduplication
- **`logging`** — Tracing subscriber initialization with multiple formats
- **`pool`** — Connection pool management for HTTP clients and databases
- **`profiling`** — Performance timers, counters, and memory stats

## Key Types

```rust
pub struct BatchedRequest<I, O> {
    pub id: I,
    pub response_tx: oneshot::Sender<O>,
    pub queued_at: Instant,
}

#[async_trait]
pub trait BatchProcessor {
    type Input: Send + Clone;
    type Output: Send + Clone;
    type Error: Send + Clone + std::fmt::Debug;
    async fn process_batch(&self, inputs: Vec<Self::Input>) -> Result<Vec<Self::Output>, Self::Error>;
}

pub struct BatchConfig {
    pub max_batch_size: usize,
    pub max_wait: Duration,
    pub min_batch_size: usize,
}

pub struct Batcher<I, O> {
    config: BatchConfig,
    request_tx: mpsc::Sender<BatchedRequest<I, O>>,
}

pub struct Deduplicator<K, V> {
    pending: Arc<tokio::sync::Mutex<HashMap<K, Vec<oneshot::Sender<V>>>>>,
}

pub struct PoolConfig {
    pub max_size: usize,
    pub min_idle: usize,
    pub timeout: Duration,
    pub max_lifetime: Duration,
    pub idle_timeout: Duration,
    pub validate: bool,
}

pub struct HttpClientPool {
    clients: Arc<RwLock<HashMap<String, reqwest::Client>>>,
    config: PoolConfig,
}

pub struct ConnectionPoolManager {
    http_pool: HttpClientPool,
    db_configs: Arc<RwLock<HashMap<String, DatabasePool>>>,
}

pub struct Profiler {
    timers: Arc<RwLock<HashMap<String, Vec<Duration>>>>,
    counters: Arc<RwLock<HashMap<String, AtomicU64>>>,
    memory: Arc<RwLock<MemoryStats>>,
}

pub struct MemoryStats {
    pub peak_bytes: usize,
    pub current_bytes: usize,
    pub total_allocations: u64,
    pub allocation_history: Vec<AllocationRecord>,
}

pub struct TimerStats {
    pub name: String,
    pub count: usize,
    pub total: Duration,
    pub avg: Duration,
    pub min: Duration,
    pub max: Duration,
}

pub struct PerformanceReport {
    pub timers: Vec<TimerStats>,
    pub counters: HashMap<String, u64>,
    pub memory: MemoryStats,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
```

## Data Flow

```
Batcher
    │
    ├──▶ submit(id) → oneshot response channel
    │
    └──▶ run_batcher() loop
            │
            ├──▶ Buffer fills (max_batch_size) → process_batch()
            └──▶ Timeout (max_wait) → process_batch()

Deduplicator
    │
    ├──▶ try_start(key) → Some(PendingRequest) or None
    │
    └──▶ wait_for(key) → awaits completion from first request

ConnectionPoolManager
    │
    ├──▶ http_pool.get_client("service") → cached or new reqwest::Client
    └──▶ register_database(name, config) → SqlitePoolOptions

Profiler
    │
    ├──▶ start_timer("name") → TimerGuard (RAII drop records duration)
    ├──▶ increment_counter("name") → atomic +1
    ├──▶ record_allocation(size, desc) → memory stats + history
    └──▶ generate_report() → PerformanceReport
```

## Implemented Features

- Generic `Batcher` with configurable size/time triggers and `BatchProcessor` trait
- `FunctionBatchProcessor` for function-based batching without custom types
- `Deduplicator` for coalescing identical in-flight requests with multiple waiters
- `PendingRequest` RAII handle that cleans up on drop if not completed
- Structured logging initialization with JSON, pretty, and compact formats
- File and stdout log output with parent directory creation
- Log level parsing (case-insensitive) with hyper/reqwest noise filtering
- Panic hook that logs via tracing and prints to stderr with location
- HTTP client pool per service with reqwest `Client` reuse
- Database pool configuration wrapper for sqlx `SqlitePoolOptions`
- Global pool accessors (`global_pool`, `global_manager`) via `OnceLock`
- `Profiler` with named timers, counters, and memory allocation tracking
- `TimerGuard` RAII helper for automatic duration recording on scope exit
- Memory allocation/deallocation tracking with peak detection
- Bounded allocation history (1000 entries) for leak detection
- `PerformanceReport` with human-readable formatting
- `time_block!` and `count_event!` convenience macros
- Comprehensive unit tests for all sub-modules

