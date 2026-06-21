//! Plugin Metrics and Resource Monitoring
//!
//! Provides per-plugin runtime metrics using lock-free atomics for
//! lightweight instrumentation of tool calls, HTTP requests, and memory usage.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;

/// Per-plugin metrics collected at runtime.
///
/// All counters use relaxed ordering for maximum performance.
/// Snapshots are taken for export / reporting.
pub struct PluginMetrics {
    pub tool_calls: AtomicU64,
    pub tool_errors: AtomicU64,
    pub http_requests: AtomicU64,
    pub http_errors: AtomicU64,
    pub memory_usage_bytes: AtomicU64,
    pub last_error: std::sync::Mutex<Option<String>>,
    pub last_active: std::sync::Mutex<Instant>,
    pub cpu_duration_ns: AtomicU64,
}

impl PluginMetrics {
    /// Create a new metrics instance with zeroed counters.
    pub fn new() -> Self {
        Self {
            tool_calls: AtomicU64::new(0),
            tool_errors: AtomicU64::new(0),
            http_requests: AtomicU64::new(0),
            http_errors: AtomicU64::new(0),
            memory_usage_bytes: AtomicU64::new(0),
            last_error: std::sync::Mutex::new(None),
            last_active: std::sync::Mutex::new(Instant::now()),
            cpu_duration_ns: AtomicU64::new(0),
        }
    }

    /// Record a tool call.
    pub fn record_tool_call(&self) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a tool error.
    pub fn record_tool_error(&self) {
        self.tool_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an HTTP request made by the plugin.
    pub fn record_http_request(&self) {
        self.http_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an HTTP error.
    pub fn record_http_error(&self) {
        self.http_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record memory usage in bytes.
    pub fn record_memory(&self, bytes: u64) {
        self.memory_usage_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Record CPU time spent (cumulative nanoseconds).
    pub fn record_cpu(&self, ns: u64) {
        self.cpu_duration_ns.fetch_add(ns, Ordering::Relaxed);
    }

    /// Set the last error message.
    pub fn set_last_error(&self, err: String) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = Some(err);
        }
    }

    /// Mark the plugin as recently active.
    pub fn touch(&self) {
        if let Ok(mut guard) = self.last_active.lock() {
            *guard = Instant::now();
        }
    }

    /// Take a snapshot of all counters for reporting / export.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let last_active_secs_ago = self
            .last_active
            .lock()
            .map(|guard| guard.elapsed().as_secs())
            .unwrap_or(0);

        let last_error = self.last_error.lock().ok().and_then(|guard| guard.clone());

        let cpu_duration_ns = self.cpu_duration_ns.load(Ordering::Relaxed);

        MetricsSnapshot {
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
            tool_errors: self.tool_errors.load(Ordering::Relaxed),
            http_requests: self.http_requests.load(Ordering::Relaxed),
            http_errors: self.http_errors.load(Ordering::Relaxed),
            memory_usage_bytes: self.memory_usage_bytes.load(Ordering::Relaxed),
            last_error,
            last_active_secs_ago,
            cpu_duration_secs: cpu_duration_ns as f64 / 1_000_000_000.0,
        }
    }
}

impl Default for PluginMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable snapshot of plugin metrics for serialization and export.
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub http_requests: u64,
    pub http_errors: u64,
    pub memory_usage_bytes: u64,
    pub last_error: Option<String>,
    pub last_active_secs_ago: u64,
    pub cpu_duration_secs: f64,
}

/// Thread-safe registry of per-plugin metrics.
pub struct PluginMetricsRegistry {
    metrics: Arc<tokio::sync::RwLock<HashMap<String, Arc<PluginMetrics>>>>,
}

