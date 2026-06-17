//! Cross-stage event broadcast hub.
//!
//! [`DerivedStreamHub`] sits **downstream** of the temporal/fusion stages
//! and carries [`Event`]s rather than raw [`super::Observation`]s.  It is
//! independent from [`super::PerceptionStreamHub`]:
//!
//! ```text
//!     Source ──► PerceptionStreamHub ──► TemporalProcessor ──┐
//!                                                            │
//!                              ┌─── DerivedStreamHub ◄───────┤
//!                              │                             │
//!                              │   FusionEngine ◄────────────┘
//!                              │       │
//!                              ▼       ▼
//!                      AttentionGate / SalienceFilter (per-agent)
//! ```
//!
//! Capacity is fixed at [`DEFAULT_DERIVED_HUB_CAPACITY`] (256). Event
//! streams are filtered/aggregated upstream so the per-second rate is
//! ~10–50 events, well under capacity.
//!
//! Unlike the raw hub, the derived hub does not own forwarder tasks —
//! the producers (TemporalProcessor, FusionStage) call
//! [`DerivedStreamHub::publish`] directly.

use tokio::sync::broadcast;

use crate::perception::Event;

/// Default broadcast capacity for the derived event hub.
///
/// Derived events are post-filter/aggregation so they're at least an
/// order of magnitude lower-rate than raw observations. 256 slots
/// gives slow consumers ~5–25 s of headroom before lagging.
pub const DEFAULT_DERIVED_HUB_CAPACITY: usize = 256;

/// Single broadcast channel for [`Event`]s produced by the temporal /
/// fusion stages.
pub struct DerivedStreamHub {
    tx: broadcast::Sender<Event>,
}

impl DerivedStreamHub {
    /// Create a hub with the given broadcast capacity (capped at a
    /// minimum of 16 to avoid pathological tiny buffers).
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(16));
        Self { tx }
    }

    /// Default-capacity constructor (`DEFAULT_DERIVED_HUB_CAPACITY` slots).
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_DERIVED_HUB_CAPACITY)
    }

    /// Subscribe to the merged event stream.
    ///
    /// Lagging consumers receive `RecvError::Lagged(n)` — they should
    /// log and skip ahead rather than treat it as fatal.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Publish a single event. Returns `true` if at least one
    /// subscriber received it; `false` if there were no live
    /// subscribers (the event is dropped — by design, not an error).
    pub fn publish(&self, event: Event) -> bool {
        self.tx.send(event).is_ok()
    }

    /// Number of currently active subscribers (informational).
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Underlying sender — exposed for callers that want to clone-and-
    /// move into background tasks.
    pub fn sender(&self) -> broadcast::Sender<Event> {
        self.tx.clone()
    }
}

impl Default for DerivedStreamHub {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{AnomalyKind, Modality};
    use std::time::{Duration, SystemTime};

    fn change_event(source: &str) -> Event {
        Event::Change {
            source: source.to_string(),
            modality: Modality::System,
            from: serde_json::json!(0),
            to: serde_json::json!(100),
            at: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn test_default_capacity_is_256() {
        let hub = DerivedStreamHub::default();
        // Default capacity is at least 256.
        let mut rx = hub.subscribe();
        // Push 200 events; default consumer should receive all of them.
        for i in 0..200 {
            assert!(hub.publish(change_event(&format!("s-{i}"))));
        }
        let mut received = 0;
        while let Ok(_e) = rx.try_recv() {
            received += 1;
        }
        assert_eq!(received, 200);
    }

    #[tokio::test]
    async fn test_publish_no_subscribers_returns_false() {
        let hub = DerivedStreamHub::new(16);
        // No subscribers — publish returns false.
        assert!(!hub.publish(change_event("orphan")));
    }

    #[tokio::test]
    async fn test_publish_delivers_to_multiple_subscribers() {
        let hub = DerivedStreamHub::new(64);
        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();
        assert_eq!(hub.receiver_count(), 2);

        let ev = change_event("multi");
        assert!(hub.publish(ev));

        let r1 = tokio::time::timeout(Duration::from_millis(100), rx1.recv())
            .await
            .unwrap()
            .unwrap();
        let r2 = tokio::time::timeout(Duration::from_millis(100), rx2.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r1.source(), Some("multi"));
        assert_eq!(r2.source(), Some("multi"));
    }

    #[tokio::test]
    async fn test_lagged_consumer_does_not_block_others() {
        let hub = DerivedStreamHub::new(16); // small capacity → easy to lag
        let mut slow = hub.subscribe();
        let mut fast = hub.subscribe();

        // Drain `fast` continuously in the background.
        let fast_handle = tokio::spawn(async move {
            let mut count = 0;
            loop {
                match fast.recv().await {
                    Ok(_) => count += 1,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return count,
                }
            }
        });

        // Push 64 events; capacity 16 → slow falls behind.
        for i in 0..64 {
            hub.publish(change_event(&format!("e-{i}")));
        }

        // Slow consumer reports Lagged on next recv.
        let mut got_lag = false;
        for _ in 0..10 {
            match slow.try_recv() {
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    got_lag = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(got_lag, "slow consumer should observe Lagged");

        // Fast consumer kept up — drop sender to close, then await.
        drop(hub);
        let fast_count = fast_handle.await.unwrap();
        assert!(fast_count >= 16, "fast consumer should have received recent events");
    }

    #[tokio::test]
    async fn test_anomaly_event_round_trips() {
        let hub = DerivedStreamHub::new(16);
        let mut rx = hub.subscribe();
        let ev = Event::Anomaly {
            source: "mic".into(),
            reason: AnomalyKind::SourceFault,
            severity: 200,
            at: SystemTime::now(),
        };
        hub.publish(ev);
        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(received.is_anomaly());
    }

    #[tokio::test]
    async fn test_minimum_capacity_floor() {
        // Even capacity=1 should be raised to at least 16 to avoid edge cases.
        let hub = DerivedStreamHub::new(1);
        let mut rx = hub.subscribe();
        // Should comfortably hold ≥ 8 events back-to-back.
        for i in 0..8 {
            hub.publish(change_event(&format!("c-{i}")));
        }
        let mut got = 0;
        while let Ok(_) = rx.try_recv() {
            got += 1;
        }
        assert!(got >= 8, "expected capacity floor to be raised to ≥ 16");
    }
}
