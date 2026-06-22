//! Pluggable inbound pipeline stages.
//!
//! Defines the [`InboundStage`] trait and a [`InboundContext`] shared between
//! stages, plus stage wrappers for each existing pipeline step.
//!
//! # Pipeline split
//!
//! The debounce stage is special: it decides whether a message should be
//! absorbed into a pending batch or released immediately. Therefore the
//! pipeline is split into two lists:
//!
//! * **Pre-debounce stages** – run once when a message first enters the
//!   pipeline. Currently: identity validation, debounce.
//! * **Post-debounce stages** – run for every message that leaves the
//!   debouncer (either because it bypassed debounce or because a batch was
//!   flushed). Currently: media, dispatch, envelope, queue, router.
//!
//! This avoids the cycle that would occur if a flushed message ran through the
//! debounce stage a second time.
//!
//! # Stage errors
//!
//! Each stage returns [`Result<InboundStageAction, StageError>`]. Terminal
//! actions (`Suppress`, `Debounce`) are represented as successful `Ok(...)`
//! values because they are expected control-flow outcomes, not failures.
//! [`StageError::Fatal`] is reserved for unexpected failures (database errors,
//! channel send failures, etc.).

use std::sync::Arc;

use async_trait::async_trait;

use super::router::AgentRouter;
use super::{
    debounce::InboundDebouncer, dispatch::DispatchResult, media::MediaUnderstandingResult,
    queue::QueueMode, router::RouteResult, AutoReplyDispatch, MediaUnderstandingPipeline,
    QueueModeResolver, RoutedMessage,
};
use crate::channels::envelope::SessionEnvelopeManager;
use crate::channels::identity::IdentityValidator;
use crate::channels::IncomingMessage;

// ── Errors ───────────────────────────────────────────────────────────────────

/// Errors that can occur while running inbound pipeline stages.
#[derive(Debug, thiserror::Error)]
pub enum StageError {
    /// A stage encountered an unexpected failure. The pipeline should stop
    /// and the error should be logged or propagated.
    #[error("stage '{stage}' failed: {source}")]
    Fatal {
        stage: &'static str,
        #[source]
        source: crate::SyscityError,
    },
}

impl StageError {
    /// Convenience constructor for fatal errors.
    pub fn fatal<S: Into<String>>(stage: &'static str, message: S) -> Self {
        Self::Fatal {
            stage,
            source: crate::SyscityError::Internal(message.into()),
        }
    }
}

// ── Actions ──────────────────────────────────────────────────────────────────

/// What the pipeline should do after this stage returns successfully.
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
///
/// The original message is owned by the context. Stages may replace it only
/// when they are producing a derived representation of the same message
/// (e.g. the debounce stage collapses a batch into a single synthetic
/// message). Stages must not mutate message identifiers such as `id`,
/// `user_id`, or `conversation_id`.
#[derive(Debug, Clone)]
pub struct InboundContext {
    /// The incoming message currently being processed.
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
    fn name(&self) -> &'static str;

    /// Process a message. Return [`InboundStageAction::Continue`] to proceed,
    /// [`InboundStageAction::Suppress`] to drop, or
    /// [`InboundStageAction::Debounce`] to absorb into the debouncer.
    ///
    /// Returning [`StageError::Fatal`] aborts the entire pipeline; callers
    /// should log the error and discard the message.
    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError>;
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
    fn name(&self) -> &'static str {
        "identity"
    }

    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError> {
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
        Ok(InboundStageAction::Continue)
    }
}

/// Debounce stage – absorbs messages that should be batched.
///
/// This stage belongs **only** in the pre-debounce pipeline. If it returns
/// [`InboundStageAction::Debounce`], the caller must not run post-debounce
/// stages until the debouncer emits the message later.
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
    fn name(&self) -> &'static str {
        "debounce"
    }

    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError> {
        match self.debouncer.enqueue(ctx.message.clone()).await {
            Some(debounced) => {
                ctx.message = debounced;
                Ok(InboundStageAction::Continue)
            }
            None => Ok(InboundStageAction::Debounce),
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
    fn name(&self) -> &'static str {
        "media"
    }

    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError> {
        ctx.media_result = if !ctx.message.attachments.is_empty() {
            Some(self.pipeline.process(&ctx.message).await)
        } else {
            None
        };
        Ok(InboundStageAction::Continue)
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

    /// Provide an already-created Arc to share ownership.
    pub fn from_arc(dispatch: Arc<AutoReplyDispatch>) -> Self {
        Self { dispatch }
    }
}

#[async_trait]
impl InboundStage for DispatchStage {
    fn name(&self) -> &'static str {
        "dispatch"
    }

    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError> {
        let result = self
            .dispatch
            .process(&ctx.message, ctx.media_result.as_ref())
            .await;
        let suppress = result.suppress;
        ctx.dispatch_result = Some(result);
        if suppress {
            Ok(InboundStageAction::Suppress)
        } else {
            Ok(InboundStageAction::Continue)
        }
    }
}

/// Session envelope tracking stage.
pub struct EnvelopeStage {
    envelope_manager: Arc<SessionEnvelopeManager>,
}

