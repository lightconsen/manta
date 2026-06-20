//! Pluggable inbound pipeline stages.
//!
//! Defines the [`InboundStage`] trait and a [`InboundContext`] shared between
//! stages, plus stage wrappers for each existing pipeline step.
//!
//! The default stage list mirrors the current hard-coded order:
//! `Identity → Debounce → Media → Dispatch → Envelope → Queue → Router`
//!
//! Gateway wiring is *not* reworked here – the new Vec-based runner is tested
//! in unit tests only. See `docs/modules/channels.md`.

use async_trait::async_trait;
use std::sync::Arc;

use super::{
    debounce::InboundDebouncer, dispatch::DispatchResult, media::MediaUnderstandingResult,
    queue::QueueMode, router::RouteResult, AutoReplyDispatch,
    MediaUnderstandingPipeline, QueueModeResolver, RoutedMessage,
};
use super::router::AgentRouter;
use crate::channels::identity::IdentityValidator;
use crate::channels::envelope::SessionEnvelopeManager;
use crate::channels::IncomingMessage;

// ── Actions ──────────────────────────────────────────────────────────────────

/// What the pipeline should do after this stage returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundStageAction {
    /// Continue to the next stage normally.
    Continue,
    /// Drop the message entirely (used by dispatch suppress).
    Suppress,
    /// Absorb into the debouncer (message will be re-emitted later).
    Debounce,
}

// ── Context ──────────────────────────────────────────────────────────────────

/// Shared mutable state threaded through all inbound stages.
#[derive(Debug, Clone)]
pub struct InboundContext {
    /// The original incoming message (may be mutated by stages).
    pub message: IncomingMessage,
    /// Result of media understanding (populated by [`MediaStage`]).
    pub media_result: Option<MediaUnderstandingResult>,
    /// Result of the dispatch stage.
    pub dispatch_result: Option<DispatchResult>,
    /// Resolved queue mode.
    pub queue_mode: Option<QueueMode>,
    /// Result of agent routing.
    pub route_result: Option<RouteResult>,
    /// Whether the session envelope was updated.
    pub envelope_updated: bool,
}

impl InboundContext {
    pub fn new(message: IncomingMessage) -> Self {
        Self {
            message,
            media_result: None,
            dispatch_result: None,
            queue_mode: None,
            route_result: None,
            envelope_updated: false,
        }
    }
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// A single stage in the inbound pipeline.
///
/// Stages are executed in order. Each stage inspects or mutates
/// [`InboundContext`] and returns an [`InboundStageAction`] to indicate
/// whether the pipeline should continue or stop early.
#[async_trait]
pub trait InboundStage: Send + Sync {
    /// Unique name for this stage (used for logging and ordering).
    fn name(&self) -> &str;

