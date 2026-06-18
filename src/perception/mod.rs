//! Perception Fusion Layer.
//!
//! Unifies fragmented perception sources (screenshots, system monitoring, device
//! sensors) under a common data model and query interface.  The [`PerceptionRegistry`]
//! manages multiple [`PerceptionSource`]s, ingests observations into a
//! [`TemporalAggregator`], and exposes them via [`PerceptionQuery`].
//!
//! # Architecture
//!
//! ```text
//! Agent/LLM → PerceptionQueryTool → PerceptionRegistry
//!                                       │
//!                           ┌───────────┼───────────┐
//!                           ▼           ▼           ▼
//!                    Screenshot    SystemMonitor  DeviceSource
//!                    Adapter       Adapter        Adapter
//! ```

mod aggregator;
pub mod agent_adapter;
pub mod attention_gate;
pub mod audio_adapter;
pub mod context;
pub mod derived_stream;
pub mod event;
pub mod focus;
mod fusion;
pub mod fusion_adaptive;
pub mod fusion_stream;
pub mod health;
pub mod llm_summarizer;
pub mod minimal_adapter;
pub mod mock;
mod observation;
pub mod persistence;
mod query;
mod registry;
pub mod salience_filter;
pub mod snapshot;
pub mod stream;
pub mod temporal_processor;

pub use aggregator::{AggregationStrategy, Entity, EntityId, TemporalAggregator};
pub use agent_adapter::{AdapterError, AgentPerceptionAdapter, PerceptionSummarizer};
pub use attention_gate::AttentionGate;
pub use audio_adapter::{AudioAdapterConfig, MicrophoneAdapter};
pub use context::{PerceptionContext, PerceptionContextConfig, DEFAULT_RAW_HUB_CAPACITY};
pub use derived_stream::{DerivedStreamHub, DEFAULT_DERIVED_HUB_CAPACITY};
pub use event::{Aggregate, AnomalyKind, Event};
pub use focus::{Focus, SalienceConfig};
pub use fusion::{FusedEntity, FusionConfig, FusionEngine};
pub use fusion_adaptive::{spawn_adaptive_fusion_loop, SensorNoiseTracker};
pub use fusion_stream::{
    spawn_fusion_stream, FusionStreamConfig, DEFAULT_ENTITY_DEDUP_WINDOW, DEFAULT_FUSION_BUFFER,
    DEFAULT_FUSION_TICK,
};
pub use health::{HealthConfig, HealthState, HealthTracker, SourceHealth};
pub use llm_summarizer::LlmProviderSummarizer;
pub use minimal_adapter::{AdapterConfig, MinimalAdapter};
pub use mock::MockPerceptionSource;
pub use observation::{
    DeviceSourceAdapter, Observation, ObservationId, PerceptionSource, ScreenshotAdapter,
    SourceStatus, SystemMonitorAdapter,
};
pub use persistence::{
    build_store, JsonlObservationStore, NullObservationStore, ObservationStore, StoreError,
};
pub use query::{PerceptionQuery, QueryResult};
pub use registry::PerceptionRegistry;
pub use salience_filter::SalienceFilter;
pub use snapshot::Snapshot;
pub use stream::{spawn_stream_hub_sync, PerceptionStreamHub};
pub use temporal_processor::{
    spawn_temporal_processor, DefaultTemporalProcessor, TemporalProcessor,
    DEFAULT_TEMPORAL_WINDOW,
};
pub use crate::tools::perception_tool::PerceptionQueryTool;

/// Sensor modality classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Modality {
    /// RGB camera / screenshot.
    Rgb,
    /// Depth sensor.
    Depth,
    /// Microphone / audio stream.
    Audio,
    /// Tactile / force feedback.
    Tactile,
    /// System resource metrics (CPU, memory, disk, network).
    System,
    /// Generic device sensor (temperature, pressure, etc.).
    Device,
    /// Accessibility / UI tree.
    UiTree,
    /// File system events.
    FileSystem,
    /// Network inspection.
    Network,
    /// Catch-all for unclassified modalities.
    Other,
}
