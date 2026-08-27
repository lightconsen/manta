//! Shared, pure config-path application.
//!
//! `apply_config_path` mutates a [`GatewayConfig`] in place for a dot-path
//! (e.g. `default_agent.temperature`). It is the single source of truth for
//! these paths, shared by the WS `config.set` handler and the harness scalar
//! optimizer so both mutate the live config identically.

use serde_json::Value;

use crate::gateway::GatewayConfig;

/// Read the current value of a scalar config path as a `f64`, if the path is
/// one this module knows. Pairs with [`apply_config_path`] so the optimizer can
/// probe the same path set it is allowed to mutate.
pub fn read_config_scalar(config: &GatewayConfig, path: &str) -> Option<f64> {
    match path {
        "default_agent.temperature" => Some(config.default_agent.temperature as f64),
        "default_agent.max_tokens" => Some(config.default_agent.max_tokens as f64),
        "default_agent.max_turns" => config.default_agent.max_turns.map(|v| v as f64),
        "default_agent.max_concurrent_tools" => {
            Some(config.default_agent.max_concurrent_tools as f64)
        }
        "default_agent.max_context_tokens" => Some(config.default_agent.max_context_tokens as f64),
        _ => None,
    }
}

/// Apply a config path to `config` in place.
///
/// Returns `true` if the path was recognized and applied, `false` if it is
/// not a path this module knows (callers should then try their own handling or
/// report an unknown path).
///
/// Invalid value types are silently ignored (matching the WS handler's
/// behavior for scalar paths).
pub fn apply_config_path(config: &mut GatewayConfig, path: &str, value: &Value) -> bool {
    match path {
        "default_agent.temperature" => {
            if let Some(v) = value.as_f64() {
                config.default_agent.temperature = v as f32;
            }
            true
        }
        "default_agent.max_tokens" => {
            if let Some(v) = value.as_u64() {
                config.default_agent.max_tokens = v as u32;
            }
            true
        }
        "default_agent.max_turns" => {
            config.default_agent.max_turns = value.as_u64().map(|v| v as usize);
            true
        }
        "default_agent.max_concurrent_tools" => {
            if let Some(v) = value.as_u64() {
                config.default_agent.max_concurrent_tools = v as usize;
            }
            true
        }
        "default_agent.max_context_tokens" => {
            if let Some(v) = value.as_u64() {
                config.default_agent.max_context_tokens = v as usize;
            }
            true
        }
        "default_agent.system_prompt" => {
            if let Some(v) = value.as_str() {
                config.default_agent.system_prompt = v.to_string();
            }
            true
        }
        "default_agent.workspace_only" => {
            if let Some(v) = value.as_bool() {
                config.default_agent.workspace_only = v;
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::GatewayConfig;

    #[test]
    fn applies_known_scalar_paths() {
        let mut cfg = GatewayConfig::default();

        assert!(apply_config_path(&mut cfg, "default_agent.temperature", &Value::from(0.9)));
        assert!((cfg.default_agent.temperature - 0.9).abs() < 1e-6);

        assert!(apply_config_path(&mut cfg, "default_agent.max_tokens", &Value::from(4096u64)));
        assert_eq!(cfg.default_agent.max_tokens, 4096);

        assert!(apply_config_path(&mut cfg, "default_agent.max_turns", &Value::from(10u64)));
        assert_eq!(cfg.default_agent.max_turns, Some(10));

        assert!(apply_config_path(
            &mut cfg,
            "default_agent.max_context_tokens",
            &Value::from(32000u64)
        ));
        assert_eq!(cfg.default_agent.max_context_tokens, 32000);
    }

    #[test]
    fn unknown_path_rejected() {
        let mut cfg = GatewayConfig::default();
        assert!(!apply_config_path(&mut cfg, "no.such.path", &Value::from(1)));
    }
}
