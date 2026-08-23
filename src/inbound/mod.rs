//! Inbound Pipeline for Syscity
//!
//! The inbound pipeline is the entry point for all user messages.
//! It replaces the direct "Channel -> Gateway -> Agent" path with a layered
//! processing pipeline:
//!
//! ```text
//! Channel Extension -> Identity (optional) -> Pre-dispatch cheap suppress check
//! -> Debounce -> Dispatch -> Media Understanding -> Envelope (optional)
//! -> Queue Mode Resolve -> Agent Router -> Agent
//! ```
// INVARIANTS-NONE: message intake plumbing; access decisions are delegated to gateway::check_incoming_access and pairing/security stores.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::warn;

use crate::channels::envelope::SessionEnvelopeManager;
use crate::channels::identity::IdentityValidator;
use crate::channels::IncomingMessage;
use crate::inbound::stage::{
    build_routed_message, default_post_debounce_stages, default_pre_debounce_stages,
    run_inbound_stages, IdentityFailMode, InboundContext, InboundStageAction, StageError,
};

pub mod debounce;
pub mod dispatch;
pub mod media;
pub mod pipeline_tests;
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

/// Detailed outcome of processing a single inbound message.
///
/// Unlike [`InboundPipeline::process`], which collapses all non-routed results
/// into `None`, this enum distinguishes between absorption (debounce),
/// explicit suppression, routing failures, and successful routing.
#[derive(Debug, Clone)]
pub enum InboundProcessOutcome {
    /// The message was routed to an agent.
    Routed(Box<RoutedMessage>),
    /// The message was absorbed into the debouncer and will be processed later.
    Absorbed,
    /// The message was explicitly suppressed by a stage (e.g. dispatch policy).
    Suppressed {
        /// Human-readable reason for suppression, if available.
        reason: Option<String>,
    },
    /// A stage failed unexpectedly.
    Failed {
        /// Name of the stage that failed.
        stage: &'static str,
        /// String representation of the error.
        error: String,
    },
}

impl InboundProcessOutcome {
    /// Convert the outcome into the legacy `Option<RoutedMessage>`
    /// representation.
    ///
    /// Returns `Some(message)` only for [`Self::Routed`]. All other variants
    /// return `None`.
    pub fn into_option(self) -> Option<RoutedMessage> {
        match self {
            Self::Routed(msg) => Some(*msg),
            _ => None,
        }
    }
}

