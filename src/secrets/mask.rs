//! Canonical secret masking for settings/config describe paths.
//!
//! Before this module, each surface that reported secrets back to a client
//! (WS `config.get`, the gateway tool, REST handlers) rolled its own masking —
//! and some leaked plaintext entirely. `mask_json_value` is the single walker
//! that makes any `serde_json::Value` describing gateway configuration safe to
//! return:
//!
//! - object keys recognized as secrets (`is_secret_key`) have their string
//!   values masked;
//! - object keys recognized as secret *containers* (`is_secret_container_key`,
//!   e.g. `credentials` / `keys` / `api_keys`) have every string leaf masked;
//! - `env` maps (MCP server env vars) are matched per key so identifiers and
//!   URLs (`HOST`, `PORT`, `BASE_URL`) stay readable while `*_KEY` / `*_TOKEN`
//!   values are masked;
//! - everything else passes through unchanged.

use serde_json::Value;

use crate::secrets::SENSITIVE_CHANNEL_CREDENTIALS;

/// Mask a secret value for display: keep the first 3 and last 4 characters,
/// hide the rest. Short non-empty keys are fully masked; empty values stay
/// empty (an unset secret must not look set).
pub fn mask_secret(value: &str) -> String {
    let k = value.trim();
    if k.is_empty() {
        return String::new();
    }
    if k.len() <= 6 {
        return "••••".to_string();
    }
    let head = k.get(..3).unwrap_or("");
    let tail = k.get(k.len() - 4..).unwrap_or("");
    format!("{head}••••{tail}")
}

/// Whether an object key names a secret value.
///
/// Matches the shared channel-credential registry plus the gateway config
/// secret field names, or a case-insensitive trailing `_key` / `_token` /
/// `_secret` / `_password` (or the bare `key` / `token` / `secret` /
/// `password`). The heuristic is deliberately conservative so non-secrets like
/// `max_tokens`, `token_url`, `key_path`, `client_id` or `model` never match.
pub fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    if SENSITIVE_CHANNEL_CREDENTIALS.contains(&lower.as_str()) {
        return true;
    }
    // `api_keys` is a list of API keys (also a secret container below).
    if matches!(lower.as_str(), "api_keys") {
        return true;
    }
    if lower.ends_with("_key")
        || lower.ends_with("_token")
        || lower.ends_with("_secret")
        || lower.ends_with("_password")
    {
        return true;
    }
    matches!(lower.as_str(), "key" | "token" | "secret" | "password")
}

/// Whether an object key is a *container* whose entire payload is secret (so
/// every string leaf is masked, regardless of the leaf key name).
///
/// `env` is intentionally excluded — MCP env maps mix secrets and non-secrets,
/// so they are matched per key instead.
pub fn is_secret_container_key(key: &str) -> bool {
    matches!(key.to_ascii_lowercase().as_str(), "keys" | "credentials" | "api_keys")
}

/// Recursively mask every string leaf in the subtree (used for secret
/// containers, where the leaf keys carry no additional meaning).
pub fn mask_secret_container_payload(value: &Value) -> Value {
    mask_all_string_leaves(value)
}

/// Recursively mask every string leaf in the subtree (used for secret
/// containers, where the leaf keys carry no additional meaning).
fn mask_all_string_leaves(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), mask_all_string_leaves(v)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(mask_all_string_leaves).collect()),
        Value::String(s) => Value::String(mask_secret(s)),
        other => other.clone(),
    }
}

/// Mask an `env` map: mask string values whose key looks like a secret, keep
/// identifiers and URLs (e.g. `HOST`, `PORT`, `BASE_URL`) readable.
fn mask_env_map(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let nv = if is_secret_key(k) {
                        match v {
                            Value::String(s) => Value::String(mask_secret(s)),
                            other => mask_env_map(other),
                        }
                    } else {
                        mask_env_map(v)
                    };
                    (k.clone(), nv)
                })
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(mask_env_map).collect()),
        other => other.clone(),
    }
}