impl EnvelopeStage {
    pub fn new(envelope_manager: SessionEnvelopeManager) -> Self {
        Self {
            envelope_manager: Arc::new(envelope_manager),
        }
    }
}

#[async_trait]
impl InboundStage for EnvelopeStage {
    fn name(&self) -> &'static str {
        "envelope"
    }

    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError> {
        let _envelope = self
            .envelope_manager
            .get_or_create(&ctx.message.conversation_id.0)
            .await;
        ctx.envelope_updated = true;
        Ok(InboundStageAction::Continue)
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
    fn name(&self) -> &'static str {
        "queue"
    }

    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError> {
        ctx.queue_mode = Some(self.resolver.resolve(&ctx.message).await);
        Ok(InboundStageAction::Continue)
    }
}

/// Agent routing stage – determines which agent handles the message.
///
/// This stage only populates `ctx.route_result`. The caller is responsible for
/// constructing the final [`RoutedMessage`] and forwarding it to the agent
/// execution layer. Keeping routing and dispatch separate makes the stage
/// easier to test and avoids giving a stage direct access to an outbound
/// channel.
pub struct RouterStage {
    router: Arc<AgentRouter>,
}

impl RouterStage {
    pub fn new(router: AgentRouter) -> Self {
        Self { router: Arc::new(router) }
    }

    /// Create a router stage from an existing `Arc` to share ownership.
    pub fn from_arc(router: Arc<AgentRouter>) -> Self {
        Self { router }
    }
}

#[async_trait]
impl InboundStage for RouterStage {
    fn name(&self) -> &'static str {
        "router"
    }

    async fn process(&self, ctx: &mut InboundContext) -> Result<InboundStageAction, StageError> {
        let workspace_hint = ctx
            .dispatch_result
            .as_ref()
            .and_then(|d| d.workspace_hint.as_deref());
        let route = self.router.route(&ctx.message, workspace_hint).await;
        ctx.route_result = Some(route);
        Ok(InboundStageAction::Continue)
    }
}

// ── Routed message construction ──────────────────────────────────────────────

/// Build a [`RoutedMessage`] from a fully processed context.
///
/// # Panics
///
/// Panics if the router stage has not populated `ctx.route_result`. Callers
/// must only invoke this after running the post-debounce stages.
pub fn build_routed_message(ctx: &mut InboundContext) -> RoutedMessage {
    let route = ctx
        .route_result
        .clone()
        .expect("router stage must run before build_routed_message");

    RoutedMessage {
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
    }
}

// ── Stage runner ─────────────────────────────────────────────────────────────

/// Run a slice of stages sequentially.
///
/// Returns `Ok(())` if all stages returned [`InboundStageAction::Continue`].
/// Returns the terminal action as `Ok(Suppress)` or `Ok(Debounce)` if a stage
/// stopped the pipeline. Returns [`StageError::Fatal`] if a stage failed.
pub async fn run_inbound_stages(
    stages: &[Box<dyn InboundStage>],
    ctx: &mut InboundContext,
) -> Result<InboundStageAction, StageError> {
    for stage in stages {
        match stage.process(ctx).await? {
            InboundStageAction::Continue => continue,
            action @ (InboundStageAction::Suppress | InboundStageAction::Debounce) => {
                tracing::debug!(stage = stage.name(), ?action, "Inbound pipeline terminal action");
                return Ok(action);
            }
        }
    }
    Ok(InboundStageAction::Continue)
}

// ── Default stage list helpers ───────────────────────────────────────────────

/// Build the default list of **pre-debounce** inbound stages.
///
/// Stages: Identity (if `validator` is provided) → Debounce
pub fn default_pre_debounce_stages(
    validator: Option<IdentityValidator>,
    debouncer: Arc<InboundDebouncer>,
) -> Vec<Box<dyn InboundStage>> {
    let mut stages: Vec<Box<dyn InboundStage>> = Vec::new();
    if let Some(validator) = validator {
        stages.push(Box::new(IdentityStage::new(validator)));
    }
    stages.push(Box::new(DebounceStage::new(debouncer)));
    stages
}

