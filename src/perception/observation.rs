//! Core perception types and the [`PerceptionSource`] trait.
//!
//! An [`Observation`] is a single datum from any sensor, carrying a timestamp,
//! confidence, and modality.  The [`PerceptionSource`]
//! trait abstracts over both poll-based (screenshot, system monitor) and
//! streaming (observable device capability) sensors.

use std::sync::Arc;
use std::time::{Instant, SystemTime};

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

/// A single observation from any perception source.
///
/// Observations carry enough metadata to be fused and filtered by
/// [`PerceptionQuery`].
#[derive(Debug, Clone)]
pub struct Observation {
    /// Unique identifier.
    pub id: ObservationId,
    /// Source name, e.g. `"screenshot"`, `"system_monitor"`, `"device:sensor-01:temperature"`.
    pub source: String,
    /// Sensor modality.
    pub modality: Modality,
    /// Wall-clock capture timestamp (process-relative; not portable across restarts).
    pub timestamp: Instant,
    /// System time when the observation was created — durable across restarts.
    /// Used by persistent stores to sort and prune by absolute date.
    pub created_at: SystemTime,
    /// Confidence estimate in `[0.0, 1.0]`.  `1.0` = ground truth.
    pub confidence: f32,
    /// Payload — arbitrary structured data (screenshot dimensions, system metrics, etc.).
    pub data: serde_json::Value,
}

impl Observation {
    /// Create an observation with `created_at = SystemTime::now()`.
    pub fn new(
        source: impl Into<String>,
        modality: Modality,
        timestamp: Instant,
        confidence: f32,
        data: serde_json::Value,
    ) -> Self {
        Self {
            id: ObservationId::new(),
            source: source.into(),
            modality,
            timestamp,
            created_at: SystemTime::now(),
            confidence,
            data,
        }
    }
}

/// Reports whether a perception source is fully operational or degraded.
///
/// Allows the LLM to distinguish between "microphone is working but no
/// sound detected" and "microphone hardware is unavailable".
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SourceStatus {
    /// Source is fully operational — hardware available, no errors.
    Healthy,

    /// Source is unavailable or degraded, with a human-readable reason.
    Unavailable {
        /// Description of why the source is unavailable.
        message: String,
    },
}

impl SourceStatus {
    /// Returns `true` if the source is healthy.
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }
}

impl Default for SourceStatus {
    fn default() -> Self {
        Self::Healthy
    }
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

    /// Current operational status of this source.
    ///
    /// Returns [`SourceStatus::Healthy`] by default. Adapters that depend on
    /// optional hardware (e.g. microphone) should override this to report
    /// `Unavailable` when the hardware cannot be accessed.
    fn status(&self) -> SourceStatus {
        SourceStatus::Healthy
    }

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
                    created_at: SystemTime::now(),
                    confidence: 1.0,
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
            created_at: SystemTime::now(),
            confidence: 1.0,
            data: serde_json::to_value(&status).unwrap_or_default(),
        }]
    }
}

/// Adapter that wraps a single device [`Capability`] into a [`PerceptionSource`].
///
/// If the capability implements [`ObservableCapability`], the adapter also
/// provides a [`subscribe`] stream that maps [`DeviceEvent`]s to [`Observation`]s.
///
/// By default all capabilities are mapped to [`Modality::Device`]. Use
/// [`with_modality`](DeviceSourceAdapter::with_modality) to specify a more
/// specific modality based on the capability's sensor type.
pub struct DeviceSourceAdapter {
    source_name: String,
    device_id: String,
    capability: Arc<dyn Capability>,
    modality: Modality,
}

impl DeviceSourceAdapter {
    /// Create a new device sensor perception source.
    ///
    /// The source name is `"device:{device_id}:{capability_name}"`.
    /// Modality defaults to [`Modality::Device`]; override with
    /// [`with_modality`](DeviceSourceAdapter::with_modality).
    pub fn new(device_id: impl Into<String>, capability: Arc<dyn Capability>) -> Self {
        let device_id = device_id.into();
        let source_name = format!("device:{}:{}", &device_id, capability.name());
        Self {
            source_name,
            device_id,
            capability,
            modality: Modality::Device,
        }
    }

    /// Override the sensor modality for this adapter.
    ///
    /// Use this when the capability represents a specific sensor type
    /// (e.g. `Modality::Rgb` for a camera, `Modality::Audio` for a
    /// microphone device) rather than the generic `Modality::Device`.
    pub fn with_modality(mut self, modality: Modality) -> Self {
        self.modality = modality;
        self
    }
}

#[async_trait]
impl PerceptionSource for DeviceSourceAdapter {
    fn name(&self) -> &str {
        &self.source_name
    }

    fn modality(&self) -> Modality {
        self.modality
    }

    async fn observe(&self) -> Vec<Observation> {
        let result: CapabilityResult = self.capability.execute(serde_json::json!({})).await;
        vec![Observation {
            id: ObservationId::new(),
            source: self.name().to_string(),
            modality: self.modality,
            timestamp: Instant::now(),
            created_at: SystemTime::now(),
            confidence: if result.success { 1.0 } else { 0.0 },
            data: result.output.unwrap_or(serde_json::Value::Null),
        }]
    }

    fn subscribe(&self) -> Option<broadcast::Receiver<Observation>> {
        let observable = self.capability.as_observable()?;
        let device_rx = observable.subscribe();
        let device_id = self.device_id.clone();
        let modality = self.modality; // Copy for the 'static task

        // Bridge: map DeviceEvent → Observation into a fresh broadcast channel.
        let (tx, rx) = broadcast::channel(256);
        tokio::spawn(async move {
            let mut rx = device_rx;
            while let Ok(event) = rx.recv().await {
                let obs = Observation {
                    id: ObservationId::new(),
                    source: format!("device:{}:{}", device_id, event.capability),
                    modality,
                    timestamp: Instant::now(),
                    created_at: SystemTime::now(),
                    confidence: 1.0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::MockCapability;

    #[tokio::test]
    async fn test_device_source_adapter_unique_names() {
        let cap1 = Arc::new(MockCapability::new("temperature"));
        let cap2 = Arc::new(MockCapability::new("pressure"));

        let adapter1 = DeviceSourceAdapter::new("device-01", cap1);
        let adapter2 = DeviceSourceAdapter::new("device-01", cap2);
        let adapter3 = DeviceSourceAdapter::new("device-02", Arc::new(MockCapability::new("temperature")));

        assert_eq!(adapter1.name(), "device:device-01:temperature");
        assert_eq!(adapter2.name(), "device:device-01:pressure");
        assert_eq!(adapter3.name(), "device:device-02:temperature");

        // Verify all three names are distinct
        let mut names = vec![
            adapter1.name().to_string(),
            adapter2.name().to_string(),
            adapter3.name().to_string(),
        ];
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 3);
    }

    #[tokio::test]
    async fn test_device_source_adapter_observe_uses_name() {
        let cap = Arc::new(MockCapability::new("temp.sensor"));
        let adapter = DeviceSourceAdapter::new("my-dev", cap);
        let obs = adapter.observe().await;
        assert!(!obs.is_empty());
        assert_eq!(obs[0].source, adapter.name());
    }
}
