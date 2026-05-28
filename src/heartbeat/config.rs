use serde::{Deserialize, Serialize};

/// Heartbeat scheduler configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HeartbeatConfig {
    /// Enable heartbeat scheduler
    pub enabled: bool,
    /// Interval between heartbeats in seconds (default: 300 = 5 min)
    pub interval_seconds: u64,
    /// Active hours start time, e.g., "08:00"
    pub active_hours_start: String,
    /// Active hours end time, e.g., "23:00"
    pub active_hours_end: String,
    /// Stop after N consecutive idle heartbeats (default: 12)
    pub max_consecutive_idle: u32,
    /// Override model for heartbeat LLM calls
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Override provider for heartbeat LLM calls
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 300,
            active_hours_start: "08:00".to_string(),
            active_hours_end: "23:00".to_string(),
            max_consecutive_idle: 12,
            model: None,
            provider: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_heartbeat_config() {
        let config = HeartbeatConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.interval_seconds, 300);
        assert_eq!(config.active_hours_start, "08:00");
        assert_eq!(config.active_hours_end, "23:00");
        assert_eq!(config.max_consecutive_idle, 12);
        assert!(config.model.is_none());
        assert!(config.provider.is_none());
    }

    #[test]
    fn test_heartbeat_config_serde_roundtrip() {
        let config = HeartbeatConfig::default();
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: HeartbeatConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.interval_seconds, deserialized.interval_seconds);
        assert_eq!(config.active_hours_start, deserialized.active_hours_start);
        assert_eq!(config.active_hours_end, deserialized.active_hours_end);
        assert_eq!(config.max_consecutive_idle, deserialized.max_consecutive_idle);
    }

    #[test]
    fn test_heartbeat_config_serde_with_model() {
        let config = HeartbeatConfig {
            enabled: true,
            interval_seconds: 60,
            model: Some("claude-haiku-4-5-20251001".to_string()),
            provider: Some("anthropic".to_string()),
            ..Default::default()
        };
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains("claude-haiku"));
        assert!(serialized.contains("anthropic"));
        let deserialized: HeartbeatConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(config.model, deserialized.model);
        assert_eq!(config.provider, deserialized.provider);
    }

    #[test]
    fn test_heartbeat_config_deserialize_partial() {
        // Should accept empty object with defaults
        let config: HeartbeatConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.enabled);
        assert_eq!(config.interval_seconds, 300);
    }
}
