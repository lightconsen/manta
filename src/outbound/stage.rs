//! Pluggable outbound pipeline stages.
//!
//! Defines the [`OutboundStage`] trait and shared context, plus stage wrappers
//! for each existing pipeline step.
//!
//! The default stage list mirrors the current hard-coded order:
//! `Trajectory → Canvas → SSE → ReplyPrefix → Dispatch → SideEffects`

use std::sync::Arc;

use async_trait::async_trait;

use super::{
    OutboundContext, OutboundResult, ReplyDispatcher, SideEffectExecutor, SseEvent, SseStreamer,
    TrajectoryWriter,
};
use crate::canvas::CanvasUpdate;
use crate::channels::reply_prefix::{ReplyPrefixEngine, TemplateContext};

// ── Actions ──────────────────────────────────────────────────────────────────

/// What the pipeline should do after this stage returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundStageAction {
    /// Continue to the next stage normally.
    Continue,
    /// Skip dispatch to the channel (result is still returned).
    SkipDispatch,
}

// ── Context ──────────────────────────────────────────────────────────────────

/// Shared mutable state threaded through all outbound stages.
///
/// Holds both the original [`OutboundContext`] (input) and the accumulating
/// [`OutboundResult`] (output).
#[derive(Debug, Clone)]
pub struct OutboundStageContext {
    /// Original pipeline input (immutable after construction).
    pub input: OutboundContext,
    /// Accumulating pipeline output.
    pub result: OutboundResult,
    /// Whether the final dispatch stage should run.
    pub skip_dispatch: bool,
}

impl OutboundStageContext {
    pub fn new(input: OutboundContext) -> Self {
        let result = OutboundResult {
            text: input.raw_output.clone(),
            canvas_update: None,
            sse_events: Vec::new(),
            side_effects: Vec::new(),
            session_id: input.session_id.clone(),
            channel: input.channel.clone(),
        };
        Self {
            input,
            result,
            skip_dispatch: false,
        }
    }
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// A single stage in the outbound pipeline.
///
/// Stages are executed in order. Each stage inspects or mutates
/// [`OutboundStageContext`] and returns an [`OutboundStageAction`] to
/// indicate whether the pipeline should continue.
#[async_trait]
pub trait OutboundStage: Send + Sync {
    /// Unique name for this stage (used for logging and ordering).
    fn name(&self) -> &str;

    /// Process the outbound data. Return [`OutboundStageAction::Continue`] to
    /// proceed, or [`OutboundStageAction::SkipDispatch`] to skip channel
    /// dispatch.
    async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction;
}

// ── Built-in stage wrappers ──────────────────────────────────────────────────

/// Trajectory persistence stage.
pub struct TrajectoryStage {
    writer: Arc<TrajectoryWriter>,
}

impl TrajectoryStage {
    pub fn new(writer: TrajectoryWriter) -> Self {
        Self { writer: Arc::new(writer) }
    }
}

#[async_trait]
impl OutboundStage for TrajectoryStage {
    fn name(&self) -> &str {
        "trajectory"
    }

    async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction {
        if !ctx.input.trajectory.entries.is_empty() {
            if let Err(e) = self
                .writer
                .append_log(&ctx.input.session_id, &ctx.input.trajectory)
                .await
            {
                tracing::warn!(
                    "Failed to persist trajectory for session {}: {}",
                    ctx.input.session_id,
                    e
                );
            }
        }
        OutboundStageAction::Continue
    }
}

/// Canvas rendering stage – detects A2UI components in agent output.
pub struct CanvasStage;

#[async_trait]
impl OutboundStage for CanvasStage {
    fn name(&self) -> &str {
        "canvas"
    }

    async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction {
        ctx.result.canvas_update = serde_json::from_str::<CanvasComponent>(&ctx.result.text)
            .ok()
            .map(|component| CanvasUpdate::Init {
                canvas_id: ctx.input.session_id.clone(),
                root: component,
            });
        OutboundStageAction::Continue
    }
}

// Re-use the canvas type
use crate::canvas::CanvasComponent;

/// SSE streaming stage – emits tool call and completion events.
pub struct SseStage {
    sse: Arc<SseStreamer>,
}

impl SseStage {
    pub fn new(sse: SseStreamer) -> Self {
        Self { sse: Arc::new(sse) }
    }
}

#[async_trait]
impl OutboundStage for SseStage {
    fn name(&self) -> &str {
        "sse"
    }

    async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction {
        for tc in &ctx.input.tool_calls {
            let evt = SseEvent::ToolStart { name: tc.function.name.clone() };
            self.sse.send(&ctx.input.session_id, evt.clone()).await;
            ctx.result.sse_events.push(evt);
        }
        let done_evt = SseEvent::Done;
        self.sse.send(&ctx.input.session_id, done_evt.clone()).await;
        ctx.result.sse_events.push(done_evt);
        OutboundStageAction::Continue
    }
}

/// Reply prefix stage – prepends model info / metadata to the text.
pub struct ReplyPrefixStage {
    engine: ReplyPrefixEngine,
}

impl ReplyPrefixStage {
    pub fn new(engine: ReplyPrefixEngine) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl OutboundStage for ReplyPrefixStage {
    fn name(&self) -> &str {
        "reply_prefix"
    }

    async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction {
        let template_ctx = TemplateContext::new()
            .with_session(&ctx.input.session_id)
            .with_channel(&ctx.input.channel);
        let prefixed = self
            .engine
            .apply_async(&ctx.result.text, &template_ctx, Some(&ctx.input.channel))
            .await;
        ctx.result.text = prefixed;
        OutboundStageAction::Continue
    }
}

/// Channel dispatch stage – routes the final message to the correct channel.
pub struct DispatchStage {
    dispatcher: Arc<ReplyDispatcher>,
}

impl DispatchStage {
    pub fn new(dispatcher: ReplyDispatcher) -> Self {
        Self {
            dispatcher: Arc::new(dispatcher),
        }
    }
}

#[async_trait]
impl OutboundStage for DispatchStage {
    fn name(&self) -> &str {
        "dispatch"
    }

