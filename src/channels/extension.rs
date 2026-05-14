//! Channel Extension Layer
//!
//! Defines how external channels integrate with the Inbound and Outbound
//! pipelines.  Replaces the ad-hoc per-channel message loops with a unified
//! extension interface.
//!
//! Design matches OpenClaw's channel extension architecture.

use crate::channels::{IncomingMessage, OutgoingMessage};
use std::sync::Arc;
use tokio::sync::mpsc;

/// A channel extension adapter.
///
/// Wraps a raw `Channel` and wires it into the Inbound/Outbound pipelines.
#[async_trait::async_trait]
pub trait ChannelExtension: Send + Sync {
    /// Unique name of this extension (e.g. "telegram", "discord_webhook").
    fn name(&self) -> &str;

    /// Start receiving messages and feed them into the inbound pipeline.
    ///
    /// The extension should convert raw channel events into `IncomingMessage`s
    /// and push them to `inbound_tx`.
    async fn run_inbound(
        &self,
        inbound_tx: mpsc::Sender<IncomingMessage>,
    ) -> crate::Result<()>;

    /// Start dispatching outbound messages back to the channel.
    ///
    /// The extension receives `OutgoingMessage`s from the reply dispatcher
    /// and delivers them to the underlying channel.
    async fn run_outbound(
        &self,
        mut outbound_rx: mpsc::Receiver<OutgoingMessage>,
    ) -> crate::Result<()>;
}

/// Registry of channel extensions.
pub struct ChannelExtensionRegistry {
    extensions: Vec<Arc<dyn ChannelExtension>>,
}

impl ChannelExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    pub fn register(&mut self,
        ext: Arc<dyn ChannelExtension>,
    ) {
        self.extensions.push(ext);
    }

    pub fn list(&self) -> &[Arc<dyn ChannelExtension>] {
        &self.extensions
    }
}

impl Default for ChannelExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for a channel extension instance.
#[derive(Debug, Clone)]
pub struct ChannelExtensionConfig {
    pub name: String,
    pub enabled: bool,
    pub credentials: std::collections::HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyExtension;
    #[async_trait::async_trait]
    impl ChannelExtension for DummyExtension {
        fn name(&self) -> &str { "dummy" }
        async fn run_inbound(
            &self,
            _inbound_tx: mpsc::Sender<IncomingMessage>,
        ) -> crate::Result<()> { Ok(()) }
        async fn run_outbound(
            &self,
            _outbound_rx: mpsc::Receiver<OutgoingMessage>,
        ) -> crate::Result<()> { Ok(()) }
    }

    #[test]
    fn test_registry() {
        let mut reg = ChannelExtensionRegistry::new();
        reg.register(Arc::new(DummyExtension));
        assert_eq!(reg.list().len(), 1);
    }
}
