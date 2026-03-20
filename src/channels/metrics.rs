//! Channel Metrics Collection
//!
//! Provides metrics tracking for channels (messages, errors, latency).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Rolling window latency tracker
#[derive(Debug)]
pub struct LatencyWindow {
    /// Stored latencies in milliseconds
    latencies: RwLock<VecDeque<u64>>,
    /// Maximum number of samples
    capacity: usize,
}

impl LatencyWindow {
    /// Create a new latency window
    pub fn new(capacity: usize) -> Self {
        Self {
            latencies: RwLock::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Add a latency sample
    pub async fn record(&self, latency_ms: u64) {
        let mut queue = self.latencies.write().await;
        if queue.len() >= self.capacity {
            queue.pop_front();
        }
        queue.push_back(latency_ms);
    }

    /// Get average latency
    pub async fn average(&self) -> Option<u64> {
        let queue = self.latencies.read().await;
        if queue.is_empty() {
            None
        } else {
            Some(queue.iter().sum::<u64>() / queue.len() as u64)
        }
    }

    /// Get percentile latency (approximate)
    pub async fn percentile(&self, p: f64) -> Option<u64> {
        let mut queue: Vec<u64> = self.latencies.read().await.iter().copied().collect();
        if queue.is_empty() {
            return None;
        }

        queue.sort_unstable();
        let index = ((queue.len() as f64) * p / 100.0) as usize;
        Some(queue[index.min(queue.len() - 1)])
    }

    /// Get the number of samples
    pub async fn len(&self) -> usize {
        self.latencies.read().await.len()
    }

    /// Check if there are any samples
    pub async fn is_empty(&self) -> bool {
        self.latencies.read().await.is_empty()
    }

    /// Clear all samples
    pub async fn clear(&self) {
        self.latencies.write().await.clear();
    }
}

/// Metrics for a single channel
#[derive(Debug)]
pub struct ChannelMetrics {
    /// Total messages received
    pub messages_received: AtomicU64,
    /// Total messages sent
    pub messages_sent: AtomicU64,
    /// Total errors encountered
    pub errors: AtomicU64,
    /// Connection establishment count
    pub connections: AtomicU64,
    /// Disconnection count
    pub disconnections: AtomicU64,
    /// When the channel connected (if currently connected)
    pub connected_at: RwLock<Option<Instant>>,
    /// When metrics collection started
    pub started_at: Instant,
    /// Latency samples (last 100)
    pub latency: LatencyWindow,
    /// Bytes received
    pub bytes_received: AtomicU64,
    /// Bytes sent
    pub bytes_sent: AtomicU64,
}

impl ChannelMetrics {
    /// Create new metrics
    pub fn new() -> Self {
        Self {
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            connections: AtomicU64::new(0),
            disconnections: AtomicU64::new(0),
            connected_at: RwLock::new(None),
            started_at: Instant::now(),
            latency: LatencyWindow::new(100),
            bytes_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
        }
    }

    /// Record a message being received
    pub fn record_receive(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a message being sent with latency
    pub async fn record_send(&self, latency: Duration) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        self.latency.record(latency.as_millis() as u64).await;
    }

    /// Record a message being sent (without latency tracking)
    pub fn record_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an error
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record bytes received
    pub fn record_bytes_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record bytes sent
    pub fn record_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record connection established
    pub async fn record_connected(&self) {
        self.connections.fetch_add(1, Ordering::Relaxed);
        *self.connected_at.write().await = Some(Instant::now());
    }

    /// Record disconnection
    pub async fn record_disconnected(&self) {
        self.disconnections.fetch_add(1, Ordering::Relaxed);
        *self.connected_at.write().await = None;
    }

    /// Check if currently connected
    pub async fn is_connected(&self) -> bool {
        self.connected_at.read().await.is_some()
    }

    /// Get connection duration (if connected)
    pub async fn connection_duration(&self) -> Option<Duration> {
        self.connected_at.read().await.map(|t| t.elapsed())
    }

    /// Get uptime since metrics started
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Get average latency
    pub async fn average_latency(&self) -> Option<Duration> {
        self.latency.average().await.map(Duration::from_millis)
    }

    /// Get P95 latency
    pub async fn p95_latency(&self) -> Option<Duration> {
        self.latency
            .percentile(95.0)
            .await
            .map(Duration::from_millis)
    }

    /// Get P99 latency
    pub async fn p99_latency(&self) -> Option<Duration> {
        self.latency
            .percentile(99.0)
            .await
            .map(Duration::from_millis)
    }

    /// Get all metrics as a snapshot
    pub async fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            connections: self.connections.load(Ordering::Relaxed),
            disconnections: self.disconnections.load(Ordering::Relaxed),
            is_connected: self.is_connected().await,
            uptime_secs: self.uptime().as_secs(),
            avg_latency_ms: self.average_latency().await.map(|d| d.as_millis() as u64),
            p95_latency_ms: self.p95_latency().await.map(|d| d.as_millis() as u64),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
        }
    }
}

impl Default for ChannelMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable metrics snapshot
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricsSnapshot {
    pub messages_received: u64,
    pub messages_sent: u64,
    pub errors: u64,
    pub connections: u64,
    pub disconnections: u64,
    pub is_connected: bool,
    pub uptime_secs: u64,
    pub avg_latency_ms: Option<u64>,
    pub p95_latency_ms: Option<u64>,
    pub bytes_received: u64,
    pub bytes_sent: u64,
}

/// Manager for all channel metrics
#[derive(Debug)]
pub struct MetricsManager {
    metrics: Arc<RwLock<std::collections::HashMap<String, Arc<ChannelMetrics>>>>,
}

