//! Channel Lifecycle Management with Auto-Restart
//!
//! Provides automatic restart with exponential backoff for channel failures.

use crate::channels::Channel;
use crate::error::{MantaError, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::{error, info, warn};

/// Channel lifecycle status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    /// Channel is starting up
    Starting,
    /// Channel is running normally
    Running,
    /// Channel is stopping
    Stopping,
    /// Channel has stopped
    Stopped,
    /// Channel has crashed
    Crashed,
    /// Channel is restarting
    Restarting,
}

impl std::fmt::Display for ChannelStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelStatus::Starting => write!(f, "starting"),
            ChannelStatus::Running => write!(f, "running"),
            ChannelStatus::Stopping => write!(f, "stopping"),
            ChannelStatus::Stopped => write!(f, "stopped"),
            ChannelStatus::Crashed => write!(f, "crashed"),
            ChannelStatus::Restarting => write!(f, "restarting"),
        }
    }
}

/// Restart policy configuration
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    /// Maximum number of restarts before giving up (default: 10)
    pub max_restarts: u32,
    /// Initial delay before first restart (default: 5s)
    pub initial_delay: Duration,
    /// Maximum delay between restarts (default: 5min)
    pub max_delay: Duration,
    /// Backoff multiplier (default: 2.0)
    pub backoff_factor: f32,
    /// Reset restart counter after this duration of success (default: 5min)
    pub reset_after: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 10,
            initial_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(300), // 5 minutes
            backoff_factor: 2.0,
            reset_after: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Internal lifecycle state
#[derive(Debug)]
pub struct LifecycleState {
    /// Number of restart attempts
    pub restart_count: u32,
    /// When the channel last restarted
    pub last_restart: Option<Instant>,
    /// When the channel last reported success
    pub last_success: Option<Instant>,
    /// Current channel status
    pub status: ChannelStatus,
    /// Whether the channel should stop (shutdown signal)
    pub should_stop: bool,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self {
            restart_count: 0,
            last_restart: None,
            last_success: None,
            status: ChannelStatus::Stopped,
            should_stop: false,
        }
    }
}

/// Manages the lifecycle of a channel with automatic restart
pub struct ChannelLifecycle {
    /// The channel being managed
    channel: Arc<dyn Channel>,
    /// Restart configuration
    policy: RestartPolicy,
    /// Mutable lifecycle state
    state: RwLock<LifecycleState>,
    /// Channel name for logging
    name: String,
}

impl ChannelLifecycle {
    /// Create a new channel lifecycle manager
    pub fn new(channel: Arc<dyn Channel>, policy: RestartPolicy) -> Self {
        let name = channel.name().to_string();
        Self {
            channel,
            policy,
            state: RwLock::new(LifecycleState::default()),
            name,
        }
    }

    /// Create with default restart policy
    pub fn with_defaults(channel: Arc<dyn Channel>) -> Self {
        Self::new(channel, RestartPolicy::default())
    }

    /// Get the current status
    pub async fn status(&self) -> ChannelStatus {
        self.state.read().await.status
    }

    /// Get the restart count
    pub async fn restart_count(&self) -> u32 {
        self.state.read().await.restart_count
    }

