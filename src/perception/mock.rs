//! Mock perception source for testing.
//!
//! Provides [`MockPerceptionSource`] with configurable name, modality, and data
//! so that tests of the perception pipeline don't require real sensors.

use std::time::Instant;

use async_trait::async_trait;

use crate::perception::{Modality, Observation, ObservationId, PerceptionSource, SourceStatus};

/// A configurable mock perception source for testing.
pub struct MockPerceptionSource {
    name: String,
    modality: Modality,
    data: serde_json::Value,
    confidence: f32,
    status: SourceStatus,
}

impl MockPerceptionSource {
    /// Create a new mock source with the given name.
    ///
    /// Defaults: `Modality::Other`, data = `json!(null)`, confidence = `1.0`,
    /// status = `Healthy`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            modality: Modality::Other,
            data: serde_json::Value::Null,
            confidence: 1.0,
            status: SourceStatus::Healthy,
        }
    }

    /// Set the modality of this mock source.
    pub fn with_modality(mut self, modality: Modality) -> Self {
        self.modality = modality;
        self
    }

    /// Set the data payload returned by this mock source.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }

    /// Set the confidence of observations from this mock source.
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence;
        self
    }

    /// Set the operational status of this mock source.
    pub fn with_status(mut self, status: SourceStatus) -> Self {
        self.status = status;
        self
    }
}

#[async_trait]
impl PerceptionSource for MockPerceptionSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn modality(&self) -> Modality {
        self.modality
    }

    fn status(&self) -> SourceStatus {
        self.status.clone()
    }

    async fn observe(&self) -> Vec<Observation> {
        vec![Observation {
            id: ObservationId::new(),
            source: self.name.clone(),
            modality: self.modality,
            timestamp: Instant::now(),
            confidence: self.confidence,
            data: self.data.clone(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_source_basics() {
        let src = MockPerceptionSource::new("mock_sensor")
            .with_modality(Modality::Device)
            .with_data(serde_json::json!({"temperature": 25.0}));

        assert_eq!(src.name(), "mock_sensor");
        assert_eq!(src.modality(), Modality::Device);

        let obs = src.observe().await;
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].source, "mock_sensor");
        assert_eq!(
            obs[0].data,
            serde_json::json!({"temperature": 25.0})
        );
    }

    #[tokio::test]
    async fn test_mock_source_subscribe_none() {
        let src = MockPerceptionSource::new("mock");
        assert!(src.subscribe().is_none());
    }
}
