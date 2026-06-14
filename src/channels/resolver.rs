//! Conversation resolution with multi-source fallback
//!
//! Resolves which agent/session should handle an incoming message by trying
//! sources in order:
//!
//! 1. `command-provider` — Explicit command specifying agent/binding
//! 2. `focused-binding` — Current session binding (existing conversation)
//! 3. `inbound-provider` — Channel-specific binding
//! 4. `inbound-bundled-artifact` — Artifact-based binding
//! 5. `inbound-bundled-plugin` — Plugin-based binding
//! 6. `inbound-fallback` — Default fallback

use crate::channels::IncomingMessage;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Sources for conversation resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionSource {
    /// Explicit command (e.g., `/agent coder ...`).
    CommandProvider,
    /// Existing session binding.
    FocusedBinding,
    /// Channel-specific binding.
    InboundProvider,
    /// Artifact-based binding.
    InboundBundledArtifact,
    /// Plugin-based binding.
    InboundBundledPlugin,
    /// Default fallback.
    InboundFallback,
}

impl ResolutionSource {
    /// Priority order (lower = tried first).
    pub fn priority(&self) -> u32 {
        match self {
            Self::CommandProvider => 1,
            Self::FocusedBinding => 2,
            Self::InboundProvider => 3,
            Self::InboundBundledArtifact => 4,
            Self::InboundBundledPlugin => 5,
            Self::InboundFallback => 6,
        }
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::CommandProvider => "command",
            Self::FocusedBinding => "focused",
            Self::InboundProvider => "inbound",
            Self::InboundBundledArtifact => "artifact",
            Self::InboundBundledPlugin => "plugin",
            Self::InboundFallback => "fallback",
        }
    }
}

impl fmt::Display for ResolutionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Result of a conversation resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationResolution {
    /// The resolved agent ID.
    pub agent_id: String,
    /// The resolved session ID (may be the same as or different from the incoming conversation_id).
    pub session_id: String,
    /// Optional workspace ID.
    pub workspace_id: Option<String>,
    /// Which source provided this resolution.
    pub source: ResolutionSource,
    /// Whether this is a new binding (created on-the-fly).
    pub created_binding: bool,
}

impl ConversationResolution {
    /// Create a new resolution.
    pub fn new(
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
        source: ResolutionSource,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            workspace_id: None,
            source,
            created_binding: false,
        }
    }

    /// Set workspace ID.
    pub fn with_workspace(mut self, ws: impl Into<String>) -> Self {
        self.workspace_id = Some(ws.into());
        self
    }

    /// Mark as a new binding.
    pub fn new_binding(mut self) -> Self {
        self.created_binding = true;
        self
    }
}

/// A provider that can resolve a conversation given an incoming message.
#[async_trait::async_trait]
pub trait ResolutionProvider: Send + Sync {
    /// Try to resolve an agent and session for this message.
    /// Returns `None` if this provider cannot resolve.
    async fn resolve(
        &self,
        message: &IncomingMessage,
    ) -> Option<ConversationResolution>;
}

/// Uses explicit `@agent_name` mentions or commands to resolve.
pub struct CommandProvider {
    #[allow(dead_code)]
    default_agent_id: String,
}

impl CommandProvider {
    pub fn new(default_agent_id: impl Into<String>) -> Self {
        Self {
            default_agent_id: default_agent_id.into(),
        }
    }
}

#[async_trait::async_trait]
impl ResolutionProvider for CommandProvider {
    async fn resolve(
        &self,
        message: &IncomingMessage,
    ) -> Option<ConversationResolution> {
        let content = message.content.trim();

        // Check for @agent mention
        if let Some(rest) = content.strip_prefix('@') {
            let name = rest.split_whitespace().next()?;
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return Some(
                    ConversationResolution::new(
                        name,
                        &message.conversation_id.0,
                        ResolutionSource::CommandProvider,
                    )
                    .new_binding(),
                );
            }
        }

        None
    }
}

/// Uses existing session binding to resolve.
pub struct FocusedBindingProvider {
    /// session_id -> (agent_id, workspace_id)
    bindings: std::sync::Arc<tokio::sync::RwLock<
        std::collections::HashMap<String, (String, Option<String>)>,
    >>,
}

