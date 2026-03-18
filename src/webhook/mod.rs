//! Webhook receivers for Manta channels
//!
//! Provides HTTP endpoints for receiving messages from external platforms
//! that use webhook callbacks (WhatsApp, Lark/Feishu, QQ).

use axum::{routing::get, Router};
use std::sync::Arc;

#[cfg(feature = "whatsapp")]
pub mod whatsapp;

#[cfg(feature = "lark")]
pub mod lark;

#[cfg(feature = "qq")]
pub mod qq;

/// Shared webhook state
#[derive(Clone)]
pub struct WebhookState {
    /// The agent to process incoming messages
    pub agent: Option<Arc<crate::agent::Agent>>,
    /// WhatsApp configuration
    #[cfg(feature = "whatsapp")]
    pub whatsapp_config: Option<crate::channels::WhatsappConfig>,
    /// Lark configuration
    #[cfg(feature = "lark")]
    pub lark_config: Option<crate::channels::LarkConfig>,
    /// QQ configuration
    #[cfg(feature = "qq")]
    pub qq_config: Option<crate::channels::QqConfig>,
}

impl WebhookState {
    /// Create new webhook state
    pub fn new(agent: Option<Arc<crate::agent::Agent>>) -> Self {
        Self {
            agent,
            #[cfg(feature = "whatsapp")]
            whatsapp_config: None,
            #[cfg(feature = "lark")]
            lark_config: None,
            #[cfg(feature = "qq")]
            qq_config: None,
        }
    }

    /// Set WhatsApp configuration
    #[cfg(feature = "whatsapp")]
    pub fn with_whatsapp_config(mut self, config: crate::channels::WhatsappConfig) -> Self {
        self.whatsapp_config = Some(config);
        self
    }

    /// Set Lark configuration
    #[cfg(feature = "lark")]
    pub fn with_lark_config(mut self, config: crate::channels::LarkConfig) -> Self {
        self.lark_config = Some(config);
        self
    }

    /// Set QQ configuration
    #[cfg(feature = "qq")]
    pub fn with_qq_config(mut self, config: crate::channels::QqConfig) -> Self {
        self.qq_config = Some(config);
        self
    }
}

/// Create the webhook router
pub fn create_webhook_router(_state: WebhookState) -> Router {
    #[allow(unused_mut)]
    let mut router = Router::new().route("/", get(webhook_root));

    #[cfg(feature = "whatsapp")]
    {
        router = router.nest("/whatsapp", whatsapp::whatsapp_webhook_router(_state.clone()));
    }

    #[cfg(feature = "lark")]
    {
        router = router.nest("/lark", lark::lark_webhook_router(_state.clone()));
    }

    #[cfg(feature = "qq")]
    {
        router = router.nest("/qq", qq::qq_webhook_router(_state));
    }

    router
}

/// Root webhook endpoint
async fn webhook_root() -> &'static str {
    "Manta Webhook Server\n\nAvailable endpoints:\n- /webhooks/whatsapp - WhatsApp Business API webhooks\n- /webhooks/lark - Lark/Feishu webhooks\n- /webhooks/qq - QQ Bot webhooks\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_state_creation() {
        let state = WebhookState::new(None);
        assert!(state.agent.is_none());
        #[cfg(feature = "whatsapp")]
        assert!(state.whatsapp_config.is_none());
    }
}
