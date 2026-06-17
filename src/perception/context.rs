//! [`PerceptionContext`] — the shared infrastructure factory.
//!
//! `PerceptionContext` owns one set of upstream pipeline pieces:
//!
//! - [`PerceptionStreamHub`]   — raw observation broadcast
//! - [`DerivedStreamHub`]      — post-pipeline event broadcast
//! - [`DefaultTemporalProcessor`] — sliding-window aggregator
//! - [`FusionEngine`]          — cross-modal fusion
//!
//! and the background tasks that connect them
//! ([`spawn_temporal_processor`] + [`spawn_fusion_stream`]). Per-agent
//! [`MinimalAdapter`]s are minted via [`PerceptionContext::new_adapter`]
//! and share these resources transparently.
//!
//! ```text
//!  PerceptionRegistry ──► raw_hub ──► temporal_processor (shared)
//!                              │           │
//!                              │           ▼
//!                              │      Snapshot::aggregates
//!                              │
//!                              ▼
//!                         FusionEngine ──► derived_hub ──► MinimalAdapter (per-agent)
//! ```
//!
//! # Lifecycle
//!
//! - [`start`] spawns the two background tasks and returns the context.
//! - [`new_adapter`] mints a per-agent adapter that subscribes to the
//!   shared hubs.
//! - [`shutdown`] aborts the temporal/fusion handles. Per-agent
//!   adapters must be shut down separately by their owners.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::perception::{
    spawn_fusion_stream, spawn_temporal_processor, AdapterConfig, DefaultTemporalProcessor,
    DerivedStreamHub, Focus, FusionConfig, FusionEngine, FusionStreamConfig, MinimalAdapter,
    PerceptionStreamHub, PerceptionSummarizer, DEFAULT_DERIVED_HUB_CAPACITY,
    DEFAULT_TEMPORAL_WINDOW,
};

/// Default raw-hub capacity.
pub const DEFAULT_RAW_HUB_CAPACITY: usize = 1024;

/// Tunables for [`PerceptionContext::start`].
#[derive(Debug, Clone)]
pub struct PerceptionContextConfig {
    /// Broadcast capacity for the raw observation hub.
    pub raw_hub_capacity: usize,
    /// Broadcast capacity for the derived event hub.
    pub derived_hub_capacity: usize,
    /// Sliding-window duration for the temporal aggregator.
    pub temporal_window: Duration,
    /// Fusion engine configuration.
    pub fusion: FusionConfig,
    /// Fusion streaming loop configuration.
    pub fusion_stream: FusionStreamConfig,
}

impl Default for PerceptionContextConfig {
    fn default() -> Self {
        Self {
            raw_hub_capacity: DEFAULT_RAW_HUB_CAPACITY,
            derived_hub_capacity: DEFAULT_DERIVED_HUB_CAPACITY,
            temporal_window: DEFAULT_TEMPORAL_WINDOW,
            fusion: FusionConfig::default(),
            fusion_stream: FusionStreamConfig::default(),
        }
    }
}

/// Shared per-gateway perception infrastructure.
///
/// Construct once at gateway startup; share the `Arc` with anything that
/// needs to mint per-agent adapters or attach perception sources.
pub struct PerceptionContext {
    /// Raw observation hub. Sources publish here via
    /// [`PerceptionStreamHub::attach_source`].
    raw_hub: Arc<PerceptionStreamHub>,
    /// Derived event hub.
    derived_hub: Arc<DerivedStreamHub>,
    /// Sliding-window aggregator (subscribed to `raw_hub`).
    temporal: Arc<DefaultTemporalProcessor>,
    /// Background task handles — kept so [`shutdown`] can abort them.
    handles: Vec<JoinHandle<()>>,
}

impl PerceptionContext {
    /// Create the shared infrastructure and spawn the background tasks.
    pub fn start(config: PerceptionContextConfig) -> Self {
        let raw_hub = Arc::new(PerceptionStreamHub::new(config.raw_hub_capacity));
        let derived_hub = Arc::new(DerivedStreamHub::new(config.derived_hub_capacity));
        let temporal = Arc::new(DefaultTemporalProcessor::new(config.temporal_window));

        let temporal_handle = spawn_temporal_processor(raw_hub.clone(), temporal.clone());

        let engine = FusionEngine::new(config.fusion);
        let fusion_handle = spawn_fusion_stream(
            raw_hub.clone(),
            derived_hub.clone(),
            engine,
            config.fusion_stream,
        );

        Self {
            raw_hub,
            derived_hub,
            temporal,
            handles: vec![temporal_handle, fusion_handle],
        }
    }

    /// Borrow the shared raw hub. Sources attach here.
    pub fn raw_hub(&self) -> &Arc<PerceptionStreamHub> {
        &self.raw_hub
    }

    /// Borrow the shared derived hub. Useful for direct subscribers
    /// (debug tools, system monitors) that don't need a full adapter.
    pub fn derived_hub(&self) -> &Arc<DerivedStreamHub> {
        &self.derived_hub
    }

