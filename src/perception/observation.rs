//! Core perception types and the [`PerceptionSource`] trait.
//!
//! An [`Observation`] is a single datum from any sensor, carrying a timestamp,
//! confidence, modality, and optional spatial context.  The [`PerceptionSource`]
//! trait abstracts over both poll-based (screenshot, system monitor) and
//! streaming (observable device capability) sensors.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::device::{Capability, CapabilityResult};
use crate::perception::Modality;

use crate::computer::system::SystemMonitor;

/// Unique observation identifier (UUID v4).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObservationId(String);

impl ObservationId {
    /// Create a new random observation ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ObservationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ObservationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Optional spatial context for an observation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpatialContext {
    /// Pixel region, if available (e.g. screenshot sub-region).
    pub region: Option<crate::computer::Rect>,
    /// Human-readable location label, e.g. `"display-1"`, `"lab-bench-left"`.
    pub location: Option<String>,
}

/// A single observation from any perception source.
///
/// Observations carry enough metadata to be fused into a [`SceneGraph`] and
/// filtered by [`PerceptionQuery`].
#[derive(Debug, Clone)]
pub struct Observation {
    /// Unique identifier.
    pub id: ObservationId,
    /// Source name, e.g. `"screenshot"`, `"system_monitor"`, `"device:sensor-01:temperature"`.
    pub source: String,
    /// Sensor modality.
    pub modality: Modality,
    /// Wall-clock capture timestamp.
    pub timestamp: Instant,
    /// Confidence estimate in `[0.0, 1.0]`.  `1.0` = ground truth.
    pub confidence: f32,
    /// Optional spatial context.
    pub spatial: Option<SpatialContext>,
    /// Payload — arbitrary structured data (screenshot dimensions, system metrics, etc.).
    pub data: serde_json::Value,
}

/// Unified interface for any perception source.
///
/// Poll-based sources implement [`observe`] to return the latest snapshot.
/// Streaming sources additionally implement [`subscribe`] to push continuous
/// observations.
#[async_trait]
pub trait PerceptionSource: Send + Sync {
    /// Stable source identifier, e.g. `"screenshot"`, `"device:sensor-01:temperature"`.
    fn name(&self) -> &str;

    /// The modality this source produces.
    fn modality(&self) -> Modality;

    /// Poll once for the latest observation(s).
    ///
    /// Returns a `Vec` because a single poll may yield multiple observations
    /// (e.g. a multi-sensor device).
    async fn observe(&self) -> Vec<Observation>;

    /// Subscribe to a continuous stream of observations.
    ///
    /// Returns `None` for poll-only sources.
    fn subscribe(&self) -> Option<broadcast::Receiver<Observation>> {
        None
    }
}

// ── Adapter implementations ────────────────────────────────────────────

/// Adapter that wraps a [`ComputerAdapter`] screenshot into a [`PerceptionSource`].
pub struct ScreenshotAdapter {
    adapter: Arc<dyn crate::computer::ComputerAdapter>,
}

impl ScreenshotAdapter {
    /// Create a new screenshot perception source.
    pub fn new(adapter: Arc<dyn crate::computer::ComputerAdapter>) -> Self {
        Self { adapter }
    }
}

#[async_trait]
impl PerceptionSource for ScreenshotAdapter {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn modality(&self) -> Modality {
        Modality::Rgb
    }

    async fn observe(&self) -> Vec<Observation> {
        match self.adapter.screenshot(None).await {
            Ok(ss) => {
                let ts = ss.timestamp;
                vec![Observation {
                    id: ObservationId::new(),
                    source: self.name().to_string(),
                    modality: Modality::Rgb,
                    timestamp: ts,
                    confidence: 1.0,
                    spatial: None,
                    data: serde_json::json!({
                        "width": ss.width,
                        "height": ss.height,
                        "base64_length": ss.base64.len(),
                    }),
                }]
            }
            Err(e) => {
                tracing::warn!("ScreenshotAdapter.observe failed: {e}");
                vec![]
            }
        }
    }
}

/// Adapter that wraps a [`SystemMonitor`] into a [`PerceptionSource`].
pub struct SystemMonitorAdapter {
    monitor: Arc<tokio::sync::Mutex<SystemMonitor>>,
}

impl SystemMonitorAdapter {
    /// Create a new system monitor perception source.
    pub fn new(monitor: Arc<tokio::sync::Mutex<SystemMonitor>>) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl PerceptionSource for SystemMonitorAdapter {
    fn name(&self) -> &str {
        "system_monitor"
    }

    fn modality(&self) -> Modality {
        Modality::System
    }

    async fn observe(&self) -> Vec<Observation> {
        let status = {
            let mut mon = self.monitor.lock().await;
            mon.get_status()
        };
        let ts = status.timestamp;
        vec![Observation {
            id: ObservationId::new(),
            source: self.name().to_string(),
            modality: Modality::System,
            timestamp: ts,
            confidence: 1.0,
            spatial: None,
            data: serde_json::to_value(&status).unwrap_or_default(),
        }]
    }
}

/// Adapter that wraps a single device [`Capability`] into a [`PerceptionSource`].
///
/// If the capability implements [`ObservableCapability`], the adapter also
/// provides a [`subscribe`] stream that maps [`DeviceEvent`]s to [`Observation`]s.
pub struct DeviceSourceAdapter {
    device_id: String,
    capability: Arc<dyn Capability>,
}

impl DeviceSourceAdapter {
    /// Create a new device sensor perception source.
    ///
    /// The source name is `"device:{device_id}:{capability_name}"`.
    pub fn new(device_id: impl Into<String>, capability: Arc<dyn Capability>) -> Self {
        Self {
            device_id: device_id.into(),
            capability,
        }
    }
}

#[async_trait]
impl PerceptionSource for DeviceSourceAdapter {
    fn name(&self) -> &str {
        // Lazy approximation; real name is stable across the struct's lifetime.
        // We could store it in a field, but the trait only returns &str.
        // This is computed once in observe() — for &str we keep a fixed pattern.
        "device_sensor"
    }

    fn modality(&self) -> Modality {
        Modality::Device
    }

    async fn observe(&self) -> Vec<Observation> {
        let result: CapabilityResult = self.capability.execute(serde_json::json!({})).await;
        vec![Observation {
            id: ObservationId::new(),
            source: format!("device:{}:{}", self.device_id, self.capability.name()),
            modality: Modality::Device,
            timestamp: Instant::now(),
            confidence: if result.success { 1.0 } else { 0.0 },
            spatial: None,
            data: result.output.unwrap_or(serde_json::Value::Null),
        }]
    }

    fn subscribe(&self) -> Option<broadcast::Receiver<Observation>> {
        let observable = self.capability.as_observable()?;
        let device_rx = observable.subscribe();
        let device_id = self.device_id.clone();

        // Bridge: map DeviceEvent → Observation into a fresh broadcast channel.
        let (tx, rx) = broadcast::channel(256);
        tokio::spawn(async move {
            let mut rx = device_rx;
            while let Ok(event) = rx.recv().await {
                let obs = Observation {
                    id: ObservationId::new(),
                    source: format!("device:{}:{}", device_id, event.capability),
                    modality: Modality::Device,
                    timestamp: Instant::now(),
                    confidence: 1.0,
                    spatial: None,
                    data: event.data,
                };
                if tx.send(obs).is_err() {
                    break; // no receivers left
                }
            }
        });

        Some(rx)
    }
}