    /// Start the managed channel (blocking, runs until shutdown)
    pub async fn start_managed(&self) -> Result<()> {
        info!("Starting managed channel: {}", self.name);

        loop {
            // Check if we should stop
            if self.state.read().await.should_stop {
                info!("Channel {} received stop signal", self.name);
                self.set_status(ChannelStatus::Stopped).await;
                break;
            }

            // Update status to starting
            self.set_status(ChannelStatus::Starting).await;

            // Try to start the channel
            match self.channel.start().await {
                Ok(()) => {
                    info!("Channel {} started successfully", self.name);
                    self.set_status(ChannelStatus::Running).await;
                    self.record_success().await;

                    // Wait for the channel to stop (or crash)
                    self.wait_for_stop().await;

                    // Check if we should restart
                    if !self.should_restart().await {
                        info!("Channel {} not restarting (stop requested or max restarts)", self.name);
                        break;
                    }

                    // Calculate backoff and restart
                    let delay = self.calculate_backoff().await;
                    info!(
                        "Channel {} restarting in {:?} (attempt {}/{})",
                        self.name,
                        delay,
                        self.state.read().await.restart_count + 1,
                        self.policy.max_restarts
                    );
                    self.set_status(ChannelStatus::Restarting).await;
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    error!("Channel {} failed to start: {}", self.name, e);
                    self.set_status(ChannelStatus::Crashed).await;

                    if !self.should_restart().await {
                        return Err(e);
                    }

                    let delay = self.calculate_backoff().await;
                    warn!(
                        "Channel {} will retry in {:?} (attempt {}/{})",
                        self.name,
                        delay,
                        self.state.read().await.restart_count + 1,
                        self.policy.max_restarts
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }

        Ok(())
    }

    /// Request graceful shutdown
    pub async fn shutdown(&self) {
        info!("Requesting shutdown for channel: {}", self.name);
        self.state.write().await.should_stop = true;
        self.set_status(ChannelStatus::Stopping).await;

        // Try to stop the channel
        if let Err(e) = self.channel.stop().await {
            error!("Error stopping channel {}: {}", self.name, e);
        }
    }

    /// Wait for the channel to stop
    async fn wait_for_stop(&self) {
        // Poll health check until it fails or stop is requested
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Check if stop was requested
            if self.state.read().await.should_stop {
                break;
            }

            // Check if channel is still healthy
            match self.channel.health_check().await {
                Ok(true) => {
                    // Channel is healthy, update success timestamp
                    self.record_success().await;
                }
                Ok(false) => {
                    warn!("Channel {} health check failed", self.name);
                    break;
                }
                Err(e) => {
                    error!("Channel {} health check error: {}", self.name, e);
                    break;
                }
            }
        }
    }

    /// Check if we should restart the channel
    async fn should_restart(&self) -> bool {
        let state = self.state.read().await;

        // Don't restart if stop was requested
        if state.should_stop {
            return false;
        }

        // Check if we've exceeded max restarts
        if state.restart_count >= self.policy.max_restarts {
            error!(
                "Channel {} exceeded max restarts ({})",
                self.name, self.policy.max_restarts
            );
            return false;
        }

        true
    }

    /// Calculate backoff delay with jitter
    async fn calculate_backoff(&self) -> Duration {
        let mut state = self.state.write().await;
        state.restart_count += 1;
        state.last_restart = Some(Instant::now());

        let attempts = state.restart_count.min(5); // Cap at 5 for calculation
        drop(state);

        let delay_ms = (self.policy.initial_delay.as_millis() as f32
            * self.policy.backoff_factor.powi(attempts as i32)) as u64;

        let delay = Duration::from_millis(
            delay_ms.min(self.policy.max_delay.as_millis() as u64)
        );

        // Add jitter (±10%)
        let jitter = delay.as_millis() as f32 * 0.1;
        let jitter_ms = rand::random::<f32>() * jitter * 2.0 - jitter;

        delay + Duration::from_millis(jitter_ms.max(0.0) as u64)
    }

    /// Update the status
    async fn set_status(&self, status: ChannelStatus) {
        self.state.write().await.status = status;
    }

    /// Record a successful operation (resets restart count after threshold)
    async fn record_success(&self) {
        let mut state = self.state.write().await;
        state.last_success = Some(Instant::now());

        // Reset restart count if enough time has passed
        if let Some(last_restart) = state.last_restart {
            if last_restart.elapsed() > self.policy.reset_after {
                if state.restart_count > 0 {
                    info!(
                        "Channel {} restart count reset after {}s of stability",
                        self.name,
                        self.policy.reset_after.as_secs()
                    );
                    state.restart_count = 0;
                }
            }
        }
    }
}

/// Collection of managed channels
pub struct LifecycleManager {
    /// Managed channels by name
    channels: RwLock<Arc<HashMap<String, Arc<ChannelLifecycle>>>>,
}

use std::collections::HashMap;

impl LifecycleManager {
    /// Create a new lifecycle manager
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(Arc::new(HashMap::new())),
        }
    }

    /// Add a channel to be managed
    pub async fn add_channel(
        &self,
        name: impl Into<String>,
        channel: Arc<dyn Channel>,
        policy: Option<RestartPolicy>,
    ) {
        let name = name.into();
        let lifecycle = Arc::new(ChannelLifecycle::new(
            channel,
            policy.unwrap_or_default(),
        ));

        let mut channels = self.channels.write().await;
        let mut new_channels = HashMap::clone(&channels);
        new_channels.insert(name, lifecycle);
        *channels = Arc::new(new_channels);
    }