    /// Process a message. Return [`InboundStageAction::Continue`] to proceed,
    /// [`InboundStageAction::Suppress`] to drop, or [`InboundStageAction::Debounce`]
    /// to absorb into the debouncer.
    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction;
}

// ── Built-in stage wrappers ──────────────────────────────────────────────────

/// Identity validation stage (warn-only, never drops messages).
pub struct IdentityStage {
    validator: IdentityValidator,
}

impl IdentityStage {
    pub fn new(validator: IdentityValidator) -> Self {
        Self { validator }
    }
}

#[async_trait]
impl InboundStage for IdentityStage {
    fn name(&self) -> &str {
        "identity"
    }

    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction {
        let identity = crate::channels::identity::SenderIdentity {
            user_id: ctx.message.user_id.0.clone(),
            display_name: None,
            username: None,
            phone: None,
            email: None,
            raw: None,
            platform_data: None,
        };
        if let Err(err) = self.validator.validate(&identity) {
            tracing::warn!(
                message_id = %ctx.message.id,
                user_id = %ctx.message.user_id,
                conversation_id = %ctx.message.conversation_id,
                reason = %err,
                "Identity validation failed (message not dropped)"
            );
        }
        InboundStageAction::Continue
    }
}

/// Debounce stage – absorbs messages that should be batched.
pub struct DebounceStage {
    debouncer: Arc<InboundDebouncer>,
}

impl DebounceStage {
    pub fn new(debouncer: Arc<InboundDebouncer>) -> Self {
        Self { debouncer }
    }
}

#[async_trait]
impl InboundStage for DebounceStage {
    fn name(&self) -> &str {
        "debounce"
    }

    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction {
        // Clone the message and hand it to the debouncer.
        // If the debouncer absorbs it, we signal Debounce and the pipeline
        // stops (the stale message in ctx is never read by subsequent stages).
        let message = ctx.message.clone();
        if let Some(debounced) = self.debouncer.enqueue(message).await {
            ctx.message = debounced;
            InboundStageAction::Continue
        } else {
            InboundStageAction::Debounce
        }
    }
}

/// Media understanding stage.
pub struct MediaStage {
    pipeline: MediaUnderstandingPipeline,
}

impl MediaStage {
    pub fn new(pipeline: MediaUnderstandingPipeline) -> Self {
        Self { pipeline }
    }
}

#[async_trait]
impl InboundStage for MediaStage {
    fn name(&self) -> &str {
        "media"
    }

    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction {
        ctx.media_result = if !ctx.message.attachments.is_empty() {
            Some(self.pipeline.process(&ctx.message).await)
        } else {
            None
        };
        InboundStageAction::Continue
    }
}

/// Dispatch stage – applies send policies, plugin bindings, may suppress.
pub struct DispatchStage {
    dispatch: Arc<AutoReplyDispatch>,
}

impl DispatchStage {
    pub fn new(dispatch: AutoReplyDispatch) -> Self {
        Self { dispatch: Arc::new(dispatch) }
    }
}

impl DispatchStage {
    /// Provide an already-created Arc to share ownership.
    pub fn from_arc(dispatch: Arc<AutoReplyDispatch>) -> Self {
        Self { dispatch }
    }
}

#[async_trait]
impl InboundStage for DispatchStage {
    fn name(&self) -> &str {
        "dispatch"
    }

    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction {
        let result = self
            .dispatch
            .process(&ctx.message, ctx.media_result.as_ref())
            .await;
        let suppress = result.suppress;
        ctx.dispatch_result = Some(result);
        if suppress {
            InboundStageAction::Suppress
        } else {
            InboundStageAction::Continue
        }
    }
}

/// Session envelope tracking stage.
pub struct EnvelopeStage {
    envelope_manager: Arc<SessionEnvelopeManager>,
}

impl EnvelopeStage {
    pub fn new(envelope_manager: SessionEnvelopeManager) -> Self {
        Self { envelope_manager: Arc::new(envelope_manager) }
    }
}

#[async_trait]
impl InboundStage for EnvelopeStage {
    fn name(&self) -> &str {
        "envelope"
    }

    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction {
        let _envelope = self
            .envelope_manager
            .get_or_create(&ctx.message.conversation_id.0)
            .await;
        ctx.envelope_updated = true;
        InboundStageAction::Continue
    }
}

/// Queue mode resolution stage.
pub struct QueueStage {
    resolver: QueueModeResolver,
}

impl QueueStage {
    pub fn new(resolver: QueueModeResolver) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl InboundStage for QueueStage {
    fn name(&self) -> &str {
        "queue"
    }

    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction {
        ctx.queue_mode = Some(self.resolver.resolve(&ctx.message).await);
        InboundStageAction::Continue
    }
}

/// Agent routing stage – determines which agent handles the message.
pub struct RouterStage {
    router: Arc<AgentRouter>,
    routed_tx: tokio::sync::mpsc::Sender<RoutedMessage>,
}

impl RouterStage {
    pub fn new(router: AgentRouter, routed_tx: tokio::sync::mpsc::Sender<RoutedMessage>) -> Self {
        Self { router: Arc::new(router), routed_tx }
    }
}

#[async_trait]
impl InboundStage for RouterStage {
    fn name(&self) -> &str {
        "router"
    }

