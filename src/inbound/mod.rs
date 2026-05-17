//! Inbound Pipeline for Manta
//!
//! The inbound pipeline is the entry point for all user messages.
//! It replaces the direct "Channel -> Gateway -> Agent" path with a layered
//! processing pipeline that matches OpenClaw's architecture:
//!
//! ```text
//! Channel Extension -> Debounce -> Media Understanding -> Dispatch
//!     -> Queue Mode Resolve -> Agent Router -> Agent
//! ```

use crate::channels::IncomingMessage;
use std::sync::Arc;
use tokio::sync::mpsc;

pub mod debounce;
pub mod dispatch;
pub mod media;
pub mod queue;
pub mod router;

pub use debounce::{InboundDebouncer, InboundDebouncerConfig};
pub use dispatch::{AutoReplyDispatch, AutoReplyDispatchConfig, DispatchResult};
pub use media::{MediaAttachmentCache, MediaUnderstandingPipeline, MediaUnderstandingResult};
pub use queue::{QueueMode, QueueModeResolver};
pub use router::{
    AgentRouter, AgentRouterConfig, BindingStore, InMemoryBindingStore, RouteResult,
    SqliteBindingStore,
};

/// A message that has been processed by the inbound pipeline and is ready
/// for routing to an agent.
#[derive(Debug, Clone)]
pub struct RoutedMessage {
    /// The original incoming message
    pub incoming: IncomingMessage,
    /// Target agent ID (resolved by router)
    pub agent_id: String,
    /// Target workspace ID (if multi-workspace)
    pub workspace_id: Option<String>,
    /// Queue mode for this message
    pub queue_mode: QueueMode,
    /// Whether delivery should be suppressed (silent mode)
    pub suppress_delivery: bool,
    /// Media understanding results (if any attachments were processed)
    pub media_results: Option<MediaUnderstandingResult>,
}

/// The inbound pipeline trait.
///
/// Implementations process raw incoming messages through the full pipeline
/// (debounce, media, dispatch, queue, router) and produce `RoutedMessage`s.
#[async_trait::async_trait]
pub trait InboundPipeline: Send + Sync {
    /// Process a single incoming message.
    ///
    /// Returns `None` if the message was absorbed by the pipeline
    /// (e.g., debounced, filtered, or queued for later).
    async fn process(&self, message: IncomingMessage) -> Option<RoutedMessage>;

    /// Flush all pending messages for a given key.
    ///
    /// Used at shutdown or when an explicit flush is needed.
    async fn flush(&self, key: &str) -> Vec<RoutedMessage>;
}

/// Default inbound pipeline implementation.
///
/// Wires all stages together: debounce -> media -> dispatch -> queue -> router.
pub struct DefaultInboundPipeline {
    debouncer: Arc<InboundDebouncer>,
    media_pipeline: MediaUnderstandingPipeline,
    dispatch: AutoReplyDispatch,
    queue_resolver: QueueModeResolver,
    router: AgentRouter,
    /// Sender to forward routed messages to the agent execution layer.
    routed_tx: mpsc::Sender<RoutedMessage>,
    /// Receiver for debounced message batches from the debouncer.
    flush_rx: tokio::sync::Mutex<mpsc::Receiver<Vec<crate::inbound::debounce::DebouncedItem>>>,
}

impl DefaultInboundPipeline {
    pub fn new(
        debouncer: Arc<InboundDebouncer>,
        media_pipeline: MediaUnderstandingPipeline,
        dispatch: AutoReplyDispatch,
        queue_resolver: QueueModeResolver,
        router: AgentRouter,
        routed_tx: mpsc::Sender<RoutedMessage>,
        flush_rx: mpsc::Receiver<Vec<crate::inbound::debounce::DebouncedItem>>,
    ) -> Self {
        Self {
            debouncer,
            media_pipeline,
            dispatch,
            queue_resolver,
            router,
            routed_tx,
            flush_rx: tokio::sync::Mutex::new(flush_rx),
        }
    }

    /// Start a background task that consumes debounced messages and runs
    /// them through the rest of the pipeline.
    pub fn start(self: Arc<Self>) {
        let pipeline = self.clone();
        tokio::spawn(async move {
            pipeline.run_loop().await;
        });
    }

    async fn run_loop(self: Arc<Self>) {
        let mut rx = self.flush_rx.lock().await;
        while let Some(batch) = rx.recv().await {
            for item in batch {
                let _ = self.process(item.message).await;
            }
        }
    }
}

#[async_trait::async_trait]
impl InboundPipeline for DefaultInboundPipeline {
    async fn process(&self, message: IncomingMessage) -> Option<RoutedMessage> {
        // Stage 1: Debounce
        // If the message should be batched, the debouncer absorbs it and
        // will emit a flushed batch later.
        let debounced = self.debouncer.enqueue(message).await?;

        // Stage 2: Media understanding
        let media_results = if !debounced.attachments.is_empty() {
            Some(self.media_pipeline.process(&debounced).await)
        } else {
            None
        };

        // Stage 3: Dispatch (send policy, plugin-owned binding, etc.)
        let dispatch_result = self
            .dispatch
            .process(&debounced, media_results.as_ref())
            .await;
        if dispatch_result.suppress {
            return None;
        }

        // Stage 4: Queue mode resolution
        let queue_mode = self.queue_resolver.resolve(&debounced).await;

        // Stage 5: Agent routing
        let route = self
            .router
            .route(&debounced, dispatch_result.workspace_hint.as_deref())
            .await;

        let routed = RoutedMessage {
            incoming: debounced,
            agent_id: route.agent_id,
            workspace_id: route.workspace_id,
            queue_mode,
            suppress_delivery: dispatch_result.suppress,
            media_results,
        };

        // Forward to the agent execution layer
        let _ = self.routed_tx.send(routed.clone()).await;

        Some(routed)
    }

    async fn flush(&self, key: &str) -> Vec<RoutedMessage> {
        let messages = self.debouncer.flush_key(key).await;
        let mut routed = Vec::new();
        for msg in messages {
            if let Some(r) = self.process(msg).await {
                routed.push(r);
            }
        }
        routed
    }
}
