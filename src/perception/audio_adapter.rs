//! Microphone audio adapter — wraps [`AudioCapture`] as a [`PerceptionSource`].
//!
//! Captures audio from the microphone (or system output loopback) and maps
//! each [`AudioSegment`] together with [`DetectedAudioEvent`] classifications
//! into an [`Observation`] with [`Modality::Audio`].
//!
//! # Architecture
//!
//! ```text
//! AudioCapture (cpal) ──mpsc──▶ spawn_blocking
//!                                  │ AudioSegment
//!                                  ▼
//!                              analyze_segment()
//!                                  │ DetectedAudioEvent
//!                                  ▼
//!                              segment_to_observation()
//!                                  │ Observation
//!                                  ▼
//!                              broadcast::Sender ──▶ subscribe() receivers
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::computer::audio::{AudioCapture, AudioSegment, AudioSource, DetectedAudioEvent};
use crate::perception::{Modality, Observation, ObservationId, PerceptionSource, SourceStatus};

/// Configuration for the microphone audio adapter.
#[derive(Debug, Clone)]
pub struct AudioAdapterConfig {
    /// Audio input source (Microphone or SystemOutput).
    pub audio_source: AudioSource,
    /// Sample rate in Hz (default 16_000).
    pub sample_rate: u32,
    /// Silence threshold in dB for VAD (default -40.0).
    pub silence_threshold_db: f32,
    /// Broadcast channel capacity (default 256).
    pub channel_capacity: usize,
    /// Minimum interval in seconds between hardware re-probes when the
    /// adapter is in [`Unavailable`](crate::perception::SourceStatus::Unavailable)
    /// state.  `0` (default) disables re-probing.
    ///
    /// When set to a positive value, [`observe`](Self::observe) will
    /// periodically attempt to re-create [`AudioCapture`] to detect
    /// hotplugged microphones.
    pub reprobe_interval_secs: u64,
}

impl Default for AudioAdapterConfig {
    fn default() -> Self {
        Self {
            audio_source: AudioSource::Microphone,
            sample_rate: 16_000,
            silence_threshold_db: -40.0,
            channel_capacity: 256,
            reprobe_interval_secs: 0,
        }
    }
}

/// Adapter that wraps [`AudioCapture`] as a [`PerceptionSource`].
///
/// This is a stream-only source — `observe()` returns empty, and
/// `subscribe()` returns a broadcast receiver that yields audio
/// [`Observation`]s at each captured segment boundary (~100 ms).
///
/// If no microphone hardware is available, the spawned capture task
/// logs a warning and exits silently; the broadcast channel simply
/// has no senders.
pub struct MicrophoneAdapter {
    source_name: String,
    config: AudioAdapterConfig,
    status: Arc<Mutex<SourceStatus>>,
    /// When the last hardware re-probe was attempted; `None` at startup.
    last_reprobe: Arc<Mutex<Option<Instant>>>,
}

impl MicrophoneAdapter {
    /// Create a new microphone perception source.
    ///
    /// Probes for audio hardware availability by attempting to create an
    /// [`AudioCapture`] instance. If no hardware is found, the adapter
    /// remains functional but reports [`SourceStatus::Unavailable`].
    /// When [`AudioAdapterConfig::reprobe_interval_secs`] is set, the
    /// adapter will periodically re-check for hotplugged hardware.
    pub fn new(config: AudioAdapterConfig) -> Self {
        let source_name = format!("audio:{}", config.audio_source);
        let status = Arc::new(Mutex::new(SourceStatus::Healthy));

        // Probe hardware availability
        match AudioCapture::new() {
            Ok(_) => {
                tracing::debug!("MicrophoneAdapter: audio hardware available");
            }
            Err(e) => {
                tracing::warn!("MicrophoneAdapter: audio hardware unavailable: {e}");
                if let Ok(mut s) = status.lock() {
                    *s = SourceStatus::Unavailable {
                        message: format!("audio hardware unavailable: {e}"),
                    };
                }
            }
        }

        Self {
            source_name,
            config,
            status,
            last_reprobe: Arc::new(Mutex::new(None)),
        }
    }

    /// Attempt to re-probe audio hardware.
    ///
    /// Called automatically by [`observe`] when `reprobe_interval_secs > 0`.
    /// Returns `true` if hardware is now available.
    fn try_reprobe(&self) -> bool {
        // Check if enough time has elapsed since the last probe.
        let interval = Duration::from_secs(self.config.reprobe_interval_secs);
        {
            if let Ok(last) = self.last_reprobe.lock() {
                if let Some(t) = *last {
                    if t.elapsed() < interval {
                        return self.is_status_healthy();
                    }
                }
            }
        }

        // Record the probe attempt timestamp.
        if let Ok(mut last) = self.last_reprobe.lock() {
            *last = Some(Instant::now());
        }

        match AudioCapture::new() {
            Ok(_) => {
                if let Ok(mut s) = self.status.lock() {
                    *s = SourceStatus::Healthy;
                }
                tracing::info!("MicrophoneAdapter: audio hardware became available (reprobe)");
                true
            }
            Err(e) => {
                if let Ok(mut s) = self.status.lock() {
                    *s = SourceStatus::Unavailable {
                        message: format!("audio hardware unavailable: {e}"),
                    };
                }
                false
            }
        }
    }

    fn is_status_healthy(&self) -> bool {
        self.status.lock().map(|s| s.is_healthy()).unwrap_or(false)
    }
}