    async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction {
        if ctx.skip_dispatch {
            return OutboundStageAction::SkipDispatch;
        }

        let outbound_msg = crate::channels::OutgoingMessage {
            conversation_id: crate::channels::ConversationId::new(&ctx.input.session_id),
            content: ctx.result.text.clone(),
            reasoning_content: None,
            tool_calls: None,
            formatted_content: None,
            attachments: vec![],
            reply_to: None,
            options: crate::channels::MessageOptions {
                silent: false,
                show_typing: false,
                custom: std::collections::HashMap::new(),
            },
            usage: ctx.input.usage,
        };
        if let Err(e) = self
            .dispatcher
            .dispatch(&ctx.input.channel, outbound_msg)
            .await
        {
            tracing::warn!("Reply dispatch failed for channel {}: {}", ctx.input.channel, e);
        }
        OutboundStageAction::Continue
    }
}

/// Side effects stage – memory storage, cron, webhooks, etc.
pub struct SideEffectStage {
    executor: Arc<SideEffectExecutor>,
}

impl SideEffectStage {
    pub fn new(executor: SideEffectExecutor) -> Self {
        Self { executor: Arc::new(executor) }
    }
}

#[async_trait]
impl OutboundStage for SideEffectStage {
    fn name(&self) -> &str {
        "side_effects"
    }

    async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction {
        if !ctx.result.side_effects.is_empty() {
            self.executor.execute_batch(&ctx.result.side_effects).await;
        }
        OutboundStageAction::Continue
    }
}

// ── Stage runner ─────────────────────────────────────────────────────────────

/// Run a slice of outbound stages sequentially.
///
/// Returns `Some(OutboundResult)` if all stages completed (potentially
/// skipping dispatch), or the partial result if `SkipDispatch` was returned.
pub async fn run_outbound_stages(
    stages: &[Box<dyn OutboundStage>],
    ctx: &mut OutboundStageContext,
) -> OutboundResult {
    for stage in stages {
        match stage.process(ctx).await {
            OutboundStageAction::Continue => continue,
            OutboundStageAction::SkipDispatch => {
                tracing::debug!("Outbound dispatch skipped by stage '{}'", stage.name());
                ctx.skip_dispatch = true;
            }
        }
    }
    ctx.result.clone()
}

// ── Default stage list helper ────────────────────────────────────────────────

/// Build the default list of outbound stages matching the current pipeline
/// order.
///
/// Stages: Trajectory → Canvas → SSE → ReplyPrefix → Dispatch → SideEffects
pub fn default_outbound_stages(
    trajectory_writer: Option<TrajectoryWriter>,
    sse: Option<SseStreamer>,
    reply_prefix_engine: Option<ReplyPrefixEngine>,
    dispatcher: ReplyDispatcher,
    side_effects: SideEffectExecutor,
) -> Vec<Box<dyn OutboundStage>> {
    let mut stages: Vec<Box<dyn OutboundStage>> = Vec::new();

    if let Some(writer) = trajectory_writer {
        stages.push(Box::new(TrajectoryStage::new(writer)));
    }
    stages.push(Box::new(CanvasStage));

    if let Some(sse) = sse {
        stages.push(Box::new(SseStage::new(sse)));
    }
    if let Some(engine) = reply_prefix_engine {
        stages.push(Box::new(ReplyPrefixStage::new(engine)));
    }

    stages.push(Box::new(DispatchStage::new(dispatcher)));
    stages.push(Box::new(SideEffectStage::new(side_effects)));
    stages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound::TrajectoryLog;

    /// A stage that always continues.
    struct PassStage;

    #[async_trait]
    impl OutboundStage for PassStage {
        fn name(&self) -> &str {
            "pass"
        }
        async fn process(&self, _ctx: &mut OutboundStageContext) -> OutboundStageAction {
            OutboundStageAction::Continue
        }
    }

    /// A stage that signals SkipDispatch.
    struct SkipStage;

    #[async_trait]
    impl OutboundStage for SkipStage {
        fn name(&self) -> &str {
            "skip"
        }
        async fn process(&self, ctx: &mut OutboundStageContext) -> OutboundStageAction {
            ctx.skip_dispatch = true;
            OutboundStageAction::SkipDispatch
        }
    }

    fn dummy_context() -> OutboundStageContext {
        OutboundStageContext::new(OutboundContext {
            session_id: "sess-1".into(),
            channel: "test".into(),
            agent_id: "agent-1".into(),
            raw_output: "hello world".into(),
            tool_calls: vec![],
            trajectory: TrajectoryLog { entries: vec![] },
            usage: None,
        })
    }

    #[tokio::test]
    async fn test_all_continue() {
        let stages: Vec<Box<dyn OutboundStage>> = vec![Box::new(PassStage), Box::new(PassStage)];
        let mut ctx = dummy_context();
        let result = run_outbound_stages(&stages, &mut ctx).await;
        assert_eq!(result.text, "hello world");
        assert!(!ctx.skip_dispatch);
    }

    #[tokio::test]
    async fn test_skip_dispatch() {
        let stages: Vec<Box<dyn OutboundStage>> = vec![Box::new(PassStage), Box::new(SkipStage)];
        let mut ctx = dummy_context();
        let result = run_outbound_stages(&stages, &mut ctx).await;
        assert_eq!(result.text, "hello world");
        assert!(ctx.skip_dispatch);
    }
}
