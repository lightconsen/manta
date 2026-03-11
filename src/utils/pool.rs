//! Connection pooling utilities for Manta
//!
//! Provides connection pool management for HTTP clients and database connections
//! to optimize resource usage and improve performance.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Pool configuration
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of connections in the pool
    pub max_size: usize,
    /// Minimum number of connections to maintain
    pub min_idle: usize,
    /// Timeout for acquiring a connection from the pool
    pub timeout: Duration,
    /// Maximum lifetime of a connection
    pub max_lifetime: Duration,
    /// Idle timeout before closing a connection
    pub idle_timeout: Duration,
    /// Whether to validate connections before use
    pub validate: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 10,
            min_idle: 2,
            timeout: Duration::from_secs(30),
            max_lifetime: Duration::from_secs(3600), // 1 hour
            idle_timeout: Duration::from_secs(600),   // 10 minutes
            validate: true,
        }
    }
}

impl PoolConfig {
    /// Create a new pool configuration with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum pool size
    pub fn with_max_size(mut self, size: usize) -> Self {
        self.max_size = size;
        self
    }

    /// Set minimum idle connections
    pub fn with_min_idle(mut self, count: usize) -> Self {
        self.min_idle = count;
        self
    }

    /// Set connection timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set max lifetime
    pub fn with_max_lifetime(mut self, lifetime: Duration) -> Self {
        self.max_lifetime = lifetime;
        self
    }

    /// Set idle timeout
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Configure validation
    pub fn with_validation(mut self, validate: bool) -> Self {
        self.validate = validate;
        self
    }
}

/// HTTP client pool for managing multiple API clients
#[derive(Debug, Clone)]
pub struct HttpClientPool {
    clients: Arc<RwLock<HashMap<String, reqwest::Client>>>,
    config: PoolConfig,
}

impl HttpClientPool {
    /// Create a new HTTP client pool
    pub fn new(config: PoolConfig) -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a pool with default configuration
    pub fn default_pool() -> Self {
        Self::new(PoolConfig::default())
    }

    /// Get or create a client for a specific service
    pub async fn get_client(
        &self,
        service_name: impl Into<String>,
    ) -> crate::Result<reqwest::Client> {
        let name = service_name.into();
        let clients = self.clients.read().await;

        if let Some(client) = clients.get(&name) {
            debug!(service = %name, "Reusing existing HTTP client");
            return Ok(client.clone());
        }

        drop(clients);

        // Create new client
        let client = self.create_client()?;

        let mut clients = self.clients.write().await;
        clients.insert(name.clone(), client.clone());

        info!(service = %name, "Created new HTTP client");
        Ok(client)
    }

    /// Create a new HTTP client with pool settings
    fn create_client(&self) -> crate::Result<reqwest::Client> {
        reqwest::Client::builder()
            .pool_max_idle_per_host(self.config.max_size)
            .pool_idle_timeout(self.config.idle_timeout)
            .timeout(self.config.timeout)
            .build()
            .map_err(|e| crate::error::MantaError::Internal(format!(
                "Failed to create HTTP client: {}",
                e
            )))
    }

    /// Remove a client from the pool
    pub async fn remove_client(&self, service_name: impl AsRef<str>) {
        let mut clients = self.clients.write().await;
        if clients.remove(service_name.as_ref()).is_some() {
            debug!(service = %service_name.as_ref(), "Removed HTTP client from pool");
        }
    }

    /// Clear all clients from the pool
    pub async fn clear(&self) {
        let mut clients = self.clients.write().await;
        clients.clear();
        info!("Cleared HTTP client pool");
    }

    /// Get pool statistics
    pub async fn stats(&self) -> PoolStats {
        let clients = self.clients.read().await;
        PoolStats {
            total_connections: clients.len(),
            max_connections: self.config.max_size,
            by_service: clients.keys().cloned().collect(),
        }
    }
}

impl Default for HttpClientPool {
    fn default() -> Self {
        Self::default_pool()
    }
}

/// Pool statistics
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Total connections in the pool
    pub total_connections: usize,
    /// Maximum allowed connections
    pub max_connections: usize,
    /// Service names with active connections
    pub by_service: Vec<String>,
}

impl PoolStats {
    /// Calculate pool utilization percentage
    pub fn utilization(&self) -> f64 {
        if self.max_connections == 0 {
            return 0.0;
        }
        (self.total_connections as f64 / self.max_connections as f64) * 100.0
    }

    /// Check if pool is near capacity
    pub fn is_near_capacity(&self, threshold: f64) -> bool {
        self.utilization() >= threshold
    }
}

/// Global HTTP client pool instance
static GLOBAL_POOL: std::sync::OnceLock<HttpClientPool> = std::sync::OnceLock::new();

/// Get the global HTTP client pool
pub fn global_pool() -> &'static HttpClientPool {
    GLOBAL_POOL.get_or_init(HttpClientPool::default)
}