impl FocusedBindingProvider {
    pub fn new(
        bindings: std::sync::Arc<
            tokio::sync::RwLock<
                std::collections::HashMap<String, (String, Option<String>)>,
            >,
        >,
    ) -> Self {
        Self { bindings }
    }

    /// Store a binding.
    pub async fn bind(
        &self,
        session_id: &str,
        agent_id: &str,
        workspace_id: Option<String>,
    ) {
        let mut b = self.bindings.write().await;
        b.insert(session_id.to_string(), (agent_id.to_string(), workspace_id));
    }

    /// Remove a binding.
    pub async fn unbind(&self, session_id: &str) {
        let mut b = self.bindings.write().await;
        b.remove(session_id);
    }
}

#[async_trait::async_trait]
impl ResolutionProvider for FocusedBindingProvider {
    async fn resolve(
        &self,
        message: &IncomingMessage,
    ) -> Option<ConversationResolution> {
        let bindings = self.bindings.read().await;
        let session_id = &message.conversation_id.0;
        if let Some((agent_id, workspace_id)) = bindings.get(session_id) {
            Some(
                ConversationResolution::new(
                    agent_id,
                    session_id,
                    ResolutionSource::FocusedBinding,
                )
                .with_workspace(workspace_id.clone().unwrap_or_default()),
            )
        } else {
            None
        }
    }
}

/// Uses channel-specific default binding.
pub struct InboundProvider {
    /// channel_name -> (agent_id, workspace_id)
    defaults: std::collections::HashMap<String, (String, Option<String>)>,
}

impl InboundProvider {
    pub fn new() -> Self {
        Self {
            defaults: std::collections::HashMap::new(),
        }
    }

    /// Set the default agent for a channel.
    pub fn set_channel_default(
        &mut self,
        channel: impl Into<String>,
        agent_id: impl Into<String>,
        workspace_id: Option<String>,
    ) {
        self.defaults
            .insert(channel.into(), (agent_id.into(), workspace_id));
    }
}

#[async_trait::async_trait]
impl ResolutionProvider for InboundProvider {
    async fn resolve(
        &self,
        message: &IncomingMessage,
    ) -> Option<ConversationResolution> {
        let channel_name = match &message.provenance {
            crate::channels::InputProvenance::ExternalUser { channel, .. } => {
                channel.clone()
            }
            _ => return None,
        };

        if let Some((agent_id, workspace_id)) = self.defaults.get(&channel_name) {
            Some(
                ConversationResolution::new(
                    agent_id,
                    &message.conversation_id.0,
                    ResolutionSource::InboundProvider,
                )
                .with_workspace(workspace_id.clone().unwrap_or_default())
                .new_binding(),
            )
        } else {
            None
        }
    }
}

impl Default for InboundProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Artifact-based binding (resolves based on artifact context in the message).
pub struct ArtifactBindingProvider;

#[async_trait::async_trait]
impl ResolutionProvider for ArtifactBindingProvider {
    async fn resolve(
        &self,
        _message: &IncomingMessage,
    ) -> Option<ConversationResolution> {
        // Placeholder: artifact-based resolution is not yet implemented.
        // In the future, this could parse artifact references in the message
        // content and route to the artifact's owning agent/session.
        None
    }
}

/// Plugin-based binding (delegates to registered plugins).
pub struct PluginBindingProvider;

#[async_trait::async_trait]
impl ResolutionProvider for PluginBindingProvider {
    async fn resolve(
        &self,
        _message: &IncomingMessage,
    ) -> Option<ConversationResolution> {
        // Placeholder: plugin-based resolution is not yet implemented.
        // In the future, registered plugins could claim ownership of
        // specific message patterns or conversation IDs.
        None
    }
}

/// Default fallback provider.
pub struct FallbackProvider {
    default_agent_id: String,
    default_workspace_id: Option<String>,
}

impl FallbackProvider {
    pub fn new(
        default_agent_id: impl Into<String>,
        default_workspace_id: Option<String>,
    ) -> Self {
        Self {
            default_agent_id: default_agent_id.into(),
            default_workspace_id,
        }
    }
}

