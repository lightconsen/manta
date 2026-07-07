//! Configuration for the trajectory reflection (nudge) pattern.
//!
//! Controls the nudge engine — periodic background review of conversation
//! trajectories for interaction pattern discovery and memory persistence.

use serde::{Deserialize, Serialize};

use super::types::QualityCriteria;

/// Configuration for the nudge-based trajectory reflection engine.
///
/// The nudge engine runs as a background task every N turns, reviewing the
/// last M turns of conversation and writing interaction patterns to memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeConfig {
    /// Evaluate conversation trajectory every N turns (default: 10).
    #[serde(default = "default_nudge_interval")]
    pub interval: usize,
    /// Number of recent turns to include in each trajectory review (default: 5).
    #[serde(default = "default_nudge_window")]
    pub window_size: usize,
    /// Minimum turns before the first nudge fires (default: 3).
    #[serde(default = "default_nudge_min_turns")]
    pub min_turns: usize,
}

const fn default_nudge_interval() -> usize {
    10
}

const fn default_nudge_window() -> usize {
    5
}

const fn default_nudge_min_turns() -> usize {
    3
}

impl Default for NudgeConfig {
    fn default() -> Self {
        Self {
            interval: default_nudge_interval(),
            window_size: default_nudge_window(),
            min_turns: default_nudge_min_turns(),
        }
    }
}

const fn default_true() -> bool {
    true
}

/// Configuration for the trajectory reflection nudge engine.
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
    /// Enable nudge-based trajectory reflection (default: true).
    #[serde(default = "default_true")]
    pub nudge_enabled: bool,
    /// Nudge engine configuration.
    #[serde(default)]
    pub nudge: NudgeConfig,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            criteria: QualityCriteria::default(),
            critic_model: None,
            nudge_enabled: true,
            nudge: NudgeConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ReflectionConfig::default();
        assert!(config.nudge_enabled);
        assert_eq!(config.nudge.interval, 10);
    }

    #[test]
    fn test_nudge_config_defaults() {
        let nudge = NudgeConfig::default();
        assert_eq!(nudge.interval, 10);
        assert_eq!(nudge.window_size, 5);
        assert_eq!(nudge.min_turns, 3);
    }
}
