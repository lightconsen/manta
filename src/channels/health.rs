//! Channel Health Monitoring
//!
//! Provides periodic health checks and staleness detection for channels.

use crate::error::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::{interval, Instant};
use tracing::{info, warn};

/// Health status of a channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Channel is healthy and responsive
    Healthy,
    /// Channel is responding but may be slow
    Degraded,
    /// Channel has failing health checks
    Unhealthy,
    /// Channel hasn't sent heartbeats recently (likely stale)
    Stale,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
            HealthStatus::Stale => write!(f, "stale"),
        }
    }
}

/// Health information for a specific channel
#[derive(Debug)]
pub struct ChannelHealth {
    /// Channel name
    pub channel_name: String,
    /// When the last heartbeat was received
    pub last_heartbeat: Instant,
    /// Number of consecutive failures
    pub consecutive_failures: u32,
    /// Current health status
    pub status: HealthStatus,
    /// Total message count received
    pub message_count: AtomicU64,
    /// When the last message was received
    pub last_message_at: Option<Instant>,
    /// Last latency measurement in ms
    pub last_latency_ms: u64,
}

impl ChannelHealth {
    /// Create new health tracking for a channel
    pub fn new(channel_name: impl Into<String>) -> Self {
        Self {
            channel_name: channel_name.into(),
            last_heartbeat: Instant::now(),
            consecutive_failures: 0,
            status: HealthStatus::Healthy,
            message_count: AtomicU64::new(0),
            last_message_at: None,
            last_latency_ms: 0,
        }
    }

    /// Record a heartbeat
    pub fn record_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
        self.consecutive_failures = 0;
        self.status = HealthStatus::Healthy;
    }

    /// Record a message being processed
    pub fn record_message(&mut self) {
        self.message_count.fetch_add(1, Ordering::Relaxed);
        self.last_message_at = Some(Instant::now());
    }

    /// Record a failure
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures > 3 {
            self.status = HealthStatus::Unhealthy;
        }
    }

    /// Update latency measurement
    pub fn record_latency(&mut self, latency: Duration) {
        self.last_latency_ms = latency.as_millis() as u64;
    }
}

/// Monitors health of all channels
pub struct ChannelHealthMonitor {
    /// Tracked channel health information
    channels: Arc<RwLock<HashMap<String, Arc<RwLock<ChannelHealth>>>>>,
    /// Check interval (how often to run health checks)
    check_interval: Duration,
    /// Threshold for considering a channel stale
    stale_threshold: Duration,
    /// Threshold for considering degraded (half of stale)
    degraded_threshold: Duration,
}