#[async_trait::async_trait]
impl ResolutionProvider for FallbackProvider {
    async fn resolve(
        &self,
        message: &IncomingMessage,
    ) -> Option<ConversationResolution> {
        Some(
            ConversationResolution::new(
                &self.default_agent_id,
                &message.conversation_id.0,
                ResolutionSource::InboundFallback,
            )
            .with_workspace(self.default_workspace_id.clone().unwrap_or_default())
            .new_binding(),
        )
    }
}

/// The conversation resolver that chains multiple providers in priority order.
pub struct ConversationResolver {
    providers: Vec<Box<dyn ResolutionProvider>>,
}

impl ConversationResolver {
    /// Create a new resolver with the default provider chain.
    pub fn with_default_chain(
        default_agent_id: impl Into<String>,
        default_workspace_id: Option<String>,
        bindings: std::sync::Arc<
            tokio::sync::RwLock<
                std::collections::HashMap<String, (String, Option<String>)>,
            >,
        >,
    ) -> Self {
        let mut resolver = Self { providers: Vec::new() };

        let agent_id: String = default_agent_id.into();

        // 1. Command provider (highest priority)
        resolver.add_provider(Box::new(CommandProvider::new(agent_id.clone())));

        // 2. Focused binding provider
        resolver.add_provider(Box::new(FocusedBindingProvider::new(bindings)));

        // 3. Inbound provider
        resolver.add_provider(Box::new(InboundProvider::new()));

        // 4. Artifact binding provider
        resolver.add_provider(Box::new(ArtifactBindingProvider));

        // 5. Plugin binding provider
        resolver.add_provider(Box::new(PluginBindingProvider));

        // 6. Fallback provider (lowest priority)
        resolver.add_provider(Box::new(FallbackProvider::new(
            agent_id,
            default_workspace_id,
        )));

        resolver
    }

    /// Add a resolution provider to the chain.
    pub fn add_provider(&mut self, provider: Box<dyn ResolutionProvider>) {
        self.providers.push(provider);
    }

    /// Resolve a conversation by trying each provider in order.
    ///
    /// Returns the first successful resolution.
    pub async fn resolve(&self, message: &IncomingMessage) -> ConversationResolution {
        for provider in &self.providers {
            if let Some(resolution) = provider.resolve(message).await {
                return resolution;
            }
        }

        // Unreachable: the fallback provider always returns Some.
        unreachable!("Fallback provider should always resolve")
    }

    /// Resolve with logging of which source was used.
    pub async fn resolve_with_log(
        &self,
        message: &IncomingMessage,
    ) -> ConversationResolution {
        for provider in &self.providers {
            if let Some(resolution) = provider.resolve(message).await {
                tracing::debug!(
                    "Conversation resolved via {} -> agent={}, session={}",
                    resolution.source,
                    resolution.agent_id,
                    resolution.session_id
                );
                return resolution;
            }
        }
        unreachable!("Fallback provider should always resolve")
    }
}