/// Recursively mask secrets inside a config-shaped JSON value.
///
/// Objects: secret-keyed strings are masked, secret containers have all string
/// leaves masked, `env` maps are matched per key, everything else recurses.
/// Arrays recurse. Scalars pass through.
pub fn mask_json_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let nv = if is_secret_container_key(k) {
                        mask_all_string_leaves(v)
                    } else if k.eq_ignore_ascii_case("env") {
                        mask_env_map(v)
                    } else if is_secret_key(k) {
                        match v {
                            Value::String(s) => Value::String(mask_secret(s)),
                            other => mask_json_value(other),
                        }
                    } else {
                        mask_json_value(v)
                    };
                    (k.clone(), nv)
                })
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(mask_json_value).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_keeps_prefix_suffix() {
        assert_eq!(mask_secret("sk-1234567890abcd"), "sk-••••abcd");
        assert_eq!(mask_secret("abcdef"), "••••"); // short non-empty
        assert_eq!(mask_secret("abc"), "••••");
        assert_eq!(mask_secret("  sk-1234567890abcd  "), "sk-••••abcd"); // trimmed
        assert_eq!(mask_secret(""), ""); // unset stays unset
    }

    #[test]
    fn is_secret_key_matches_exact_and_suffix() {
        for key in [
            "api_key",
            "API_KEY",
            "client_secret",
            "shared_token",
            "signing_key",
            "bot_token",
            "refresh_token",
            "access_token",
            "api_password",
            "webhook_verify_token",
            "app_secret",
            "account",
            "password",
            "token",
            "secret",
            "key",
        ] {
            assert!(is_secret_key(key), "expected {key} to be a secret key");
        }
    }

    #[test]
    fn is_secret_key_rejects_innocent_keys() {
        for key in [
            "model",
            "model_provider",
            "default_agent",
            "max_tokens", // trailing 's' — must not match "_token"
            "max_turns",
            "temperature",
            "system_prompt",
            "workspace_only",
            "token_url",
            "key_path",
            "credential_precedence",
            "allow_credentials",
            "client_id",
            "app_id",
            "provider",
            "base_url",
            "timeout",
            "rate_limit",
            "enabled",
        ] {
            assert!(!is_secret_key(key), "expected {key} to NOT be a secret key");
        }
    }

    #[test]
    fn is_secret_container_key_recognizes_containers_only() {
        for key in ["keys", "credentials", "api_keys"] {
            assert!(is_secret_container_key(key));
        }
        for key in [
            "env",
            "channels",
            "providers",
            "agents",
            "agent_models",
            "servers",
        ] {
            assert!(!is_secret_container_key(key));
        }
    }

    #[test]
    fn mask_json_value_masks_provider_api_key_and_list() {
        let v = serde_json::json!({
            "providers": {
                "anthropic": {
                    "api_key": "sk-ant-abc123",
                    "api_keys": ["sk-1", "sk-2-secret"],
                    "base_url": "https://api.example.com",
                }
            }
        });
        let out = mask_json_value(&v);
        let provider = &out["providers"]["anthropic"];
        assert_eq!(provider["api_key"], "sk-••••c123");
        assert_eq!(provider["api_keys"][0], "••••");
        assert_eq!(provider["api_keys"][1], "sk-••••cret");
        assert_eq!(provider["base_url"], "https://api.example.com");
    }

    #[test]
    fn mask_json_value_masks_channel_credentials() {
        let v = serde_json::json!({
            "channels": {
                "tg": {
                    "enabled": true,
                    "credentials": { "bot_token": "123456:ABC", "app_id": "987654" }
                }
            }
        });
        let out = mask_json_value(&v);
        assert_eq!(out["channels"]["tg"]["credentials"]["bot_token"], "123••••:ABC");
        // Identifiers inside a credentials container are masked too — the
        // container is treated as an all-secret payload.
        assert_eq!(out["channels"]["tg"]["credentials"]["app_id"], "••••");
        assert_eq!(out["channels"]["tg"]["enabled"], true);
    }

    #[test]
    fn mask_json_value_masks_search_api_key_and_keys() {
        let v = serde_json::json!({
            "search": {
                "provider": "tavily",
                "api_key": "tvly-abc123",
                "keys": { "tavily": "tvly-abc123", "brave": "bsa-xyz" }
            }
        });
        let out = mask_json_value(&v);
        assert_eq!(out["search"]["api_key"], "tvl••••c123");
        assert_eq!(out["search"]["keys"]["tavily"], "tvl••••c123");
        assert_eq!(out["search"]["keys"]["brave"], "bsa••••-xyz");
        assert_eq!(out["search"]["provider"], "tavily");
    }

    #[test]
    fn mask_json_value_env_is_per_key() {
        let v = serde_json::json!({
            "servers": {
                "s": { "env": { "ANTHROPIC_API_KEY": "sk-ant-zzz", "HOST": "localhost", "PORT": "8080", "BASE_URL": "https://x" } }
            }
        });
        let out = mask_json_value(&v);
        assert_eq!(out["servers"]["s"]["env"]["ANTHROPIC_API_KEY"], "sk-••••-zzz");
        assert_eq!(out["servers"]["s"]["env"]["HOST"], "localhost");
        assert_eq!(out["servers"]["s"]["env"]["PORT"], "8080");
        assert_eq!(out["servers"]["s"]["env"]["BASE_URL"], "https://x");
    }

    #[test]
    fn mask_json_value_masks_security_but_not_credential_precedence() {
        let v = serde_json::json!({
            "security": {
                "shared_token": "sh-abc",
                "credential_precedence": "env_first",
                "rate_limit": { "max_per_minute": 10 }
            },
            "oauth": { "client_secret": "cs-secret", "client_id": "cid" }
        });
        let out = mask_json_value(&v);
        assert_eq!(out["security"]["shared_token"], "••••"); // len 6 → fully masked
                                                             // The critical negative: a string whose key merely contains
                                                             // "credential" must not be masked.
        assert_eq!(out["security"]["credential_precedence"], "env_first");
        assert_eq!(out["security"]["rate_limit"]["max_per_minute"], 10);
        assert_eq!(out["oauth"]["client_secret"], "cs-••••cret");
        assert_eq!(out["oauth"]["client_id"], "cid");
    }

    #[test]
    fn mask_json_value_preserves_innocent_config() {
        let v = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "model_provider": "anthropic",
            "default_agent": { "temperature": 0.7, "max_tokens": 4096, "workspace_only": false },
            "heartbeat": { "enabled": true, "interval_seconds": 3600 },
            "agents": { "default": { "token_url": "https://idp", "key_path": "/tmp/k" } }
        });
        assert_eq!(mask_json_value(&v), v);
    }
}
