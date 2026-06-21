//! Streaming wrapper around [`FusionEngine`].
//!
//! [`FusionEngine`] is a pure function: `(&[Observation], FusionConfig) →
//! Vec<FusedEntity>`. To make it part of the streaming pipeline we need
//! a stateful wrapper that:
//!
//! 1. Subscribes to the raw [`PerceptionStreamHub`].
//! 2. Buffers recent observations in a sliding window.
//! 3. Periodically re-fuses and emits new [`FusedEntity`]s to the
//!    [`DerivedStreamHub`] as [`Event::Entity`].
//!
//! The wrapper is **shared infrastructure** — every agent sees the
//! same fusion output. Per-agent filtering happens later, in the
//! [`super::AttentionGate`].
//!
//! # Dedup
//!
//! Re-fusing every `tick_interval` would emit the same entity many
//! times in a row (the underlying observations don't change between
//! ticks). We dedup by `(entity.id, entity.modalities)` — an entity is
//! re-emitted only when its modality set changes or the dedup window
//! has elapsed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{broadcast, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

use crate::perception::{
    DerivedStreamHub, Event, FusedEntity, FusionEngine, Observation, PerceptionStreamHub,
};

/// Default re-fuse cadence — balance between freshness and CPU cost.
pub const DEFAULT_FUSION_TICK: Duration = Duration::from_millis(200);

/// Default sliding-window depth for fusion input.
pub const DEFAULT_FUSION_BUFFER: Duration = Duration::from_secs(2);

/// Default re-emit window — same entity won't be emitted twice within
/// this duration unless its modalities change.
pub const DEFAULT_ENTITY_DEDUP_WINDOW: Duration = Duration::from_secs(5);

/// Configuration for the streaming fusion loop.
#[derive(Debug, Clone)]
pub struct FusionStreamConfig {
    /// How often to re-run fusion over the buffer.
    pub tick_interval: Duration,
    /// How long observations are kept in the rolling buffer.
    pub buffer_window: Duration,
    /// Same-entity dedup window.
    pub dedup_window: Duration,
}

impl Default for FusionStreamConfig {
    fn default() -> Self {
        Self {
            tick_interval: DEFAULT_FUSION_TICK,
            buffer_window: DEFAULT_FUSION_BUFFER,
            dedup_window: DEFAULT_ENTITY_DEDUP_WINDOW,
        }
    }
}

/// Per-entity emit fingerprint used for dedup.
#[derive(Debug, Clone)]
struct EmitRecord {
    /// When we last emitted this entity.
    at: Instant,
    /// Modality set at the time of last emit (sorted by Debug name).
    modality_signature: String,
}

/// Spawn a background task that bridges the raw observation hub into
/// the derived event hub via [`FusionEngine`].
///
/// Returns the [`JoinHandle`] so callers can `abort()` on shutdown.
pub fn spawn_fusion_stream(
    raw_hub: Arc<PerceptionStreamHub>,
    derived_hub: Arc<DerivedStreamHub>,
    engine: FusionEngine,
    config: FusionStreamConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let buffer: Arc<AsyncMutex<Vec<Observation>>> = Arc::new(AsyncMutex::new(Vec::new()));
        let recent_emits: Arc<AsyncMutex<HashMap<String, EmitRecord>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));

        // Forwarder: drain raw_hub into the buffer.
        let buf_for_recv = buffer.clone();
        let mut rx = raw_hub.subscribe();
        let recv_handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(obs) => {
                        let mut b = buf_for_recv.lock().await;
                        b.push(obs);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("fusion stream: lagged, skipped {} observations", n);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("fusion stream: raw hub closed");
                        break;
                    }
                }
            }
        });

        // Tick loop: re-fuse on a cadence.
        let mut ticker = tokio::time::interval(config.tick_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;

            // Snapshot + prune buffer.
            let snapshot: Vec<Observation> = {
                let mut b = buffer.lock().await;
                let cutoff = Instant::now()
                    .checked_sub(config.buffer_window)
                    .unwrap_or_else(Instant::now);
                b.retain(|o| o.timestamp >= cutoff);
                b.clone()
            };
            if snapshot.is_empty() {
                continue;
            }

            let entities = engine.fuse(&snapshot).await;

            // Emit with dedup.
            let mut emits = recent_emits.lock().await;
            let now = Instant::now();
            // Evict stale dedup entries.
            emits.retain(|_, rec| now.duration_since(rec.at) < config.dedup_window);

            for entity in entities {
                let signature = modality_signature(&entity);
                let should_emit = match emits.get(&entity.id) {
                    Some(rec) => {
                        rec.modality_signature != signature
                            || now.duration_since(rec.at) >= config.dedup_window
                    }
                    None => true,
                };
                if should_emit {
                    emits.insert(
                        entity.id.clone(),
                        EmitRecord {
                            at: now,
                            modality_signature: signature,
                        },
                    );
                    derived_hub.publish(Event::Entity { entity, at: SystemTime::now() });
                }
            }

            if recv_handle.is_finished() {
                break;
            }
        }

        recv_handle.abort();
    })
}

