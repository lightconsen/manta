//! Inbound Pipeline for Syscity
//!
//! The inbound pipeline is the entry point for all user messages.
//! It replaces the direct "Channel -> Gateway -> Agent" path with a layered
//! processing pipeline:
//!
//! ```text
//! Channel Extension -> Identity (optional) -> Debounce -> Dispatch -> Media Understanding
//! -> Envelope (optional) -> Queue Mode Resolve -> Agent Router -> Agent
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::warn;

use crate::channels::envelope::SessionEnvelopeManager;
use crate::channels::identity::IdentityValidator;
use crate::channels::IncomingMessage;
use crate::inbound::stage::{
    build_routed_message, default_post_debounce_stages, default_pre_debounce_stages,
    run_inbound_stages, InboundContext, InboundStageAction, StageError,
};

pub mod debounce;
pub mod dispatch;
pub mod equivalence_tests;
pub mod media;
pub mod queue;
pub mod router;
pub mod stage;

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
    /// Session envelope context captured during pipeline processing.
    pub envelope_context: Option<crate::channels::envelope::SessionEnvelopeContext>,
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
/// Wires all stages together through the pluggable [`InboundStage`] runner:
/// identity (optional) → debounce → media → dispatch → envelope (optional)
/// → queue → router.
pub struct DefaultInboundPipeline {
    debouncer: Arc<InboundDebouncer>,
    media_pipeline: MediaUnderstandingPipeline,
    dispatch: AutoReplyDispatch,
    queue_resolver: QueueModeResolver,
    router: Arc<AgentRouter>,
    /// Sender to forward routed messages to the agent execution layer.
    routed_tx: mpsc::Sender<RoutedMessage>,
    /// Receiver for debounced message batches from the debouncer.
    flush_rx: tokio::sync::Mutex<mpsc::Receiver<Vec<crate::inbound::debounce::DebouncedItem>>>,
    /// Optional identity validator for pre-debounce checks.
    identity_validator: Option<IdentityValidator>,
    /// Optional session envelope manager for tracking session context.
    envelope_manager: Option<SessionEnvelopeManager>,
    /// Cached pre-debounce stage list to avoid rebuilding it per message.
    pre_stages: Vec<Box<dyn crate::inbound::stage::InboundStage>>,
    /// Cached post-debounce stage list to avoid rebuilding it per message.
    post_stages: Vec<Box<dyn crate::inbound::stage::InboundStage>>,
    /// Whether the background flush loop has already been started.
    started: AtomicBool,
}

impl DefaultInboundPipeline {
    pub fn new(
        debouncer: Arc<InboundDebouncer>,
        media_pipeline: MediaUnderstandingPipeline,
        dispatch: AutoReplyDispatch,
        queue_resolver: QueueModeResolver,
        router: Arc<AgentRouter>,
        routed_tx: mpsc::Sender<RoutedMessage>,
        flush_rx: mpsc::Receiver<Vec<crate::inbound::debounce::DebouncedItem>>,
    ) -> Self {
        let pre_stages = default_pre_debounce_stages(None, debouncer.clone());
        let post_stages = default_post_debounce_stages(
            media_pipeline.clone(),
            dispatch.clone(),
            None,
            queue_resolver.clone(),
            router.clone(),
        );
        Self {
            debouncer,
            media_pipeline,
            dispatch,
            queue_resolver,
            router,
            routed_tx,
            flush_rx: tokio::sync::Mutex::new(flush_rx),
            identity_validator: None,
            envelope_manager: None,
            pre_stages,
            post_stages,
            started: AtomicBool::new(false),
        }
    }

    /// Attach an identity validator for pre-debounce checks.
    pub fn with_identity_validator(mut self, validator: IdentityValidator) -> Self {
        self.identity_validator = Some(validator.clone());
        self.pre_stages = default_pre_debounce_stages(Some(validator), self.debouncer.clone());
        self
    }