#[async_trait]
impl PerceptionSource for MicrophoneAdapter {
    fn name(&self) -> &str {
        &self.source_name
    }

    fn modality(&self) -> Modality {
        Modality::Audio
    }

    fn status(&self) -> SourceStatus {
        self.status.lock().unwrap().clone()
    }

    async fn observe(&self) -> Vec<Observation> {
        // Poll-based observation is not meaningful for audio streams.
        // But we use the poll cycle as a hook to re-probe for hotplugged
        // audio hardware when reprobe_interval_secs is configured.
        if self.config.reprobe_interval_secs > 0 && !self.is_status_healthy() {
            self.try_reprobe();
        }
        vec![]
    }

    fn subscribe(&self) -> Option<broadcast::Receiver<Observation>> {
        let (tx, rx) = broadcast::channel(self.config.channel_capacity);
        let config = self.config.clone();

        tokio::spawn(async move {
            // Create AudioCapture and start — these happen inside the
            // spawned task so failure doesn't affect the adapter.
            let mut capture = match AudioCapture::new() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("MicrophoneAdapter: failed to create AudioCapture: {e}");
                    return;
                }
            };

            let mut segment_rx = match capture.start(config.audio_source).await {
                Ok(rx) => rx,
                Err(e) => {
                    tracing::warn!("MicrophoneAdapter: failed to start capture: {e}");
                    return;
                }
            };

            // Forward segments as observations until the channel closes
            // or all receivers are dropped.
            while let Some(segment) = segment_rx.recv().await {
                let events = capture.analyze_segment(&segment);
                let obs = Self::segment_to_observation(&segment, &events);
                if tx.send(obs).is_err() {
                    break; // no more receivers
                }
            }
        });

        Some(rx)
    }
}

impl MicrophoneAdapter {
    /// Map an [`AudioSegment`] plus its classified [`DetectedAudioEvent`]s
    /// into an [`Observation`].
    fn segment_to_observation(
        segment: &AudioSegment,
        events: &[DetectedAudioEvent],
    ) -> Observation {
        let source = format!("audio:{}", segment.source);
        let event_types: Vec<String> = events.iter().map(|e| format!("{e:?}")).collect();
        let confidence = if events.is_empty() { 0.5 } else { 0.9 };

        Observation {
            id: ObservationId::new(),
            source,
            modality: Modality::Audio,
            timestamp: segment.timestamp,
            confidence,
            data: serde_json::json!({
                "duration_ms": segment.duration_ms,
                "rms_energy": segment.rms_energy(),
                "event_types": event_types,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_name_and_modality() {
        let adapter = MicrophoneAdapter::new(AudioAdapterConfig::default());
        assert_eq!(adapter.name(), "audio:microphone");
        assert_eq!(adapter.modality(), Modality::Audio);
    }

    #[test]
    fn test_system_output_name() {
        let config = AudioAdapterConfig {
            audio_source: AudioSource::SystemOutput,
            ..Default::default()
        };
        let adapter = MicrophoneAdapter::new(config);
        assert_eq!(adapter.name(), "audio:system_output");
    }

    #[tokio::test]
    async fn test_observe_returns_empty() {
        let adapter = MicrophoneAdapter::new(AudioAdapterConfig::default());
        let obs = adapter.observe().await;
        assert!(obs.is_empty(), "audio adapter observe should return empty vec");
    }

    #[tokio::test]
    async fn test_subscribe_returns_receiver() {
        let adapter = MicrophoneAdapter::new(AudioAdapterConfig::default());
        let rx = adapter.subscribe();
        assert!(rx.is_some(), "audio adapter subscribe should return Some");
    }

    #[test]
    fn test_segment_to_observation_silence() {
        let segment = AudioSegment {
            timestamp: Instant::now(),
            samples: vec![0.0; 1600], // 100ms @ 16kHz
            duration_ms: 100,
            source: AudioSource::Microphone,
        };
        let events = vec![DetectedAudioEvent::Silence { duration_ms: 100 }];
        let obs = MicrophoneAdapter::segment_to_observation(&segment, &events);

        assert_eq!(obs.modality, Modality::Audio);
        assert_eq!(obs.source, "audio:microphone");
        assert!((obs.confidence - 0.9).abs() < 0.01);
        assert_eq!(obs.data["duration_ms"], 100);
        assert_eq!(obs.data["rms_energy"], 0.0);
    }

    #[test]
    fn test_segment_to_observation_speech() {
        // Simulated speech: moderate energy, longer duration
        let mut samples = vec![0.0; 4800]; // 300ms
        for (i, s) in samples.iter_mut().enumerate() {
            *s = 0.3 * (i as f32 / 100.0 * std::f32::consts::TAU).sin();
        }
        let segment = AudioSegment {
            timestamp: Instant::now(),
            samples,
            duration_ms: 300,
            source: AudioSource::Microphone,
        };
        let events = MicrophoneAdapter::segment_to_observation(&segment, &[]);

        assert_eq!(events.modality, Modality::Audio);
        assert_eq!(events.source, "audio:microphone");
        // No events passed, so confidence should be 0.5
        assert!((events.confidence - 0.5).abs() < 0.01);
        assert!(events.data["rms_energy"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn test_subscribe_does_not_panic() {
        // Verify that subscribe() can be called without panicking in
        // environments without microphone hardware.  The spawned task
        // should fail gracefully (log warning, exit).
        let adapter = MicrophoneAdapter::new(AudioAdapterConfig::default());
        let _rx = adapter.subscribe();
        // If we reach here, subscribe() did not panic.
    }
}
