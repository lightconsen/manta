//! Auto-reply Dispatch
//!
//! The dispatch layer is the orchestration heart of the inbound pipeline.
//! It applies send policies, resolves plugin-owned bindings, and decides
//! whether a message should be processed or suppressed.
//!
//! This replaces the direct "enqueue then route" logic in Gateway.

use crate::channels::IncomingMessage;
use crate::gateway::send_policy::{PolicyDecision, SendPolicy};

use super::media::MediaUnderstandingResult;

/// Result of the dispatch stage.
#[derive(Debug, Clone)]
pub struct DispatchResult {
    /// Whether the message should be suppressed (silent mode).
    pub suppress: bool,
    /// Optional workspace hint for routing.
    pub workspace_hint: Option<String>,
    /// Optional plugin-owned binding target.
    pub plugin_binding: Option<String>,
    /// Human-readable reason for suppression (if suppressed).
    pub suppress_reason: Option<String>,
}

impl DispatchResult {
    pub fn allow() -> Self {
        Self {
            suppress: false,
            workspace_hint: None,
            plugin_binding: None,
            suppress_reason: None,
        }
    }

    pub fn suppress(reason: impl Into<String>) -> Self {
        Self {
            suppress: true,
            workspace_hint: None,
            plugin_binding: None,
            suppress_reason: Some(reason.into()),
        }
    }
}

/// Configuration for the auto-reply dispatch layer.
#[derive(Debug, Clone)]
pub struct AutoReplyDispatchConfig {
    /// Send policy engine (if None, no policy checks are applied).
    pub send_policy: Option<SendPolicy>,
    /// Whether to suppress delivery in group chats unless the bot is mentioned.
    pub suppress_unless_mentioned_in_groups: bool,
}

impl Default for AutoReplyDispatchConfig {
    fn default() -> Self {
        Self {
            send_policy: None,
            suppress_unless_mentioned_in_groups: false,
        }
    }
}

/// Auto-reply dispatch orchestrator.
///
/// Stages (in order):
/// 1. Send policy evaluation (allow / deny / silence)
/// 2. Plugin-owned binding resolution
/// 3. Workspace hint extraction (`@workspace` mention)
/// 4. Suppression check (group chats without mention)
pub struct AutoReplyDispatch {
    config: AutoReplyDispatchConfig,
}

impl AutoReplyDispatch {
    pub fn new(config: AutoReplyDispatchConfig) -> Self {
        Self { config }
    }

    /// Process a message through the dispatch layer.
    ///
    /// Returns `DispatchResult::suppress(...)` if the message should not
    /// be routed to an agent.
    pub async fn process(
        &self,
        message: &IncomingMessage,
        _media_results: Option<&MediaUnderstandingResult>,
    ) -> DispatchResult {
        // Stage 1: Send policy evaluation
        if let Some(ref policy) = self.config.send_policy {
            let channel = match &message.provenance {
                crate::channels::InputProvenance::ExternalUser { channel, .. } => channel.as_str(),
                crate::channels::InputProvenance::InterSession { .. } => "inter_session",
                crate::channels::InputProvenance::InternalSystem { .. } => "internal",
            };
            let decision = policy.evaluate(&message.user_id.0, channel, &message.content);
            match decision {
                PolicyDecision::Allow => {}
                PolicyDecision::Deny { reason } => {
                    return DispatchResult::suppress(format!("send policy denied: {}", reason));
                }
                PolicyDecision::Silenced => {
                    return DispatchResult::suppress("send policy: silenced");
                }
            }
        }

        // Stage 2: Plugin-owned binding (stub — would check plugin registry)
        // let plugin_binding = self.resolve_plugin_binding(message).await;

        // Stage 3: Extract workspace hint from message content
        let workspace_hint = Self::extract_workspace_mention(&message.content);

        // Stage 4: Suppression check for group chats
        if self.config.suppress_unless_mentioned_in_groups {
            let is_group = matches!(
                message.provenance,
                crate::channels::InputProvenance::ExternalUser { is_direct: false, .. }
            );
            let is_mentioned = message.mention.should_process(true);
            if is_group && !is_mentioned {
                return DispatchResult::suppress("group chat without mention");
            }
        }

        DispatchResult {
            suppress: false,
            workspace_hint,
            plugin_binding: None,
            suppress_reason: None,
        }
    }

    /// Extract a `@workspace_name` mention from message content.
    fn extract_workspace_mention(content: &str) -> Option<String> {
        // Look for #workspace_name anywhere in the message
        for word in content.split_whitespace() {
            if let Some(name) = word.strip_prefix('#') {
                let name =
                    name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_allow_basic() {
        let dispatch = AutoReplyDispatch::new(AutoReplyDispatchConfig::default());
        let msg = IncomingMessage::new("u1", "s1", "hello");
        let result = dispatch.process(&msg, None).await;
        assert!(!result.suppress);
    }

    #[tokio::test]
    async fn test_workspace_mention() {
        let dispatch = AutoReplyDispatch::new(AutoReplyDispatchConfig::default());
        let msg = IncomingMessage::new("u1", "s1", "check #dev workspace");
        let result = dispatch.process(&msg, None).await;
        assert_eq!(result.workspace_hint, Some("dev".to_string()));
    }

    #[tokio::test]
    async fn test_suppress_group_without_mention() {
        let mut config = AutoReplyDispatchConfig::default();
        config.suppress_unless_mentioned_in_groups = true;
        let dispatch = AutoReplyDispatch::new(config);

        let mut msg = IncomingMessage::new("u1", "s1", "hello");
        msg.provenance = crate::channels::InputProvenance::ExternalUser {
            channel: "telegram".to_string(),
            is_direct: false,
        };
        msg.mention = crate::channels::MentionState::NotMentioned;

        let result = dispatch.process(&msg, None).await;
        assert!(result.suppress);
    }
}
