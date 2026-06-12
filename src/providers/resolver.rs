//! Provider resolver — config-to-provider dispatch.
//!
//! Resolves user configuration (preset name + overrides) into a concrete
//! protocol-level provider by merging preset defaults with user overrides.

use std::sync::Arc;

use super::preset::builtin_providers;
use super::stream_wrappers::ProviderStreamFamily;
use super::{AuthMethod, Protocol, ProviderInstanceConfig};
use super::{AnthropicProvider, GeminiProvider, OpenAiProvider, Provider};

/// Resolve a provider configuration into a concrete provider instance.
///
/// # Arguments
/// * `provider_type` — Preset name (e.g. `"openai"`, `"kimi"`, `"anthropic"`) or `"custom"`
/// * `api_key` — API key for the provider (if applicable)
/// * `base_url` — Override base URL
/// * `model` — Override model name
/// * `protocol` — Protocol override (required for `"custom"`, optional for presets)
///
/// For full control, use `resolve_from_config()`.
pub fn resolve_provider(
    provider_type: &str,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    protocol: Option<Protocol>,
) -> crate::Result<Arc<dyn Provider>> {
    let presets = builtin_providers();

    if provider_type == "custom" || !presets.contains_key(provider_type) {
        // Custom provider: protocol is required
        let proto = protocol.ok_or_else(|| {
            crate::error::ConfigError::InvalidValue {
                key: "protocol".to_string(),
                message: "Custom providers require an explicit protocol".to_string(),
            }
        })?;
        let instance = resolve_custom_instance(proto, api_key, base_url, model)?;
        return create_protocol_provider(&instance);
    }

    // Look up preset
    let preset = &presets[provider_type];

    // Select variant: by protocol override or default (first variant)
    let variant = match protocol {
        Some(p) => preset
            .variants
            .iter()
            .find(|v| v.protocol == p)
            .ok_or_else(|| {
                let available: Vec<String> =
                    preset.variants.iter().map(|v| format!("{:?}", v.protocol)).collect();
                crate::error::ConfigError::InvalidValue {
                    key: "protocol".to_string(),
                    message: format!(
                        "Provider '{}' does not support protocol '{:?}'. Available: {}",
                        provider_type,
                        p,
                        available.join(", "),
                    ),
                }
            })?,
        None => &preset.variants[0],
    };

    let instance = ProviderInstanceConfig {
        protocol: variant.protocol,
        auth_method: variant.auth_method.clone(),
        api_key,
        base_url: base_url.unwrap_or_else(|| variant.default_base_url.clone()),
        model: model.unwrap_or_else(|| variant.default_model.clone()),
        max_context: variant.default_max_context,
        supports_vision: variant.default_supports_vision,
        supports_tools: variant.default_supports_tools,
        stream_family: variant.default_stream_family,
    };

    create_protocol_provider(&instance)
}

/// Resolve from a fully-specified set of parameters (used by `ModelRouter`).
///
/// Merges preset defaults with all overrides, then creates the provider.
///
/// # Arguments
/// * `provider_type` — Preset name or `"custom"`
/// * `api_key` — The effective API key
/// * `protocol` — Protocol override
/// * `base_url` — Base URL override
/// * `model` — Model override
/// * `max_context` — Max context override
/// * `supports_vision` — Vision support override
/// * `supports_tools` — Tools support override
/// * `stream_family` — Stream family override
/// * `auth_method` — Auth method override
pub fn resolve_from_config(
    provider_type: &str,
    api_key: Option<String>,
    protocol: Option<Protocol>,
    base_url: Option<String>,
    model: Option<String>,
    max_context: Option<usize>,
    supports_vision: Option<bool>,
    supports_tools: Option<bool>,
    stream_family: Option<ProviderStreamFamily>,
    auth_method: Option<AuthMethod>,
) -> crate::Result<Arc<dyn Provider>> {
    let presets = builtin_providers();

    let instance = if provider_type == "custom" || !presets.contains_key(provider_type) {
        let proto = protocol.ok_or_else(|| {
            crate::error::ConfigError::InvalidValue {
                key: "protocol".to_string(),
                message: "Custom providers require an explicit protocol".to_string(),
            }
        })?;
        ProviderInstanceConfig {
            protocol: proto,
            auth_method: auth_method.unwrap_or(AuthMethod::Bearer),
            api_key,
            base_url: base_url.clone().unwrap_or_default(),
            model: model.clone().unwrap_or_default(),
            max_context: max_context.unwrap_or(128_000),
            supports_vision: supports_vision.unwrap_or(true),
            supports_tools: supports_tools.unwrap_or(true),
            stream_family: stream_family.unwrap_or(ProviderStreamFamily::OpenAi),
        }
    } else {
        let preset = &presets[provider_type];
        let variant = match protocol {
            Some(p) => preset
                .variants
                .iter()
                .find(|v| v.protocol == p)
                .ok_or_else(|| {
                    let available: Vec<String> =
                        preset.variants.iter().map(|v| format!("{:?}", v.protocol)).collect();
                    crate::error::ConfigError::InvalidValue {
                        key: "protocol".to_string(),
                        message: format!(
                            "Provider '{}' does not support protocol '{:?}'. Available: {}",
                            provider_type, p, available.join(", "),
                        ),
                    }
                })?,
            None => &preset.variants[0],
        };

        ProviderInstanceConfig {
            protocol: variant.protocol,
            auth_method: auth_method.unwrap_or_else(|| variant.auth_method.clone()),
            api_key,
            base_url: base_url.unwrap_or_else(|| variant.default_base_url.clone()),
            model: model.unwrap_or_else(|| variant.default_model.clone()),
            max_context: max_context.unwrap_or(variant.default_max_context),
            supports_vision: supports_vision.unwrap_or(variant.default_supports_vision),
            supports_tools: supports_tools.unwrap_or(variant.default_supports_tools),
            stream_family: stream_family.unwrap_or(variant.default_stream_family),
        }
    };

    create_protocol_provider(&instance)
}