    async fn process(&self, ctx: &mut InboundContext) -> InboundStageAction {
        let workspace_hint = ctx
            .dispatch_result
            .as_ref()
            .and_then(|d| d.workspace_hint.as_deref());
        let route: RouteResult = self.router.route(&ctx.message, workspace_hint).await;
        ctx.route_result = Some(route.clone());

        // Build and forward the routed message
        let routed = RoutedMessage {
            incoming: ctx.message.clone(),
            agent_id: route.agent_id,
            workspace_id: route.workspace_id,
            queue_mode: ctx.queue_mode.unwrap_or(QueueMode::Interrupt),
            suppress_delivery: ctx
                .dispatch_result
                .as_ref()
                .map(|d| d.suppress)
                .unwrap_or(false),
            media_results: ctx.media_result.take(),
        };

        let _ = self.routed_tx.send(routed).await;
        InboundStageAction::Continue
    }
}

// ── Stage runner ─────────────────────────────────────────────────────────────

/// Run a slice of stages sequentially.
///
/// Returns `None` if any stage returned [`InboundStageAction::Suppress`] or
/// [`InboundStageAction::Debounce`], or `Some(())` if all stages completed.
pub async fn run_inbound_stages(
    stages: &[Box<dyn InboundStage>],
    ctx: &mut InboundContext,
) -> Option<()> {
    for stage in stages {
        match stage.process(ctx).await {
            InboundStageAction::Continue => continue,
            InboundStageAction::Suppress => {
                tracing::debug!("Inbound message suppressed by stage '{}'", stage.name());
                return None;
            }
            InboundStageAction::Debounce => {
                tracing::debug!("Inbound message debounced by stage '{}'", stage.name());
                return None;
            }
        }
    }
    Some(())
}

// ── Default stage list helper ────────────────────────────────────────────────

/// Build the default list of inbound stages matching the current pipeline order.
///
/// Stages: Identity → Debounce → Media → Dispatch → Envelope → Queue → Router
#[allow(clippy::too_many_arguments)]
pub fn default_inbound_stages(
    validator: IdentityValidator,
    debouncer: Arc<InboundDebouncer>,
    media_pipeline: MediaUnderstandingPipeline,
    dispatch: AutoReplyDispatch,
    envelope_manager: SessionEnvelopeManager,
    queue_resolver: QueueModeResolver,
    router: AgentRouter,
    routed_tx: tokio::sync::mpsc::Sender<RoutedMessage>,
) -> Vec<Box<dyn InboundStage>> {
    vec![
        Box::new(IdentityStage::new(validator)),
        Box::new(DebounceStage::new(debouncer)),
        Box::new(MediaStage::new(media_pipeline)),
        Box::new(DispatchStage::new(dispatch)),
        Box::new(EnvelopeStage::new(envelope_manager)),
        Box::new(QueueStage::new(queue_resolver)),
        Box::new(RouterStage::new(router, routed_tx)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::IncomingMessage;
    use crate::channels::MessageMetadata;

    /// A stage that always suppresses.
    struct SuppressStage;

    #[async_trait]
    impl InboundStage for SuppressStage {
        fn name(&self) -> &str {
            "suppress"
        }
        async fn process(&self, _ctx: &mut InboundContext) -> InboundStageAction {
            InboundStageAction::Suppress
        }
    }

    /// A stage that always continues.
    struct PassStage;

    #[async_trait]
    impl InboundStage for PassStage {
        fn name(&self) -> &str {
            "pass"
        }
        async fn process(&self, _ctx: &mut InboundContext) -> InboundStageAction {
            InboundStageAction::Continue
        }
    }

    fn dummy_message() -> IncomingMessage {
        IncomingMessage {
            id: crate::core::models::Id::new(),
            user_id: crate::channels::UserId::new("user1"),
            conversation_id: crate::channels::ConversationId::new("conv1"),
            content: "hello".into(),
            attachments: vec![],
            metadata: MessageMetadata::new(),
            provenance: crate::channels::InputProvenance::ExternalUser {
                channel: "test".into(),
                is_direct: true,
            },
            mention: crate::channels::MentionState::DirectMessage,
        }
    }

    #[tokio::test]
    async fn test_all_continue() {
        let stages: Vec<Box<dyn InboundStage>> = vec![
            Box::new(PassStage),
            Box::new(PassStage),
        ];
        let mut ctx = InboundContext::new(dummy_message());
        let result = run_inbound_stages(&stages, &mut ctx).await;
        assert!(result.is_some(), "all stages continue => Some(())");
    }

    #[tokio::test]
    async fn test_suppress_early_exit() {
        let stages: Vec<Box<dyn InboundStage>> = vec![
            Box::new(PassStage),
            Box::new(SuppressStage), // should stop here
            Box::new(PassStage),     // should never run
        ];
        let mut ctx = InboundContext::new(dummy_message());
        let result = run_inbound_stages(&stages, &mut ctx).await;
        assert!(result.is_none(), "suppress stage => None");
    }

    #[tokio::test]
    async fn test_debounce_early_exit() {
        // Create a stage that signals Debounce
        struct DebounceStage_;
        #[async_trait]
        impl InboundStage for DebounceStage_ {
            fn name(&self) -> &str { "db" }
            async fn process(&self, _: &mut InboundContext) -> InboundStageAction {
                InboundStageAction::Debounce
            }
        }

        let stages: Vec<Box<dyn InboundStage>> = vec![
            Box::new(PassStage),
            Box::new(DebounceStage_),
            Box::new(PassStage),
        ];
        let mut ctx = InboundContext::new(dummy_message());
        let result = run_inbound_stages(&stages, &mut ctx).await;
        assert!(result.is_none(), "debounce stage => None");
    }
}