//! Command provider/model inference.
//!
//! Maps a command definition, the caller's permission level, and optional
//! channel capabilities to a provider/model hint. The hint is advisory:
//! callers can use it to pick a fast/cheap model for simple commands or a
//! capable model for power/admin commands.

use serde::{Deserialize, Serialize};

use crate::channels::ChannelCapabilities;
use crate::gateway::commands::{CommandCategory, CommandDef, CommandTier};
use crate::tools::command_gate::UserLevel;

/// An inferred provider/model hint for a command invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandProviderHint {
    /// Suggested provider identifier (e.g. "anthropic", "openai").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Suggested model identifier or alias (e.g. "fast", "power",
    /// "claude-3-sonnet").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Human-readable reason for the hint.
    pub reason: String,
}

impl CommandProviderHint {
    /// Create a hint with only a model alias.
    pub fn model(model: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            provider: None,
            model: Some(model.into()),
            reason: reason.into(),
        }
    }

    /// Create a hint with a provider and model.
    pub fn provider_model(
        provider: impl Into<String>,
        model: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            provider: Some(provider.into()),
            model: Some(model.into()),
            reason: reason.into(),
        }
    }
}

/// Infers the best provider/model for a command.
#[derive(Debug, Clone, Default)]
pub struct CommandProviderResolver;

impl CommandProviderResolver {
    /// Create a new resolver.
    pub fn new() -> Self {
        Self
    }

    /// Resolve a provider hint for `def`.
    ///
    /// Returns `None` when the default model is appropriate.
    pub fn resolve(
        def: &CommandDef,
        user_level: UserLevel,
        caps: Option<&ChannelCapabilities>,
    ) -> Option<CommandProviderHint> {
        // Explicit per-command hint takes precedence.
        if let Some(hint) = &def.provider_hint {
            return Some(hint.clone());
        }

        // If the channel cannot run commands at all, only essential commands
        // receive a hint (and callers should still gate execution).
        if let Some(caps) = caps {
            if !caps.supports_commands && def.tier != CommandTier::Essential {
                return None;
            }
        }

        // Low-trust users are steered toward the fast/cheap model.
        if user_level == UserLevel::Chat {
            return Some(CommandProviderHint::model(
                "fast",
                format!("chat-only user steered to fast model for /{}", def.key),
            ));
        }

        match (def.category, def.tier) {
            // Admin and power-tier commands benefit from the capable model.
            (CommandCategory::Admin, _) | (_, CommandTier::Power) => {
                Some(CommandProviderHint::model(
                    "power",
                    format!("admin/power command /{} uses the power model", def.key),
                ))
            }
            // Essential status/session commands are cheap to answer.
            (CommandCategory::Session | CommandCategory::Status, CommandTier::Essential) => {
                Some(CommandProviderHint::model(
                    "fast",
                    format!("essential /{} command uses the fast model", def.key),
                ))
            }
            // Everything else uses the default selection.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(cat: CommandCategory, tier: CommandTier) -> CommandDef {
        CommandDef {
            key: "test".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            args: None,
            category: cat,
            tier,
            local: false,
            requires_admin: false,
            aliases: Vec::new(),
            scope: crate::gateway::commands::CommandScope::Global,
            provider_hint: None,
        }
    }

    fn def_with_hint(hint: CommandProviderHint) -> CommandDef {
        CommandDef {
            key: "echo".to_string(),
            name: "echo".to_string(),
            description: "echo".to_string(),
            args: None,
            category: CommandCategory::Status,
            tier: CommandTier::Essential,
            local: false,
            requires_admin: false,
            aliases: Vec::new(),
            scope: crate::gateway::commands::CommandScope::Global,
            provider_hint: Some(hint),
        }
    }

    #[test]
    fn test_explicit_hint_wins() {
        let hint = CommandProviderHint::provider_model("openai", "gpt-4o", "explicit");
        let def = def_with_hint(hint.clone());
        assert_eq!(CommandProviderResolver::resolve(&def, UserLevel::User, None), Some(hint));
    }

    #[test]
    fn test_power_command_uses_power_model() {
        let def = def(CommandCategory::Admin, CommandTier::Power);
        let hint = CommandProviderResolver::resolve(&def, UserLevel::Admin, None).unwrap();
        assert_eq!(hint.model.as_deref(), Some("power"));
    }

    #[test]
    fn test_essential_status_uses_fast_model() {
        let def = def(CommandCategory::Status, CommandTier::Essential);
        let hint = CommandProviderResolver::resolve(&def, UserLevel::User, None).unwrap();
        assert_eq!(hint.model.as_deref(), Some("fast"));
    }

    #[test]
    fn test_chat_user_steered_to_fast() {
        let def = def(CommandCategory::Tools, CommandTier::Standard);
        let hint = CommandProviderResolver::resolve(&def, UserLevel::Chat, None).unwrap();
        assert_eq!(hint.model.as_deref(), Some("fast"));
    }

    #[test]
    fn test_standard_command_no_hint() {
        let def = def(CommandCategory::Agents, CommandTier::Standard);
        assert!(CommandProviderResolver::resolve(&def, UserLevel::User, None).is_none());
    }

    #[test]
    fn test_channel_without_commands_suppresses_non_essential() {
        let mut caps = ChannelCapabilities::default();
        caps.supports_commands = false;
        let standard = def(CommandCategory::Agents, CommandTier::Standard);
        assert!(CommandProviderResolver::resolve(&standard, UserLevel::User, Some(&caps)).is_none());

        let essential = def(CommandCategory::Status, CommandTier::Essential);
        assert!(
            CommandProviderResolver::resolve(&essential, UserLevel::User, Some(&caps)).is_some()
        );
    }
}
