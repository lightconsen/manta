//! Configuration for the trajectory reflection (retrospect) pattern.
//!
//! Controls the retrospect engine — periodic background review of conversation
//! trajectories for interaction pattern discovery and memory persistence.

use serde::{Deserialize, Serialize};

use super::types::QualityCriteria;

/// Configuration for the retrospect-based trajectory reflection engine.
///
/// The retrospect engine runs as a background task every N turns, reviewing the
/// last M turns of conversation and writing interaction patterns to memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrospectConfig {
    /// Evaluate conversation trajectory every N turns (default: 10).
    #[serde(default = "default_retrospect_interval")]
    pub interval: usize,
    /// Number of recent turns to include in each trajectory review (default:
    /// 5).
    #[serde(default = "default_retrospect_window")]
    pub window_size: usize,
    /// Minimum turns before the first retrospect fires (default: 3).
    #[serde(default = "default_retrospect_min_turns")]
    pub min_turns: usize,
}

const fn default_retrospect_interval() -> usize {
    10
}

const fn default_retrospect_window() -> usize {
    5
}

const fn default_retrospect_min_turns() -> usize {
    3
}

impl Default for RetrospectConfig {
    fn default() -> Self {
        Self {
            interval: default_retrospect_interval(),
            window_size: default_retrospect_window(),
            min_turns: default_retrospect_min_turns(),
        }
    }
}

const fn default_true() -> bool {
    true
}

/// Configuration for the trajectory reflection retrospect engine.
///
/// When enabled, a background task periodically reviews the last N turns of
/// conversation and writes interaction patterns to memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// Quality criteria for evaluating trajectories.
    #[serde(default)]
    pub criteria: QualityCriteria,
    /// Optional model to use for the critic.
    /// When `None`, the agent's own provider/model is used.
    #[serde(default)]
    pub critic_model: Option<String>,
    /// Enable retrospect-based trajectory reflection (default: true).
    #[serde(default = "default_true")]
    pub retrospect_enabled: bool,
    /// Retrospect engine configuration.
    #[serde(default)]
    pub retrospect: RetrospectConfig,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            criteria: QualityCriteria::default(),
            critic_model: None,
            retrospect_enabled: true,
            retrospect: RetrospectConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ReflectionConfig::default();
        assert!(config.retrospect_enabled);
        assert_eq!(config.retrospect.interval, 10);
    }

    #[test]
    fn test_retrospect_config_defaults() {
        let retrospect = RetrospectConfig::default();
        assert_eq!(retrospect.interval, 10);
        assert_eq!(retrospect.window_size, 5);
        assert_eq!(retrospect.min_turns, 3);
    }
}