impl MetricsManager {
    /// Create a new metrics manager
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Register a channel for metrics collection
    pub async fn register(&self, channel_name: impl Into<String>) -> Arc<ChannelMetrics> {
        let name = channel_name.into();
        let metrics = Arc::new(ChannelMetrics::new());

        let mut map = self.metrics.write().await;
        map.insert(name, Arc::clone(&metrics));

        metrics
    }

    /// Get metrics for a channel
    pub async fn get(&self, channel_name: &str) -> Option<Arc<ChannelMetrics>> {
        self.metrics.read().await.get(channel_name).cloned()
    }

    /// Unregister a channel
    pub async fn unregister(&self, channel_name: &str) {
        self.metrics.write().await.remove(channel_name);
    }

    /// Get all metrics snapshots
    pub async fn get_all_snapshots(&self) -> std::collections::HashMap<String, MetricsSnapshot> {
        let metrics = self.metrics.read().await;
        let mut snapshots = std::collections::HashMap::new();

        for (name, metric) in metrics.iter() {
            snapshots.insert(name.clone(), metric.snapshot().await);
        }

        snapshots
    }

    /// Get aggregate metrics across all channels
    pub async fn get_aggregate(&self) -> MetricsSnapshot {
        let snapshots = self.get_all_snapshots().await;

        let mut total = MetricsSnapshot {
            messages_received: 0,
            messages_sent: 0,
            errors: 0,
            connections: 0,
            disconnections: 0,
            is_connected: false,
            uptime_secs: 0,
            avg_latency_ms: None,
            p95_latency_ms: None,
            bytes_received: 0,
            bytes_sent: 0,
        };

        for snapshot in snapshots.values() {
            total.messages_received += snapshot.messages_received;
            total.messages_sent += snapshot.messages_sent;
            total.errors += snapshot.errors;
            total.connections += snapshot.connections;
            total.disconnections += snapshot.disconnections;
            total.bytes_received += snapshot.bytes_received;
            total.bytes_sent += snapshot.bytes_sent;
        }

        total
    }
}

impl Default for MetricsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_latency_window() {
        let window = LatencyWindow::new(5);

        // Add samples
        for i in 1..=10 {
            window.record(i * 10).await;
        }

        // Should only keep last 5 (values: 60, 70, 80, 90, 100)
        assert_eq!(window.len().await, 5);

        // Average should be (60+70+80+90+100)/5 = 80
        assert_eq!(window.average().await, Some(80));

        // P50 (median) should be 80
        let p50 = window.percentile(50.0).await.unwrap();
        assert!(p50 >= 60 && p50 <= 100);
    }

    #[tokio::test]
    async fn test_channel_metrics_record() {
        let metrics = ChannelMetrics::new();

        metrics.record_receive();
        metrics.record_receive();
        metrics.record_sent();

        assert_eq!(metrics.messages_received.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.messages_sent.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_channel_metrics_latency() {
        let metrics = ChannelMetrics::new();

        metrics.record_send(Duration::from_millis(50)).await;
        metrics.record_send(Duration::from_millis(100)).await;
        metrics.record_send(Duration::from_millis(150)).await;

        let avg = metrics.average_latency().await;
        assert!(avg.is_some());
        assert!(avg.unwrap().as_millis() >= 50 && avg.unwrap().as_millis() <= 150);
    }

    #[tokio::test]
    async fn test_channel_metrics_connection() {
        let metrics = ChannelMetrics::new();

        assert!(!metrics.is_connected().await);
        assert!(metrics.connection_duration().await.is_none());

        metrics.record_connected().await;
        assert!(metrics.is_connected().await);
        assert!(metrics.connection_duration().await.is_some());

        // Small delay to ensure duration > 0
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(metrics.connection_duration().await.unwrap().as_millis() > 0);

        metrics.record_disconnected().await;
        assert!(!metrics.is_connected().await);
        assert!(metrics.connection_duration().await.is_none());
    }

    #[tokio::test]
    async fn test_metrics_manager() {
        let manager = MetricsManager::new();

        // Register channels
        let telegram = manager.register("telegram").await;
        let discord = manager.register("discord").await;

        // Record some activity
        telegram.record_receive();
        telegram.record_sent();
        discord.record_receive();

        // Get metrics
        let tel_metrics = manager.get("telegram").await.unwrap();
        assert_eq!(tel_metrics.messages_received.load(Ordering::Relaxed), 1);

        // Get all snapshots
        let snapshots = manager.get_all_snapshots().await;
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots.get("telegram").unwrap().messages_received, 1);
        assert_eq!(snapshots.get("discord").unwrap().messages_received, 1);

        // Get aggregate
        let aggregate = manager.get_aggregate().await;
        assert_eq!(aggregate.messages_received, 2);
        assert_eq!(aggregate.messages_sent, 1);
    }

    #[tokio::test]
    async fn test_metrics_snapshot() {
        let metrics = ChannelMetrics::new();

        metrics.record_receive();
        metrics.record_receive();
        metrics.record_sent();
        metrics.record_error();
        metrics.record_bytes_received(100);
        metrics.record_bytes_sent(50);

        let snapshot = metrics.snapshot().await;

        assert_eq!(snapshot.messages_received, 2);
        assert_eq!(snapshot.messages_sent, 1);
        assert_eq!(snapshot.errors, 1);
        assert_eq!(snapshot.bytes_received, 100);
        assert_eq!(snapshot.bytes_sent, 50);
    }
}