/// Default inbound pipeline implementation.
///
/// Wires all stages together through the pluggable [`InboundStage`] runner:
/// identity (optional) → pre-dispatch cheap suppress check → debounce → media
/// → dispatch → envelope (optional) → queue → router.
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
    /// How to handle identity validation failures.
    identity_fail_mode: IdentityFailMode,
    /// Optional session envelope manager for tracking session context.
    envelope_manager: Option<SessionEnvelopeManager>,
    /// Cached pre-debounce stage list to avoid rebuilding it per message.
    pre_stages: Vec<Box<dyn crate::inbound::stage::InboundStage>>,
    /// Cached post-debounce stage list to avoid rebuilding it per message.
    post_stages: Vec<Box<dyn crate::inbound::stage::InboundStage>>,
    /// Whether the background flush loop has already been started.
    started: AtomicBool,
    /// Whether to start the background flush loop automatically when the
    /// pipeline is wrapped in an [`Arc`] and [`DefaultInboundPipeline::start`]
    /// (or [`DefaultInboundPipeline::start_if_configured`]) is called.
    auto_start: bool,
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
        let pre_stages = default_pre_debounce_stages(
            None,
            IdentityFailMode::Warn,
            dispatch.clone(),
            debouncer.clone(),
        );
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
            identity_fail_mode: IdentityFailMode::Warn,
            envelope_manager: None,
            pre_stages,
            post_stages,
            started: AtomicBool::new(false),
            auto_start: false,
        }
    }

    /// Configure whether the pipeline should start its background flush loop
    /// automatically.
    ///
    /// When `auto_start` is `true`, callers can use
    /// [`DefaultInboundPipeline::start_if_configured`] to start the loop
    /// without an extra conditional check. The default is `false`, preserving
    /// the existing explicit-start behavior.
    pub fn with_auto_start(mut self, auto_start: bool) -> Self {
        self.auto_start = auto_start;
        self
    }

    /// Attach an identity validator for pre-debounce checks.
    pub fn with_identity_validator(mut self, validator: IdentityValidator) -> Self {
        self.identity_validator = Some(validator.clone());
        self.pre_stages = default_pre_debounce_stages(
            Some(validator),
            self.identity_fail_mode,
            self.dispatch.clone(),
            self.debouncer.clone(),
        );
        self
    }

    /// Configure how identity validation failures are handled.
    ///
    /// The default is [`IdentityFailMode::Warn`], which logs a warning and
    /// continues processing. [`IdentityFailMode::Suppress`] drops messages that
    /// fail validation.
    ///
    /// This may be called before or after [`Self::with_identity_validator`];
    /// the pre-debounce stage list is rebuilt when either method is called.
    pub fn with_identity_fail_mode(mut self, fail_mode: IdentityFailMode) -> Self {
        self.identity_fail_mode = fail_mode;
        // If a validator is already attached, rebuild the stage list with the
        // new fail mode.
        if let Some(validator) = self.identity_validator.clone() {
            self.pre_stages = default_pre_debounce_stages(
                Some(validator),
                fail_mode,
                self.dispatch.clone(),
                self.debouncer.clone(),
            );
        }
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

    /// Start the background loop only if [`Self::auto_start`] is enabled.
    ///
    /// This is a convenience for callers that configure the pipeline with
    /// [`Self::with_auto_start`]. It is a no-op when `auto_start` is `false`.
    pub fn start_if_configured(self: Arc<Self>) {
        if self.auto_start {
            self.start();
        }
    }

    /// Process a message and return a detailed outcome.
    ///
    /// This is the richer counterpart to [`InboundPipeline::process`]. Callers
    /// that only need the routed message can use `process()`; callers that need
    /// to distinguish debounce, suppress, and failure should use this method.
    pub async fn process_detailed(&self, message: IncomingMessage) -> InboundProcessOutcome {
        if !self.started.load(Ordering::Relaxed) {
            tracing::warn!(
                "DefaultInboundPipeline::process() called before start(); debounced messages will \
                 not be flushed by the background loop"
            );
        }

        let mut ctx = InboundContext::new(message);

        match run_inbound_stages(&self.pre_stages, &mut ctx).await {
            Ok(InboundStageAction::Continue) => {}
            Ok(InboundStageAction::Suppress) => {
                let reason = ctx
                    .dispatch_result
                    .as_ref()
                    .and_then(|d| d.suppress_reason.clone());
                return InboundProcessOutcome::Suppressed { reason };
            }
            Ok(InboundStageAction::Debounce) => {
                return InboundProcessOutcome::Absorbed;
            }
            Err(StageError::Fatal { stage, source }) => {
                warn!(stage, error = %source, "Pre-debounce stage failed");
                return InboundProcessOutcome::Failed {
                    stage,
                    error: source.to_string(),
                };
            }
        }

        match run_inbound_stages(&self.post_stages, &mut ctx).await {
            Ok(InboundStageAction::Continue) => match build_routed_message(&mut ctx) {
                Ok(routed) => {
                    if let Err(e) = self.routed_tx.send(routed.clone()).await {
                        warn!(error = %e, "Failed to forward routed message to agent execution layer");
                    }
                    InboundProcessOutcome::Routed(Box::new(routed))
                }
                Err(e) => {
                    warn!(error = %e, "Post-debounce routing failed");
                    InboundProcessOutcome::Failed {
                        stage: "router",
                        error: e.to_string(),
                    }
                }
            },
            Ok(InboundStageAction::Suppress) => {
                let reason = ctx
                    .dispatch_result
                    .as_ref()
                    .and_then(|d| d.suppress_reason.clone());
                InboundProcessOutcome::Suppressed { reason }
            }
            Ok(InboundStageAction::Debounce) => {
                tracing::debug!(
                    "Post-debounce Debounce action is unexpected; treating as Absorbed"
                );
                InboundProcessOutcome::Absorbed
            }
            Err(StageError::Fatal { stage, source }) => {
                warn!(stage, error = %source, "Post-debounce stage failed");
                InboundProcessOutcome::Failed {
                    stage,
                    error: source.to_string(),
                }
            }
        }
    }

    /// Route a single message through the post-debounce stages and forward it
    /// to the agent execution layer.
    ///
    /// This helper is shared between the background flush loop and explicit
    /// flushes so both paths use exactly the same routing logic.
    async fn route_one(&self, message: IncomingMessage) -> Option<RoutedMessage> {
        let mut ctx = InboundContext::new(message);
        match run_inbound_stages(&self.post_stages, &mut ctx).await {
            Ok(InboundStageAction::Continue) => match build_routed_message(&mut ctx) {
                Ok(routed) => {
                    if let Err(e) = self.routed_tx.send(routed.clone()).await {
                        warn!(error = %e, "Failed to forward routed message to agent execution layer (route_one)");
                    }
                    Some(routed)
                }
                Err(e) => {
                    warn!(error = %e, "Routing failed");
                    None
                }
            },
            Ok(action) => {
                tracing::debug!(?action, "Routing terminal action");
                None
            }
            Err(StageError::Fatal { stage, source }) => {
                warn!(stage, error = %source, "Routing stage failed");
                None
            }
        }
    }

    async fn run_loop(self: Arc<Self>) {
        let mut rx = self.flush_rx.lock().await;
        while let Some(batch) = rx.recv().await {
            for item in batch {
                self.route_one(item.message).await;
            }
        }
    }
}

#[async_trait::async_trait]
impl InboundPipeline for DefaultInboundPipeline {
    async fn process(&self, message: IncomingMessage) -> Option<RoutedMessage> {
        self.process_detailed(message).await.into_option()
    }

    async fn flush(&self, key: &str) -> Vec<RoutedMessage> {
        let messages = self.debouncer.flush_key(key).await;
        let mut routed = Vec::new();
        for msg in messages {
            if let Some(r) = self.route_one(msg).await {
                routed.push(r);
            }
        }
        routed
    }
}