/// Resolve config for a custom (non-preset) provider.
fn resolve_custom_instance(
    protocol: Protocol,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> crate::Result<ProviderInstanceConfig> {
    let base_url = base_url.ok_or_else(|| {
        crate::error::ConfigError::InvalidValue {
            key: "base_url".to_string(),
            message: "Custom providers require a base_url".to_string(),
        }
    })?;

    Ok(ProviderInstanceConfig {
        protocol,
        auth_method: AuthMethod::Bearer,
        api_key,
        base_url,
        model: model.unwrap_or_else(|| "default".to_string()),
        max_context: 128_000,
        supports_vision: true,
        supports_tools: true,
        stream_family: ProviderStreamFamily::OpenAi,
    })
}

/// Create a protocol-level provider from a fully-resolved instance config.
fn create_protocol_provider(config: &ProviderInstanceConfig) -> crate::Result<Arc<dyn Provider>> {
    match config.protocol {
        Protocol::OpenAi => Ok(Arc::new(OpenAiProvider::from_config(config.clone())?)),
        Protocol::Anthropic => Ok(Arc::new(AnthropicProvider::from_config(config.clone())?)),
        Protocol::Gemini => Ok(Arc::new(GeminiProvider::from_config(config.clone())?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_openai_preset() {
        let provider = resolve_provider("openai", None, None, None, None).unwrap();
        assert!(provider.supports_tools());
        assert_eq!(provider.default_model(), "gpt-4o-mini");
    }

    #[test]
    fn test_resolve_anthropic_preset() {
        let provider =
            resolve_provider("anthropic", Some("sk-test".into()), None, None, None).unwrap();
        assert_eq!(provider.default_model(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_resolve_gemini_preset() {
        let provider =
            resolve_provider("gemini", Some("test-key".into()), None, None, None).unwrap();
        assert_eq!(provider.default_model(), "gemini-2.0-flash");
    }

    #[test]
    fn test_resolve_kimi_default_variant() {
        // Default Kimi should use OpenAI protocol (first variant)
        let provider = resolve_provider("kimi", Some("sk-test".into()), None, None, None).unwrap();
        assert_eq!(provider.default_model(), "kimi-k2");
    }

    #[test]
    fn test_resolve_kimi_anthropic_variant() {
        // Explicitly choose Kimi's Anthropic variant
        let provider = resolve_provider(
            "kimi",
            Some("sk-test".into()),
            None,
            None,
            Some(Protocol::Anthropic),
        )
        .unwrap();
        assert_eq!(provider.default_model(), "kimi-k2");
    }

    #[test]
    fn test_resolve_with_model_override() {
        let provider = resolve_provider(
            "openai",
            None,
            None,
            Some("gpt-4".into()),
            None,
        )
        .unwrap();
        assert_eq!(provider.default_model(), "gpt-4");
    }

    #[test]
    fn test_resolve_custom_provider() {
        let provider = resolve_provider(
            "custom",
            Some("sk-test".into()),
            Some("https://api.example.com/v1".into()),
            Some("my-model".into()),
            Some(Protocol::OpenAi),
        )
        .unwrap();
        assert_eq!(provider.default_model(), "my-model");
    }

    #[test]
    fn test_resolve_custom_without_protocol_fails() {
        let result = resolve_provider(
            "custom",
            Some("sk-test".into()),
            Some("https://api.example.com/v1".into()),
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_custom_without_base_url_fails() {
        let result = resolve_provider("custom", Some("sk-test".into()), None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_ollama_preset() {
        let provider = resolve_provider("ollama", None, None, None, None).unwrap();
        assert_eq!(provider.default_model(), "llama3.2");
    }

    #[test]
    fn test_resolve_minimax_preset() {
        let provider =
            resolve_provider("minimax", Some("sk-test".into()), None, None, None).unwrap();
        assert_eq!(provider.default_model(), "abab6.5s-chat");
    }

    #[test]
    fn test_resolve_invalid_protocol_for_preset_fails() {
        let result = resolve_provider(
            "ollama",
            None,
            None,
            None,
            Some(Protocol::Anthropic),
        );
        assert!(result.is_err());
    }
}