/// Resolve a conversation using the default chain. Convenience function.
pub async fn resolve_conversation(
    message: &IncomingMessage,
    default_agent: &str,
    bindings: std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, (String, Option<String>)>,
        >,
    >,
) -> ConversationResolution {
    let resolver =
        ConversationResolver::with_default_chain(default_agent, None, bindings);
    resolver.resolve(message).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{InputProvenance, MentionState};
    use std::sync::Arc;

    fn make_message(
        user_id: &str,
        conv_id: &str,
        content: &str,
        channel: &str,
    ) -> IncomingMessage {
        IncomingMessage {
            id: crate::core::models::Id::new(),
            user_id: crate::channels::UserId::new(user_id),
            conversation_id: crate::channels::ConversationId::new(conv_id),
            content: content.to_string(),
            attachments: Vec::new(),
            metadata: crate::channels::MessageMetadata::new(),
            provenance: InputProvenance::ExternalUser {
                channel: channel.to_string(),
                is_direct: true,
            },
            mention: MentionState::DirectMessage,
        }
    }

    #[tokio::test]
    async fn test_command_provider() {
        let provider = CommandProvider::new("default");
        let msg = make_message("u1", "c1", "@coder write code", "telegram");
        let result = provider.resolve(&msg).await.unwrap();
        assert_eq!(result.agent_id, "coder");
        assert_eq!(result.source, ResolutionSource::CommandProvider);
    }

    #[tokio::test]
    async fn test_command_provider_no_match() {
        let provider = CommandProvider::new("default");
        let msg = make_message("u1", "c1", "hello", "telegram");
        assert!(provider.resolve(&msg).await.is_none());
    }

    #[tokio::test]
    async fn test_focused_binding_provider() {
        let bindings = Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        ));
        let provider = FocusedBindingProvider::new(bindings.clone());

        // No binding yet
        let msg = make_message("u1", "c1", "hello", "telegram");
        assert!(provider.resolve(&msg).await.is_none());

        // Add binding
        provider.bind("c1", "coder", None).await;
        let result = provider.resolve(&msg).await.unwrap();
        assert_eq!(result.agent_id, "coder");
        assert_eq!(result.source, ResolutionSource::FocusedBinding);

        // Unbind
        provider.unbind("c1").await;
        assert!(provider.resolve(&msg).await.is_none());
    }

    #[tokio::test]
    async fn test_inbound_provider() {
        let mut provider = InboundProvider::new();
        provider.set_channel_default("telegram", "telegram_agent", None);

        let msg = make_message("u1", "c1", "hello", "telegram");
        let result = provider.resolve(&msg).await.unwrap();
        assert_eq!(result.agent_id, "telegram_agent");

        // Unknown channel
        let msg2 = make_message("u1", "c1", "hello", "unknown");
        assert!(provider.resolve(&msg2).await.is_none());
    }

    #[tokio::test]
    async fn test_fallback_provider() {
        let provider = FallbackProvider::new("default", Some("ws1".to_string()));
        let msg = make_message("u1", "c1", "hello", "telegram");
        let result = provider.resolve(&msg).await.unwrap();
        assert_eq!(result.agent_id, "default");
        assert_eq!(result.source, ResolutionSource::InboundFallback);
        assert!(result.created_binding);
    }

    #[tokio::test]
    async fn test_full_resolver_chain() {
        let bindings = Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        ));
        let resolver =
            ConversationResolver::with_default_chain("default", None, bindings);

        // Should resolve via command provider
        let msg = make_message("u1", "c1", "@coder do thing", "telegram");
        let result = resolver.resolve(&msg).await;
        assert_eq!(result.source, ResolutionSource::CommandProvider);
        assert_eq!(result.agent_id, "coder");
    }

    #[tokio::test]
    async fn test_resolver_fallback() {
        let bindings = Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        ));
        let resolver =
            ConversationResolver::with_default_chain("default", None, bindings);

        // No bindings, no command, unknown channel — should fallback
        let msg = make_message("u1", "c1", "hello", "signal");
        let result = resolver.resolve(&msg).await;
        assert_eq!(result.source, ResolutionSource::InboundFallback);
        assert_eq!(result.agent_id, "default");
    }

    #[tokio::test]
    async fn test_resolver_prefers_binding_over_fallback() {
        let bindings = Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        ));
        let binding_provider = FocusedBindingProvider::new(bindings.clone());
        binding_provider.bind("c1", "specialist", None).await;

        let resolver =
            ConversationResolver::with_default_chain("default", None, bindings);

        let msg = make_message("u1", "c1", "hello", "signal");
        let result = resolver.resolve(&msg).await;
        assert_eq!(result.source, ResolutionSource::FocusedBinding);
        assert_eq!(result.agent_id, "specialist");
    }

    #[tokio::test]
    async fn test_convenience_function() {
        let bindings = Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        ));
        let msg = make_message("u1", "c1", "hello", "telegram");
        let result = resolve_conversation(&msg, "default", bindings).await;
        assert_eq!(result.agent_id, "default");
    }

    #[test]
    fn test_resolution_source_priority() {
        assert!(
            ResolutionSource::CommandProvider.priority()
                < ResolutionSource::FocusedBinding.priority()
        );
        assert!(
            ResolutionSource::FocusedBinding.priority()
                < ResolutionSource::InboundProvider.priority()
        );
        assert!(
            ResolutionSource::InboundFallback.priority()
                > ResolutionSource::InboundBundledPlugin.priority()
        );
    }

    #[test]
    fn test_resolution_source_labels() {
        assert_eq!(ResolutionSource::CommandProvider.label(), "command");
        assert_eq!(ResolutionSource::InboundFallback.label(), "fallback");
    }
}