impl ChannelHealthMonitor {
    /// Create a new health monitor
    pub fn new(check_interval: Duration, stale_threshold: Duration) -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            check_interval,
            stale_threshold,
            degraded_threshold: stale_threshold / 2,
        }
    }

    /// Create with default settings (30s check, 2min stale threshold)
    pub fn with_defaults() -> Self {
        Self::new(Duration::from_secs(30), Duration::from_secs(120))
    }

    /// Register a channel for health monitoring
    pub async fn register_channel(&self, channel_name: impl Into<String>) {
        let name = channel_name.into();
        let health = Arc::new(RwLock::new(ChannelHealth::new(&name)));

        let mut channels = self.channels.write().await;
        channels.insert(name, health);
    }

    /// Unregister a channel
    pub async fn unregister_channel(&self, channel_name: &str) {
        let mut channels = self.channels.write().await;
        channels.remove(channel_name);
    }

    /// Record a heartbeat for a channel
    pub async fn record_heartbeat(&self, channel_name: &str) {
        let channels = self.channels.read().await;
        if let Some(health) = channels.get(channel_name) {
            health.write().await.record_heartbeat();
        }
    }

    /// Record a message for a channel
    pub async fn record_message(&self, channel_name: &str) {
        let channels = self.channels.read().await;
        if let Some(health) = channels.get(channel_name) {
            health.write().await.record_message();
        }
    }

    /// Record latency for a channel
    pub async fn record_latency(&self, channel_name: &str, latency: Duration) {
        let channels = self.channels.read().await;
        if let Some(health) = channels.get(channel_name) {
            health.write().await.record_latency(latency);
        }
    }

    /// Get health status for a specific channel
    pub async fn get_health(&self, channel_name: &str) -> Option<HealthStatus> {
        let channels = self.channels.read().await;
        if let Some(health) = channels.get(channel_name) {
            Some(health.read().await.status)
        } else {
            None
        }
    }

    /// Get all health information
    pub async fn get_all_health(&self) -> Vec<(String, HealthStatus, u64)> {
        let channels = self.channels.read().await;
        let mut results = Vec::new();

        for (name, health) in channels.iter() {
            let h = health.read().await;
            results.push((name.clone(), h.status, h.message_count.load(Ordering::Relaxed)));
        }

        results
    }

    /// Start the health monitoring background task
    pub async fn start_monitoring(&self) {
        let mut interval = interval(self.check_interval);
        let channels = Arc::clone(&self.channels);
        let stale_threshold = self.stale_threshold;
        let degraded_threshold = self.degraded_threshold;

        tokio::spawn(async move {
            loop {
                interval.tick().await;

                let channels_guard = channels.read().await;
                for (name, health) in channels_guard.iter() {
                    let mut h = health.write().await;
                    let elapsed = h.last_heartbeat.elapsed();

                    // Check staleness
                    if elapsed > stale_threshold {
                        if h.status != HealthStatus::Stale {
                            warn!("Channel {} is stale (no heartbeat for {:?})", name, elapsed);
                            h.status = HealthStatus::Stale;
                        }
                    } else if elapsed > degraded_threshold {
                        if h.status != HealthStatus::Degraded {
                            h.status = HealthStatus::Degraded;
                        }
                    } else if h.consecutive_failures == 0 && h.status != HealthStatus::Healthy {
                        h.status = HealthStatus::Healthy;
                    }
                }
            }
        });
    }

    /// Check if a channel is stale
    pub async fn is_stale(&self, channel_name: &str) -> bool {
        self.get_health(channel_name).await == Some(HealthStatus::Stale)
    }

    /// Check if a channel is healthy
    pub async fn is_healthy(&self, channel_name: &str) -> bool {
        self.get_health(channel_name).await == Some(HealthStatus::Healthy)
    }

    /// Get channels that need restart (stale or unhealthy)
    pub async fn get_channels_needing_restart(&self) -> Vec<String> {
        let channels = self.channels.read().await;
        let mut needs_restart = Vec::new();

        for (name, health) in channels.iter() {
            let h = health.read().await;
            match h.status {
                HealthStatus::Stale | HealthStatus::Unhealthy => {
                    needs_restart.push(name.clone());
                }
                _ => {}
            }
        }

        needs_restart
    }
}

impl Default for ChannelHealthMonitor {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Stale.to_string(), "stale");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
    }

    #[tokio::test]
    async fn test_channel_health_record_heartbeat() {
        let mut health = ChannelHealth::new("test");

        // Simulate time passing
        sleep(Duration::from_millis(10)).await;

        health.record_heartbeat();
        assert!(health.last_heartbeat.elapsed().as_millis() < 5);
        assert_eq!(health.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn test_channel_health_record_message() {
        let mut health = ChannelHealth::new("test");

        health.record_message();
        health.record_message();

        assert_eq!(health.message_count.load(Ordering::Relaxed), 2);
        assert!(health.last_message_at.is_some());
    }

    #[tokio::test]
    async fn test_channel_health_failures() {
        let mut health = ChannelHealth::new("test");

        health.record_failure();
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.status, HealthStatus::Healthy); // Still healthy

        health.record_failure();
        health.record_failure();
        health.record_failure();
        assert_eq!(health.consecutive_failures, 4);
        assert_eq!(health.status, HealthStatus::Unhealthy); // Now unhealthy
    }

    #[tokio::test]
    async fn test_health_monitor_register() {
        let monitor = ChannelHealthMonitor::with_defaults();

        monitor.register_channel("test-channel").await;

        let health = monitor.get_health("test-channel").await;
        assert!(health.is_some());
        assert_eq!(health.unwrap(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_health_monitor_record_heartbeat() {
        let monitor = ChannelHealthMonitor::with_defaults();

        monitor.register_channel("test").await;
        monitor.record_heartbeat("test").await;

        // Should still be healthy after heartbeat
        assert!(monitor.is_healthy("test").await);
    }

    #[tokio::test]
    async fn test_health_monitor_unregister() {
        let monitor = ChannelHealthMonitor::with_defaults();

        monitor.register_channel("test").await;
        assert!(monitor.get_health("test").await.is_some());

        monitor.unregister_channel("test").await;
        assert!(monitor.get_health("test").await.is_none());
    }
}