fn modality_signature(entity: &FusedEntity) -> String {
    let mut names: Vec<String> = entity.modalities.iter().map(|m| format!("{m:?}")).collect();
    names.sort();
    names.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::mock::MockPerceptionSource;
    use crate::perception::{FusionConfig, Modality, PerceptionSource};

    fn streaming_mock(name: &str) -> (Arc<MockPerceptionSource>, broadcast::Sender<Observation>) {
        let (mock, tx) = MockPerceptionSource::new(name).with_streaming(64);
        (Arc::new(mock), tx)
    }

    fn obs(source: &str, modality: Modality, conf: f32) -> Observation {
        Observation::new(source, modality, std::time::Instant::now(), conf, serde_json::json!({}))
    }

    #[tokio::test]
    async fn test_fusion_stream_emits_entity_on_multi_modal_input() {
        let raw_hub = Arc::new(PerceptionStreamHub::new(64));
        let derived_hub = Arc::new(DerivedStreamHub::new(64));
        let mut rx_derived = derived_hub.subscribe();

        let engine = FusionEngine::new(FusionConfig::default());
        let cfg = FusionStreamConfig {
            tick_interval: Duration::from_millis(50),
            buffer_window: Duration::from_secs(2),
            dedup_window: Duration::from_secs(5),
        };
        let _h = spawn_fusion_stream(raw_hub.clone(), derived_hub.clone(), engine, cfg);

        // Attach a streaming source so raw_hub has at least one forwarder.
        let (mock, tx) = streaming_mock("cam");
        raw_hub
            .attach_source("cam", mock as Arc<dyn PerceptionSource>)
            .await;

        // Push two observations of different modalities into the same temporal cluster.
        tx.send(obs("cam", Modality::Rgb, 0.9)).unwrap();
        tx.send(obs("cam", Modality::Audio, 0.85)).unwrap();

        // Wait for an entity event.
        let received = tokio::time::timeout(Duration::from_secs(2), rx_derived.recv())
            .await
            .expect("did not receive entity event in time")
            .expect("derived hub closed");
        match received {
            Event::Entity { entity, .. } => {
                assert!(entity.modalities.len() >= 1);
            }
            other => panic!("expected Entity event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_fusion_stream_dedups_repeated_emits() {
        let raw_hub = Arc::new(PerceptionStreamHub::new(64));
        let derived_hub = Arc::new(DerivedStreamHub::new(64));
        let mut rx_derived = derived_hub.subscribe();

        let engine = FusionEngine::new(FusionConfig::default());
        // tick fast, dedup wide.
        let cfg = FusionStreamConfig {
            tick_interval: Duration::from_millis(20),
            buffer_window: Duration::from_secs(2),
            dedup_window: Duration::from_secs(60),
        };
        let _h = spawn_fusion_stream(raw_hub.clone(), derived_hub.clone(), engine, cfg);

        let (mock, tx) = streaming_mock("cam");
        raw_hub
            .attach_source("cam", mock as Arc<dyn PerceptionSource>)
            .await;

        // Single observation pair: should yield one entity, not many.
        tx.send(obs("cam", Modality::Rgb, 0.9)).unwrap();
        tx.send(obs("cam", Modality::Audio, 0.85)).unwrap();

        // Drain for ~250 ms (which is ~12 ticks); count entity events.
        let mut entity_count = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            match tokio::time::timeout(deadline - now, rx_derived.recv()).await {
                Ok(Ok(Event::Entity { .. })) => entity_count += 1,
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => break,
            }
        }

        assert!(entity_count <= 2, "dedup should suppress repeat emits, got {entity_count}");
        assert!(entity_count >= 1, "should emit at least once");
    }
}
