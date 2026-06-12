//! Built-in provider presets.
//!
//! Each preset defines a known LLM vendor with its protocol variants (endpoints).
//! A single vendor may expose multiple protocols (e.g. Kimi supports both
//! OpenAI-compatible and Anthropic-compatible endpoints).

use std::collections::HashMap;

use super::stream_wrappers::ProviderStreamFamily;
use super::{AuthMethod, Protocol, ProtocolVariant, ProviderDefinition};

/// All built-in provider definitions.
///
/// The key is the preset name used in config (e.g. `"openai"`, `"kimi"`).
pub fn builtin_providers() -> HashMap<&'static str, ProviderDefinition> {
    let mut m = HashMap::new();

    // ── OpenAI ──
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

    // ── DeepSeek ──
    m.insert(
        "deepseek",
        ProviderDefinition {
            name: "deepseek".into(),
            display_name: "DeepSeek".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::OpenAi,
                default_base_url: "https://api.deepseek.com/v1".into(),
                default_model: "deepseek-chat".into(),
                auth_method: AuthMethod::Bearer,
                default_max_context: 128_000,
                default_supports_vision: false,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::OpenAi,
            }],
        },
    );

    // ── Ollama (local) ──
    m.insert(
        "ollama",
        ProviderDefinition {
            name: "ollama".into(),
            display_name: "Ollama".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::OpenAi,
                default_base_url: "http://localhost:11434/v1".into(),
                default_model: "llama3.2".into(),
                auth_method: AuthMethod::None,
                default_max_context: 8_192,
                default_supports_vision: false,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::OpenAi,
            }],
        },
    );

    // ── Qwen (Alibaba Cloud) ──
    m.insert(
        "qwen",
        ProviderDefinition {
            name: "qwen".into(),
            display_name: "Qwen".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::OpenAi,
                default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
                default_model: "qwen-max".into(),
                auth_method: AuthMethod::Bearer,
                default_max_context: 128_000,
                default_supports_vision: true,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::OpenAi,
            }],
        },
    );

    // ── Kimi (Moonshot): supports both OpenAI and Anthropic protocols ──
    m.insert(
        "kimi",
        ProviderDefinition {
            name: "kimi".into(),
            display_name: "Moonshot (Kimi)".into(),
            variants: vec![
                ProtocolVariant {
                    protocol: Protocol::OpenAi,
                    default_base_url: "https://api.moonshot.cn/v1".into(),
                    default_model: "kimi-k2".into(),
                    auth_method: AuthMethod::Bearer,
                    default_max_context: 128_000,
                    default_supports_vision: true,
                    default_supports_tools: true,
                    default_stream_family: ProviderStreamFamily::Moonshot,
                },
                ProtocolVariant {
                    protocol: Protocol::Anthropic,
                    default_base_url: "https://api.moonshot.cn/anthropic".into(),
                    default_model: "kimi-k2".into(),
                    auth_method: AuthMethod::ApiKeyHeader,
                    default_max_context: 128_000,
                    default_supports_vision: true,
                    default_supports_tools: true,
                    default_stream_family: ProviderStreamFamily::Anthropic,
                },
            ],
        },
    );

    // ── Anthropic ──
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

    // ── Azure OpenAI ──
    m.insert(
        "azure",
        ProviderDefinition {
            name: "azure".into(),
            display_name: "Azure OpenAI".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::OpenAi,
                default_base_url: "https://YOUR_RESOURCE.openai.azure.com".into(),
                default_model: "gpt-4o".into(),
                auth_method: AuthMethod::Bearer,
                default_max_context: 128_000,
                default_supports_vision: true,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::OpenAi,
            }],
        },
    );

    // ── Gemini ──
    m.insert(
        "gemini",
        ProviderDefinition {
            name: "gemini".into(),
            display_name: "Gemini".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::Gemini,
                default_base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
                default_model: "gemini-2.0-flash".into(),
                auth_method: AuthMethod::GoogleApiKey,
                default_max_context: 1_048_576,
                default_supports_vision: true,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::GoogleThinking,
            }],
        },
    );

    // ── MiniMax ──
    m.insert(
        "minimax",
        ProviderDefinition {
            name: "minimax".into(),
            display_name: "MiniMax".into(),
            variants: vec![ProtocolVariant {
                protocol: Protocol::OpenAi,
                default_base_url: "https://api.minimax.chat/v1".into(),
                default_model: "abab6.5s-chat".into(),
                auth_method: AuthMethod::Bearer,
                default_max_context: 128_000,
                default_supports_vision: false,
                default_supports_tools: true,
                default_stream_family: ProviderStreamFamily::Minimax,
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