/// Build the default list of **post-debounce** inbound stages.
///
/// Stages: Media → Dispatch → Envelope (if `envelope_manager` is provided)
/// → Queue → Router
pub fn default_post_debounce_stages(
    media_pipeline: MediaUnderstandingPipeline,
    dispatch: AutoReplyDispatch,
    envelope_manager: Option<SessionEnvelopeManager>,
    queue_resolver: QueueModeResolver,
    router: Arc<AgentRouter>,
) -> Vec<Box<dyn InboundStage>> {
    let mut stages: Vec<Box<dyn InboundStage>> = vec![
        Box::new(MediaStage::new(media_pipeline)),
        Box::new(DispatchStage::new(dispatch)),
    ];
    if let Some(envelope_manager) = envelope_manager {
        stages.push(Box::new(EnvelopeStage::new(envelope_manager)));
    }
    stages.push(Box::new(QueueStage::new(queue_resolver)));
    stages.push(Box::new(RouterStage::from_arc(router)));
    stages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::IncomingMessage;
    use crate::channels::MessageMetadata;

    #[allow(unused_imports)]
    use crate::inbound::dispatch::AutoReplyDispatchConfig;

    /// A stage that always suppresses.
    struct SuppressStage;

    #[async_trait]
    impl InboundStage for SuppressStage {
        fn name(&self) -> &'static str {
            "suppress"
        }
        async fn process(
            &self,
            _ctx: &mut InboundContext,
        ) -> Result<InboundStageAction, StageError> {
            Ok(InboundStageAction::Suppress)
        }
    }

    /// A stage that always continues.
    struct PassStage;

    #[async_trait]
    impl InboundStage for PassStage {
        fn name(&self) -> &'static str {
            "pass"
        }
        async fn process(
            &self,
            _ctx: &mut InboundContext,
        ) -> Result<InboundStageAction, StageError> {
            Ok(InboundStageAction::Continue)
        }
    }

    /// A stage that records how many times it ran.
    struct CountStage {
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl InboundStage for CountStage {
        fn name(&self) -> &'static str {
            "count"
        }
        async fn process(
            &self,
            _ctx: &mut InboundContext,
        ) -> Result<InboundStageAction, StageError> {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(InboundStageAction::Continue)
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
        let stages: Vec<Box<dyn InboundStage>> = vec![Box::new(PassStage), Box::new(PassStage)];
        let mut ctx = InboundContext::new(dummy_message());
        let result = run_inbound_stages(&stages, &mut ctx).await;
        assert_eq!(result.unwrap(), InboundStageAction::Continue);
    }

    #[tokio::test]
    async fn test_suppress_early_exit() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stages: Vec<Box<dyn InboundStage>> = vec![
            Box::new(CountStage { counter: counter.clone() }),
            Box::new(SuppressStage),
            Box::new(CountStage { counter: counter.clone() }),
        ];
        let mut ctx = InboundContext::new(dummy_message());
        let result = run_inbound_stages(&stages, &mut ctx).await;
        assert_eq!(result.unwrap(), InboundStageAction::Suppress);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_debounce_early_exit() {
        struct DebounceStage_;
        #[async_trait]
        impl InboundStage for DebounceStage_ {
            fn name(&self) -> &'static str {
                "db"
            }
            async fn process(
                &self,
                _: &mut InboundContext,
            ) -> Result<InboundStageAction, StageError> {
                Ok(InboundStageAction::Debounce)
            }
        }

        let stages: Vec<Box<dyn InboundStage>> = vec![
            Box::new(PassStage),
            Box::new(DebounceStage_),
            Box::new(PassStage),
        ];
        let mut ctx = InboundContext::new(dummy_message());
        let result = run_inbound_stages(&stages, &mut ctx).await;
        assert_eq!(result.unwrap(), InboundStageAction::Debounce);
    }

    #[tokio::test]
    async fn test_fatal_stage_error() {
        struct ErrorStage;
        #[async_trait]
        impl InboundStage for ErrorStage {
            fn name(&self) -> &'static str {
                "error"
            }
            async fn process(
                &self,
                _: &mut InboundContext,
            ) -> Result<InboundStageAction, StageError> {
                Err(StageError::fatal("error", "simulated failure"))
            }
        }

        let stages: Vec<Box<dyn InboundStage>> = vec![Box::new(PassStage), Box::new(ErrorStage)];
        let mut ctx = InboundContext::new(dummy_message());
        let result = run_inbound_stages(&stages, &mut ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("error"));
        assert!(err.contains("simulated failure"));
    }

    #[tokio::test]
    async fn test_dispatch_stage_suppresses() {
        let dispatch = AutoReplyDispatch::new(AutoReplyDispatchConfig {
            suppress_unless_mentioned_in_groups: true,
            ..Default::default()
        });
        let stage = DispatchStage::new(dispatch);
        let mut msg = dummy_message();
        msg.provenance = crate::channels::InputProvenance::ExternalUser {
            channel: "test".into(),
            is_direct: false,
        };
        msg.mention = crate::channels::MentionState::NotMentioned;

        let mut ctx = InboundContext::new(msg);
        let action = stage.process(&mut ctx).await.unwrap();
        assert_eq!(action, InboundStageAction::Suppress);
        assert!(ctx.dispatch_result.is_some());
    }

    #[tokio::test]
    async fn test_queue_stage_sets_mode() {
        let stage = QueueStage::new(QueueModeResolver::new());
        let mut ctx = InboundContext::new(IncomingMessage::new("u1", "s1", "!stop"));
        let action = stage.process(&mut ctx).await.unwrap();
        assert_eq!(action, InboundStageAction::Continue);
        assert_eq!(ctx.queue_mode, Some(QueueMode::Interrupt));
    }

    #[tokio::test]
    async fn test_build_routed_message_requires_route() {
        let mut ctx = InboundContext::new(dummy_message());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_routed_message(&mut ctx)
        }));
        assert!(result.is_err(), "build_routed_message must panic without route_result");
    }
}