impl PluginMetricsRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Register metrics for a plugin. If already registered, returns the
    /// existing entry.
    pub async fn register(&self, plugin_id: &str) -> Arc<PluginMetrics> {
        let mut map = self.metrics.write().await;
        map.entry(plugin_id.to_string())
            .or_insert_with(|| Arc::new(PluginMetrics::new()))
            .clone()
    }

    /// Unregister metrics for a plugin.
    pub async fn unregister(&self, plugin_id: &str) {
        let mut map = self.metrics.write().await;
        map.remove(plugin_id);
    }

    /// Get metrics for a plugin, if registered.
    pub async fn get(&self, plugin_id: &str) -> Option<Arc<PluginMetrics>> {
        let map = self.metrics.read().await;
        map.get(plugin_id).cloned()
    }

    /// Get snapshots for all registered plugins.
    pub async fn all_snapshots(&self) -> Vec<(String, MetricsSnapshot)> {
        let map = self.metrics.read().await;
        let mut results = Vec::with_capacity(map.len());
        for (id, m) in map.iter() {
            results.push((id.clone(), m.snapshot()));
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }

    /// Get the number of registered plugins.
    pub async fn len(&self) -> usize {
        self.metrics.read().await.len()
    }

    /// Check if the registry is empty.
    pub async fn is_empty(&self) -> bool {
        self.metrics.read().await.is_empty()
    }
}

impl Default for PluginMetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metrics_new() {
        let m = PluginMetrics::new();
        assert_eq!(m.tool_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_plugin_metrics_record_tool_call() {
        let m = PluginMetrics::new();
        m.record_tool_call();
        m.record_tool_call();
        assert_eq!(m.tool_calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_plugin_metrics_record_tool_error() {
        let m = PluginMetrics::new();
        m.record_tool_error();
        assert_eq!(m.tool_errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_plugin_metrics_record_http() {
        let m = PluginMetrics::new();
        m.record_http_request();
        m.record_http_error();
        assert_eq!(m.http_requests.load(Ordering::Relaxed), 1);
        assert_eq!(m.http_errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_plugin_metrics_memory() {
        let m = PluginMetrics::new();
        m.record_memory(42);
        assert_eq!(m.memory_usage_bytes.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn test_plugin_metrics_cpu() {
        let m = PluginMetrics::new();
        m.record_cpu(1_000_000);
        m.record_cpu(500_000);
        assert_eq!(m.cpu_duration_ns.load(Ordering::Relaxed), 1_500_000);
    }

    #[test]
    fn test_plugin_metrics_last_error() {
        let m = PluginMetrics::new();
        m.set_last_error("oops".to_string());
        let snap = m.snapshot();
        assert_eq!(snap.last_error, Some("oops".to_string()));
    }

    #[test]
    fn test_plugin_metrics_snapshot() {
        let m = PluginMetrics::new();
        m.record_tool_call();
        m.record_http_request();
        m.record_memory(1024);
        let snap = m.snapshot();
        assert_eq!(snap.tool_calls, 1);
        assert_eq!(snap.http_requests, 1);
        assert_eq!(snap.memory_usage_bytes, 1024);
    }

    #[tokio::test]
    async fn test_metrics_registry_register_get() {
        let reg = PluginMetricsRegistry::new();
        let m = reg.register("plugin-a").await;
        m.record_tool_call();

        let found = reg.get("plugin-a").await.unwrap();
        assert_eq!(found.tool_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_metrics_registry_unregister() {
        let reg = PluginMetricsRegistry::new();
        reg.register("plugin-a").await;
        reg.unregister("plugin-a").await;
        assert!(reg.get("plugin-a").await.is_none());
    }

    #[tokio::test]
    async fn test_metrics_registry_all_snapshots() {
        let reg = PluginMetricsRegistry::new();
        let m1 = reg.register("plugin-a").await;
        let m2 = reg.register("plugin-b").await;
        m1.record_tool_call();
        m2.record_http_request();

        let snapshots = reg.all_snapshots().await;
        assert_eq!(snapshots.len(), 2);
        // Results are sorted by id
        assert_eq!(snapshots[0].0, "plugin-a");
        assert_eq!(snapshots[0].1.tool_calls, 1);
        assert_eq!(snapshots[1].0, "plugin-b");
        assert_eq!(snapshots[1].1.http_requests, 1);
    }

    #[tokio::test]
    async fn test_metrics_registry_register_returns_existing() {
        let reg = PluginMetricsRegistry::new();
        let m1 = reg.register("plugin-a").await;
        m1.record_tool_call();
        let m2 = reg.register("plugin-a").await;
        assert_eq!(m2.tool_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_metrics_registry_len() {
        let reg = PluginMetricsRegistry::new();
        assert!(reg.is_empty().await);
        reg.register("a").await;
        assert_eq!(reg.len().await, 1);
        reg.register("b").await;
        assert_eq!(reg.len().await, 2);
    }
}
