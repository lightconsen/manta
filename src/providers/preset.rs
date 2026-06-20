//! Built-in provider presets.
//!
//! Each preset defines a known LLM vendor with its protocol variants (endpoints).
//! A single vendor may expose multiple protocols (e.g. Kimi supports both
//! OpenAI-compatible and Anthropic-compatible endpoints).
//!
//! The vendor list lives in `presets.toml` (embedded at compile time via
//! [`include_str!`]) so adding a new vendor is a data-only change. If the
//! embedded TOML ever fails to parse, [`builtin_providers`] logs the error
//! and falls back to a minimal hand-rolled set (OpenAI + Anthropic) so the
//! system still boots.

use std::collections::HashMap;

use serde::Deserialize;

use super::stream_wrappers::ProviderStreamFamily;
use super::{AuthMethod, Protocol, ProtocolVariant, ProviderDefinition};

/// Embedded preset table, parsed once per call.
const PRESETS_TOML: &str = include_str!("presets.toml");

/// A preset entry as stored in `presets.toml`. The provider `name` is
/// supplied by the table key, so it is not repeated in the file.
#[derive(Debug, Deserialize)]
struct RawPreset {
    display_name: String,
    variants: Vec<ProtocolVariant>,
}

/// All built-in provider definitions.
///
/// The key is the preset name used in config (e.g. `"openai"`, `"kimi"`).
pub fn builtin_providers() -> HashMap<&'static str, ProviderDefinition> {
    match toml::from_str::<HashMap<String, RawPreset>>(PRESETS_TOML) {
        Ok(raw) => raw
            .into_iter()
            .map(|(name, p)| {
                let def = ProviderDefinition {
                    name: name.clone(),
                    display_name: p.display_name,
                    variants: p.variants,
                };
                // Leak the key to obtain a `&'static str`. The preset set is
                // small and built once; this keeps the long-standing
                // `&'static str` key contract without churn at call sites.
                (Box::leak(name.into_boxed_str()) as &'static str, def)
            })
            .collect(),
        Err(e) => {
            tracing::error!(
                "failed to parse embedded provider presets.toml ({e}); \
                 falling back to minimal hand-rolled set"
            );
            fallback_providers()
        }
    }
}

/// Minimal hand-rolled provider set used only if `presets.toml` fails to
/// parse. Keeps the gateway bootable with the two most common vendors.
fn fallback_providers() -> HashMap<&'static str, ProviderDefinition> {
    let mut m = HashMap::new();
    m.insert(
        "openai",
        ProviderDefinition {
            name: "openai".into(),
            display_name: "OpenAI".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::OpenAi,
                default_base_url: "https://api.openai.com/v1".into(),
                default_model: "gpt-4o-mini".into(),
                auth_method: AuthMethod::Bearer,
                default_max_context: 128_000,
                default_supports_vision: true,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::OpenAi,
            }],
        },
    );
    m.insert(
        "anthropic",
        ProviderDefinition {
            name: "anthropic".into(),
            display_name: "Anthropic".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::Anthropic,
                default_base_url: "https://api.anthropic.com".into(),
                default_model: "claude-sonnet-4-20250514".into(),
                auth_method: AuthMethod::ApiKeyHeader,
                default_max_context: 200_000,
                default_supports_vision: true,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::Anthropic,
            }],
        },
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_providers_contains_expected() {
        let providers = builtin_providers();
        assert!(providers.contains_key("openai"));
        assert!(providers.contains_key("anthropic"));
        assert!(providers.contains_key("gemini"));
        assert!(providers.contains_key("kimi"));
        assert!(providers.contains_key("deepseek"));
        assert!(providers.contains_key("ollama"));
        assert!(providers.contains_key("qwen"));
        assert!(providers.contains_key("minimax"));
        assert!(providers.contains_key("azure"));
    }

    #[test]
    fn test_embedded_toml_parses() {
        // If the embedded TOML is malformed we would fall back to 2 entries;
        // the full set proves the file parsed cleanly.
        let providers = builtin_providers();
        assert!(
            providers.len() >= 9,
            "expected full preset set from presets.toml, got {}",
            providers.len()
        );
    }

    #[test]
    fn test_names_match_keys() {
        let providers = builtin_providers();
        for (key, def) in &providers {
            assert_eq!(*key, def.name, "preset key must match definition name");
        }
    }

    #[test]
    fn test_kimi_has_two_variants() {
        let providers = builtin_providers();
        let kimi = providers.get("kimi").unwrap();
        assert_eq!(kimi.variants.len(), 2);
        assert_eq!(kimi.variants[0].protocol, Protocol::OpenAi);
        assert_eq!(kimi.variants[1].protocol, Protocol::Anthropic);
    }

    #[test]
    fn test_ollama_no_auth() {
        let providers = builtin_providers();
        let ollama = providers.get("ollama").unwrap();
        assert_eq!(ollama.variants[0].auth_method, AuthMethod::None);
    }

    #[test]
    fn test_gemini_uses_google_auth() {
        let providers = builtin_providers();
        let gemini = providers.get("gemini").unwrap();
        assert_eq!(gemini.variants[0].auth_method, AuthMethod::GoogleApiKey);
    }

    #[test]
    fn test_each_provider_has_at_least_one_variant() {
        let providers = builtin_providers();
        for (name, def) in &providers {
            assert!(!def.variants.is_empty(), "{} has no variants", name);
        }
    }
}
