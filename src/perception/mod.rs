//! Perception Fusion Layer.
//!
//! Unifies fragmented perception sources (screenshots, system monitoring, device
//! sensors) under a common data model and query interface.  The [`PerceptionRegistry`]
//! manages multiple [`PerceptionSource`]s, ingests observations into a [`SceneGraph`],
//! and exposes them via [`PerceptionQuery`].
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
pub mod mock;
mod observation;
mod query;
mod registry;
mod scene_graph;

pub use aggregator::{AggregationStrategy, TemporalAggregator};
pub use mock::MockPerceptionSource;
pub use observation::{
    DeviceSourceAdapter, Observation, ObservationId, PerceptionSource, ScreenshotAdapter,
    SpatialContext, SystemMonitorAdapter,
};
pub use query::{PerceptionQuery, QueryResult};
pub use registry::PerceptionRegistry;
pub use scene_graph::{Entity, EntityId, Relationship, SceneGraph};
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