    /// Borrow the shared temporal processor. Use
    /// [`super::TemporalProcessor::snapshot_aggregates`] to read it.
    pub fn temporal(&self) -> &Arc<DefaultTemporalProcessor> {
        &self.temporal
    }

    /// Mint a per-agent adapter wired into this context.
    pub fn new_adapter(
        &self,
        focus: Focus,
        summarizer: Option<Arc<dyn PerceptionSummarizer>>,
        adapter_config: AdapterConfig,
    ) -> Arc<MinimalAdapter> {
        MinimalAdapter::new(
            self.raw_hub.clone(),
            self.derived_hub.clone(),
            self.temporal.clone(),
            summarizer,
            focus,
            adapter_config,
        )
    }

    /// Abort the shared background tasks. Per-agent adapters created
    /// from this context are unaffected — call their own
    /// [`super::AgentPerceptionAdapter::shutdown`] separately.
    pub async fn shutdown(mut self) {
        for h in self.handles.drain(..) {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{
        AgentPerceptionAdapter, Event, Modality, MockPerceptionSource, Observation,
        ObservationId, PerceptionSource,
    };
    use std::time::{Instant, SystemTime};
    use tokio::sync::broadcast;

    fn obs(source: &str, modality: Modality, conf: f32) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality,
            timestamp: Instant::now(),
            created_at: SystemTime::now(),
            confidence: conf,
            data: serde_json::json!({}),
        }
    }

    async fn attach_streaming(
        hub: &PerceptionStreamHub,
        name: &str,
    ) -> broadcast::Sender<Observation> {
        let (mock, tx) = MockPerceptionSource::new(name).with_streaming(64);
        hub.attach_source(name, Arc::new(mock) as Arc<dyn PerceptionSource>)
            .await;
        tx
    }

    #[tokio::test]
    async fn test_start_with_defaults_succeeds() {
        let ctx = PerceptionContext::start(PerceptionContextConfig::default());
        assert!(ctx.handles.len() == 2);
        ctx.shutdown().await;
    }

    #[tokio::test]
    async fn test_new_adapter_receives_entity_event_via_fusion() {
        let cfg = PerceptionContextConfig {
            fusion_stream: FusionStreamConfig {
                tick_interval: Duration::from_millis(50),
                buffer_window: Duration::from_secs(2),
                dedup_window: Duration::from_secs(5),
            },
            ..Default::default()
        };
        let ctx = PerceptionContext::start(cfg);
        let adapter = ctx.new_adapter(Focus::default(), None, AdapterConfig::default());

        let tx = attach_streaming(ctx.raw_hub(), "cam").await;
        // Two modalities in a temporal cluster → fusion should bind them.
        tx.send(obs("cam", Modality::Rgb, 0.9)).unwrap();
        tx.send(obs("cam", Modality::Audio, 0.85)).unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(2), adapter.next_event())
            .await
            .expect("timeout")
            .expect("pipeline closed");
        match ev {
            Event::Entity { .. } => {}
            other => panic!("expected Entity event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_temporal_aggregates_visible_via_now() {
        let ctx = PerceptionContext::start(PerceptionContextConfig::default());
        let adapter = ctx.new_adapter(Focus::default(), None, AdapterConfig::default());

        let tx = attach_streaming(ctx.raw_hub(), "cpu").await;
        let mut o = obs("cpu", Modality::System, 1.0);
        o.data = serde_json::json!({"cpu_pct": 42.0});
        tx.send(o).unwrap();

        // Allow the temporal processor to ingest.
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let snap = adapter.now();
            if let Some(agg) = snap
                .aggregates
                .get(&("cpu".to_string(), Modality::System))
            {
                assert_eq!(agg.stats["count"], 1);
                assert_eq!(agg.stats["mean"], 42.0);
                return;
            }
        }
        panic!("aggregate never appeared in snapshot");
    }

    #[tokio::test]
    async fn test_two_adapters_share_infrastructure() {
        let ctx = PerceptionContext::start(PerceptionContextConfig::default());
        let a1 = ctx.new_adapter(Focus::default(), None, AdapterConfig::default());
        let a2 = ctx.new_adapter(
            Focus::default().with_modalities([Modality::FileSystem]),
            None,
            AdapterConfig::default(),
        );

        // Anomaly published to derived_hub — both adapters should see it
        // (anomaly bypasses gates).
        ctx.derived_hub().publish(Event::Anomaly {
            source: "cpu".into(),
            reason: crate::perception::AnomalyKind::SourceFault,
            severity: 200,
            at: SystemTime::now(),
        });

        let r1 = tokio::time::timeout(Duration::from_secs(1), a1.next_event())
            .await
            .expect("a1 timeout")
            .expect("a1 closed");
        let r2 = tokio::time::timeout(Duration::from_secs(1), a2.next_event())
            .await
            .expect("a2 timeout")
            .expect("a2 closed");
        assert!(r1.is_anomaly());
        assert!(r2.is_anomaly());
    }
}