    /// Attach a session envelope manager for tracking session context.
    pub fn with_envelope_manager(mut self, manager: SessionEnvelopeManager) -> Self {
        self.envelope_manager = Some(manager.clone());
        self.post_stages = default_post_debounce_stages(
            self.media_pipeline.clone(),
            self.dispatch.clone(),
            Some(manager),
            self.queue_resolver.clone(),
            self.router.clone(),
        );
        self
    }

    /// Start a background task that consumes debounced messages and runs
    /// them through the rest of the pipeline.
    ///
    /// This is a no-op if the pipeline has already been started.
    pub fn start(self: Arc<Self>) {
        if self
            .started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("DefaultInboundPipeline already started; ignoring duplicate start()");
            return;
        }
        let pipeline = self.clone();
        tokio::spawn(async move {
            pipeline.run_loop().await;
        });
    }

    async fn run_loop(self: Arc<Self>) {
        let mut rx = self.flush_rx.lock().await;
        while let Some(batch) = rx.recv().await {
            for item in batch {
                let mut ctx = InboundContext::new(item.message);
                match run_inbound_stages(&self.post_stages, &mut ctx).await {
                    Ok(InboundStageAction::Continue) => match build_routed_message(&mut ctx) {
                        Ok(routed) => {
                            let _ = self.routed_tx.send(routed).await;
                        }
                        Err(e) => {
                            warn!(error = %e, "Post-debounce routing failed during flush");
                        }
                    },
                    Ok(action) => {
                        tracing::debug!(?action, "Debounce flush terminal action");
                    }
                    Err(e) => {
                        warn!(error = %e, "Post-debounce stage failed during flush");
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl InboundPipeline for DefaultInboundPipeline {
    async fn process(&self, message: IncomingMessage) -> Option<RoutedMessage> {
        if !self.started.load(Ordering::Relaxed) {
            tracing::warn!(
                "DefaultInboundPipeline::process() called before start(); \
                 debounced messages will not be flushed by the background loop"
            );
        }

        let mut ctx = InboundContext::new(message);

        match run_inbound_stages(&self.pre_stages, &mut ctx).await {
            Ok(InboundStageAction::Continue) => {}
            Ok(action) => {
                tracing::debug!(?action, "Pre-debounce terminal action");
                return None;
            }
            Err(StageError::Fatal { stage, source }) => {
                warn!(stage, error = %source, "Pre-debounce stage failed");
                return None;
            }
        }

        match run_inbound_stages(&self.post_stages, &mut ctx).await {
            Ok(InboundStageAction::Continue) => match build_routed_message(&mut ctx) {
                Ok(routed) => {
                    let _ = self.routed_tx.send(routed.clone()).await;
                    Some(routed)
                }
                Err(e) => {
                    warn!(error = %e, "Post-debounce routing failed");
                    None
                }
            },
            Ok(action) => {
                tracing::debug!(?action, "Post-debounce terminal action");
                None
            }
            Err(StageError::Fatal { stage, source }) => {
                warn!(stage, error = %source, "Post-debounce stage failed");
                None
            }
        }
    }

    async fn flush(&self, key: &str) -> Vec<RoutedMessage> {
        let messages = self.debouncer.flush_key(key).await;
        let mut routed = Vec::new();
        for msg in messages {
            let mut ctx = InboundContext::new(msg);
            match run_inbound_stages(&self.post_stages, &mut ctx).await {
                Ok(InboundStageAction::Continue) => match build_routed_message(&mut ctx) {
                    Ok(r) => {
                        let _ = self.routed_tx.send(r.clone()).await;
                        routed.push(r);
                    }
                    Err(e) => {
                        warn!(error = %e, "Flush routing failed");
                    }
                },
                Ok(action) => {
                    tracing::debug!(?action, "Flush terminal action");
                }
                Err(StageError::Fatal { stage, source }) => {
                    warn!(stage, error = %source, "Flush stage failed");
                }
            }
        }
        routed
    }
}
