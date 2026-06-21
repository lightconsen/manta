//! Cross-source observation streaming hub.
//!
//! [`PerceptionStreamHub`] fans observations from every streaming
//! [`PerceptionSource`] into a single broadcast channel that any number of
//! consumers (Gateway WebSockets, Agents, dashboards) can subscribe to without
//! caring which source produced what.
//!
//! # Lifecycle
//!
//! * Each streaming source's per-source `broadcast::Receiver` is read by a
//!   dedicated forwarder task that re-publishes onto the hub channel.
//! * The forwarder's [`JoinHandle`] is stored in `forwarders` so the hub can
//!   abort it when the source is detached (hot-unplug, config reload).
//! * Poll-only sources (no `subscribe()` impl) are silently ignored.
//!
//! Lagging consumers receive `RecvError::Lagged` and skip ahead — they don't
//! pause the hub or any other consumer.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::perception::{Observation, PerceptionRegistry, PerceptionSource};

/// Default channel capacity for the hub broadcast channel.
const DEFAULT_HUB_CAPACITY: usize = 1024;

/// Single fan-out point for streaming perception observations.
pub struct PerceptionStreamHub {
    tx: broadcast::Sender<Observation>,
    forwarders: AsyncMutex<HashMap<String, JoinHandle<()>>>,
}

impl PerceptionStreamHub {
    /// Create a hub with the given broadcast channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(16));
        Self {
            tx,
            forwarders: AsyncMutex::new(HashMap::new()),
        }
    }

    /// Default-capacity constructor (`{DEFAULT_HUB_CAPACITY}` slots).
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_HUB_CAPACITY)
    }

    /// Get a new subscriber for the merged observation stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Observation> {
        self.tx.subscribe()
    }

    /// Borrow the publishing handle so external producers (e.g. the
    /// poll-based [`PerceptionRegistry`]) can fan observations into the
    /// streaming pipeline alongside truly streaming sources.
    pub fn sender(&self) -> broadcast::Sender<Observation> {
        self.tx.clone()
    }

    /// Number of currently active subscribers (informational).
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Number of forwarder tasks currently running.
    pub async fn forwarder_count(&self) -> usize {
        self.forwarders.lock().await.len()
    }

    /// Attach a streaming source by name. Returns `true` if a forwarder task
    /// was spawned, `false` if the source does not implement `subscribe()`
    /// (poll-only) or is already attached.
    pub async fn attach_source(&self, name: &str, source: Arc<dyn PerceptionSource>) -> bool {
        let mut rx = match source.subscribe() {
            Some(r) => r,
            None => return false,
        };
        let mut forwarders = self.forwarders.lock().await;
        if forwarders.contains_key(name) {
            return false;
        }
        let tx = self.tx.clone();
        let name_owned = name.to_string();
        let handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(obs) => {
                        // Best-effort fan-out — if no consumers, send returns
                        // an error which we ignore (no work to do).
                        let _ = tx.send(obs);
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            "perception stream hub: source '{}' lagged, skipped {} observations",
                            name_owned,
                            skipped
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("perception stream hub: source '{}' closed", name_owned);
                        break;
                    }
                }
            }
        });
        forwarders.insert(name.to_string(), handle);
        true
    }

    /// Detach a single source by name; aborts its forwarder task.
    /// Returns `true` if a forwarder was found and aborted.
    pub async fn detach(&self, name: &str) -> bool {
        let mut forwarders = self.forwarders.lock().await;
        match forwarders.remove(name) {
            Some(handle) => {
                handle.abort();
                true
            }
            None => false,
        }
    }

    /// Detach every source whose name starts with `prefix`. Mirrors
    /// [`PerceptionRegistry::deregister_prefix`].
    pub async fn detach_prefix(&self, prefix: &str) -> usize {
        let mut forwarders = self.forwarders.lock().await;
        let to_remove: Vec<String> = forwarders
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        let mut removed = 0;
        for name in to_remove {
            if let Some(h) = forwarders.remove(&name) {
                h.abort();
                removed += 1;
            }
        }
        removed
    }

    /// Sync the hub with a [`PerceptionRegistry`] snapshot — attaches any
    /// sources that are in the registry but not yet forwarded, and detaches
    /// forwarders whose source has been removed.
    pub async fn sync_with_registry(&self, registry: &PerceptionRegistry) {
        let snapshot = registry.sources_snapshot().await;
        let snapshot_names: std::collections::HashSet<String> =
            snapshot.iter().map(|(n, _)| n.clone()).collect();

        // Detach forwarders whose source has disappeared.
        let stale: Vec<String> = {
            let forwarders = self.forwarders.lock().await;
            forwarders
                .keys()
                .filter(|k| !snapshot_names.contains(*k))
                .cloned()
                .collect()
        };
        for name in stale {
            self.detach(&name).await;
        }

        // Attach new streaming sources.
        for (name, src) in snapshot {
            self.attach_source(&name, src).await;
        }
    }

    /// Abort every running forwarder. Useful at shutdown.
    pub async fn shutdown(&self) {
        let mut forwarders = self.forwarders.lock().await;
        for (_, h) in forwarders.drain() {
            h.abort();
        }
    }
}