    /// Remove a channel from management
    pub async fn remove_channel(&self, name: &str) -> Result<()> {
        // Shutdown the channel first
        if let Some(lifecycle) = self.get_lifecycle(name).await {
            lifecycle.shutdown().await;
        }

        // Remove from managed channels
        let mut channels = self.channels.write().await;
        let mut new_channels = HashMap::clone(&channels);
        new_channels.remove(name);
        *channels = Arc::new(new_channels);

        Ok(())
    }

    /// Get a channel's lifecycle
    pub async fn get_lifecycle(&self, name: &str) -> Option<Arc<ChannelLifecycle>> {
        self.channels.read().await.get(name).cloned()
    }

    /// Get status of all managed channels
    pub async fn get_all_status(&self) -> Vec<(String, ChannelStatus, u32)> {
        let channels = self.channels.read().await;
        let mut results = Vec::new();

        for (name, lifecycle) in channels.iter() {
            results.push((
                name.clone(),
                lifecycle.status().await,
                lifecycle.restart_count().await,
            ));
        }

        results
    }

    /// Shutdown all managed channels
    pub async fn shutdown_all(&self) {
        let channels = self.channels.read().await.clone();

        for (name, lifecycle) in channels.iter() {
            info!("Shutting down channel: {}", name);
            lifecycle.shutdown().await;
        }

        // Wait for all channels to stop
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::core::models::Id;

    struct MockChannel {
        name: String,
        should_fail_start: RwLock<bool>,
        running: RwLock<bool>,
    }

    impl MockChannel {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                should_fail_start: RwLock::new(false),
                running: RwLock::new(false),
            }
        }

        async fn set_should_fail_start(&self, value: bool) {
            *self.should_fail_start.write().await = value;
        }
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            &self.name
        }

        fn capabilities(&self) -> crate::channels::ChannelCapabilities {
            crate::channels::ChannelCapabilities::default()
        }

        async fn start(&self) -> Result<()> {
            if *self.should_fail_start.read().await {
                return Err(MantaError::Internal("Start failed".to_string()));
            }
            *self.running.write().await = true;
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            *self.running.write().await = false;
            Ok(())
        }

        async fn send(&self, _message: crate::channels::OutgoingMessage) -> Result<Id> {
            Ok(Id::new())
        }

        async fn send_typing(&self, _conversation_id: &crate::channels::ConversationId) -> Result<()> {
            Ok(())
        }

        async fn edit_message(&self, _message_id: Id, _new_content: String) -> Result<()> {
            Ok(())
        }

        async fn delete_message(&self, _message_id: Id) -> Result<()> {
            Ok(())
        }

        async fn health_check(&self) -> Result<bool> {
            Ok(*self.running.read().await)
        }
    }

    #[tokio::test]
    async fn test_lifecycle_status_transitions() {
        let channel = Arc::new(MockChannel::new("test"));
        let lifecycle = ChannelLifecycle::with_defaults(channel);

        assert_eq!(lifecycle.status().await, ChannelStatus::Stopped);

        // Note: We can't test start_managed without actually running it
        // which would block. Unit tests focus on state management.
    }

    #[test]
    fn test_restart_policy_default() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.max_restarts, 10);
        assert_eq!(policy.initial_delay, Duration::from_secs(5));
        assert_eq!(policy.max_delay, Duration::from_secs(300));
        assert_eq!(policy.backoff_factor, 2.0);
    }

    #[tokio::test]
    async fn test_lifecycle_manager_add_remove() {
        let manager = LifecycleManager::new();
        let channel = Arc::new(MockChannel::new("test-channel"));

        manager.add_channel("test-channel", channel.clone(), None).await;

        let lifecycle = manager.get_lifecycle("test-channel").await;
        assert!(lifecycle.is_some());

        // Remove and verify
        manager.remove_channel("test-channel").await.unwrap();
        let lifecycle = manager.get_lifecycle("test-channel").await;
        assert!(lifecycle.is_none());
    }

    #[tokio::test]
    async fn test_channel_status_display() {
        assert_eq!(ChannelStatus::Running.to_string(), "running");
        assert_eq!(ChannelStatus::Crashed.to_string(), "crashed");
        assert_eq!(ChannelStatus::Restarting.to_string(), "restarting");
    }
}