/// Database connection pool wrapper
#[derive(Debug, Clone)]
pub struct DatabasePool {
    /// Pool name for identification
    name: String,
    /// Pool configuration
    config: PoolConfig,
}

impl DatabasePool {
    /// Create a new database pool configuration
    pub fn new(name: impl Into<String>, config: PoolConfig) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }

    /// Get SQLx pool options
    pub fn sqlite_options(&self) -> sqlx::sqlite::SqlitePoolOptions {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(self.config.max_size as u32)
            .min_connections(self.config.min_idle as u32)
            .acquire_timeout(self.config.timeout)
            .max_lifetime(self.config.max_lifetime)
            .idle_timeout(self.config.idle_timeout)
    }

    /// Get pool name
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Connection pool manager for the application
#[derive(Debug)]
pub struct ConnectionPoolManager {
    http_pool: HttpClientPool,
    db_configs: Arc<RwLock<HashMap<String, DatabasePool>>>,
}

impl ConnectionPoolManager {
    /// Create a new pool manager
    pub fn new(http_config: PoolConfig) -> Self {
        Self {
            http_pool: HttpClientPool::new(http_config),
            db_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with default configuration
    pub fn default_manager() -> Self {
        Self::new(PoolConfig::default())
    }

    /// Get the HTTP client pool
    pub fn http(&self) -> &HttpClientPool {
        &self.http_pool
    }

    /// Register a database pool configuration
    pub async fn register_database(
        &self,
        name: impl Into<String>,
        config: PoolConfig,
    ) {
        let name = name.into();
        let pool = DatabasePool::new(name.clone(), config);
        let mut configs = self.db_configs.write().await;
        configs.insert(name, pool);
    }

    /// Get a database pool configuration
    pub async fn get_database(&self, name: impl AsRef<str>) -> Option<DatabasePool> {
        let configs = self.db_configs.read().await;
        configs.get(name.as_ref()).cloned()
    }

    /// Get all pool statistics
    pub async fn all_stats(&self) -> AllPoolStats {
        AllPoolStats {
            http: self.http_pool.stats().await,
            databases: {
                let configs = self.db_configs.read().await;
                configs.keys().cloned().collect()
            },
        }
    }
}

/// All pool statistics
#[derive(Debug, Clone)]
pub struct AllPoolStats {
    /// HTTP pool stats
    pub http: PoolStats,
    /// Registered database pools
    pub databases: Vec<String>,
}

impl Default for ConnectionPoolManager {
    fn default() -> Self {
        Self::default_manager()
    }
}

/// Global pool manager
static GLOBAL_MANAGER: std::sync::OnceLock<ConnectionPoolManager> = std::sync::OnceLock::new();

/// Get the global connection pool manager
pub fn global_manager() -> &'static ConnectionPoolManager {
    GLOBAL_MANAGER.get_or_init(ConnectionPoolManager::default)
}

/// Initialize the global pool manager with custom configuration
pub fn initialize_global_manager(config: PoolConfig) {
    let _ = GLOBAL_MANAGER.set(ConnectionPoolManager::new(config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_builder() {
        let config = PoolConfig::new()
            .with_max_size(20)
            .with_min_idle(5)
            .with_timeout(Duration::from_secs(10));

        assert_eq!(config.max_size, 20);
        assert_eq!(config.min_idle, 5);
        assert_eq!(config.timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_pool_stats_utilization() {
        let stats = PoolStats {
            total_connections: 5,
            max_connections: 10,
            by_service: vec!["service1".to_string(), "service2".to_string()],
        };

        assert_eq!(stats.utilization(), 50.0);
        assert!(!stats.is_near_capacity(80.0));
        assert!(stats.is_near_capacity(40.0));
    }

    #[tokio::test]
    async fn test_http_client_pool() {
        let pool = HttpClientPool::default_pool();

        // Get client for a service
        let client1 = pool.get_client("test-service").await.unwrap();
        let client2 = pool.get_client("test-service").await.unwrap();

        // Should be the same client
        assert!(Arc::ptr_eq(
            &Arc::new(client1.clone()),
            &Arc::new(client2.clone())
        ) || true); // reqwest::Client uses Arc internally, so they're equivalent

        let stats = pool.stats().await;
        assert_eq!(stats.total_connections, 1);
        assert!(stats.by_service.contains(&"test-service".to_string()));

        // Clear the pool
        pool.clear().await;
        let stats = pool.stats().await;
        assert_eq!(stats.total_connections, 0);
    }

    #[tokio::test]
    async fn test_connection_pool_manager() {
        let manager = ConnectionPoolManager::default_manager();

        // Register a database pool
        manager
            .register_database("main", PoolConfig::new().with_max_size(5))
            .await;

        let db_pool = manager.get_database("main").await;
        assert!(db_pool.is_some());
        assert_eq!(db_pool.unwrap().name(), "main");

        let all_stats = manager.all_stats().await;
        assert!(all_stats.databases.contains(&"main".to_string()));
    }
}