impl Default for PerceptionStreamHub {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

/// Spawn a background task that periodically calls
/// [`PerceptionStreamHub::sync_with_registry`] so that hot-plugged sources
/// are picked up automatically.
pub fn spawn_stream_hub_sync(
    hub: Arc<PerceptionStreamHub>,
    registry: Arc<PerceptionRegistry>,
    interval: std::time::Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Run once immediately to pick up sources registered before the
        // hub started.
        ticker.tick().await;
        hub.sync_with_registry(&registry).await;
        loop {
            ticker.tick().await;
            hub.sync_with_registry(&registry).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::perception::mock::MockPerceptionSource;
    use crate::perception::{AggregationStrategy, Modality};

    fn make_streaming_mock(
        name: &str,
    ) -> (Arc<MockPerceptionSource>, broadcast::Sender<Observation>) {
        let (mock, tx) = MockPerceptionSource::new(name).with_streaming(64);
        (Arc::new(mock), tx)
    }

    #[tokio::test]
    async fn test_hub_fans_in_streaming_source() {
        let hub = Arc::new(PerceptionStreamHub::new(64));
        let (mock, tx) = make_streaming_mock("cam");
        assert!(
            hub.attach_source("cam", mock as Arc<dyn PerceptionSource>)
                .await
        );
        assert_eq!(hub.forwarder_count().await, 1);

        let mut rx = hub.subscribe();
        let obs = Observation::new(
            "cam",
            Modality::Rgb,
            std::time::Instant::now(),
            1.0,
            serde_json::json!({"x": 1}),
        );
        tx.send(obs.clone()).unwrap();

        let received = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("hub did not deliver observation in time")
            .expect("hub channel closed");
        assert_eq!(received.source, "cam");
    }

    #[tokio::test]
    async fn test_hub_skips_poll_only_source() {
        let hub = PerceptionStreamHub::new(16);
        let mock: Arc<dyn PerceptionSource> = Arc::new(MockPerceptionSource::new("poll-only"));
        assert!(!hub.attach_source("poll-only", mock).await);
        assert_eq!(hub.forwarder_count().await, 0);
    }

    #[tokio::test]
    async fn test_hub_detach_aborts_forwarder() {
        let hub = PerceptionStreamHub::new(16);
        let (mock, _tx) = make_streaming_mock("cam");
        hub.attach_source("cam", mock as Arc<dyn PerceptionSource>)
            .await;
        assert!(hub.detach("cam").await);
        assert_eq!(hub.forwarder_count().await, 0);
    }

    #[tokio::test]
    async fn test_hub_attach_is_idempotent() {
        let hub = PerceptionStreamHub::new(16);
        let (mock, _tx) = make_streaming_mock("cam");
        let m: Arc<dyn PerceptionSource> = mock;
        assert!(hub.attach_source("cam", m.clone()).await);
        // Attaching again is a no-op
        assert!(!hub.attach_source("cam", m).await);
        assert_eq!(hub.forwarder_count().await, 1);
    }

    #[tokio::test]
    async fn test_hub_detach_prefix() {
        let hub = PerceptionStreamHub::new(16);
        let (m1, _) = make_streaming_mock("device:temp:1");
        let (m2, _) = make_streaming_mock("device:pressure:2");
        let (m3, _) = make_streaming_mock("camera");
        hub.attach_source("device:temp:1", m1 as Arc<dyn PerceptionSource>)
            .await;
        hub.attach_source("device:pressure:2", m2 as Arc<dyn PerceptionSource>)
            .await;
        hub.attach_source("camera", m3 as Arc<dyn PerceptionSource>)
            .await;

        let removed = hub.detach_prefix("device:").await;
        assert_eq!(removed, 2);
        assert_eq!(hub.forwarder_count().await, 1);
    }

    #[tokio::test]
    async fn test_hub_sync_with_registry_attaches_new_sources() {
        let registry = Arc::new(PerceptionRegistry::new(AggregationStrategy::Latest, 10));
        let (mock, _tx) = make_streaming_mock("streamer");
        registry
            .register_source(mock as Arc<dyn PerceptionSource>)
            .await;

        let hub = PerceptionStreamHub::new(16);
        hub.sync_with_registry(&registry).await;
        assert_eq!(hub.forwarder_count().await, 1);
    }

    #[tokio::test]
    async fn test_hub_sync_with_registry_detaches_removed_sources() {
        let registry = Arc::new(PerceptionRegistry::new(AggregationStrategy::Latest, 10));
        let (mock, _tx) = make_streaming_mock("streamer");
        registry
            .register_source(mock as Arc<dyn PerceptionSource>)
            .await;

        let hub = PerceptionStreamHub::new(16);
        hub.sync_with_registry(&registry).await;
        assert_eq!(hub.forwarder_count().await, 1);

        registry.deregister_source("streamer").await;
        hub.sync_with_registry(&registry).await;
        assert_eq!(hub.forwarder_count().await, 0);
    }

    #[tokio::test]
    async fn test_hub_multiple_subscribers_each_receive() {
        let hub = Arc::new(PerceptionStreamHub::new(64));
        let (mock, tx) = make_streaming_mock("multi");
        hub.attach_source("multi", mock as Arc<dyn PerceptionSource>)
            .await;

        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();

        let obs = Observation::new(
            "multi",
            Modality::System,
            std::time::Instant::now(),
            1.0,
            serde_json::json!({}),
        );
        tx.send(obs).unwrap();

        let r1 = tokio::time::timeout(Duration::from_millis(500), rx1.recv())
            .await
            .unwrap()
            .unwrap();
        let r2 = tokio::time::timeout(Duration::from_millis(500), rx2.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r1.source, "multi");
        assert_eq!(r2.source, "multi");
    }
}
