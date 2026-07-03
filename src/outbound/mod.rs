//! Outbound Pipeline for Syscity
//!
//! The outbound pipeline handles everything that happens *after* the agent
//! produces a response.
//!
//! ```text
//! Agent Output
//! -> Trajectory (capture execution trace)
//! -> Canvas (render dynamic UI)
//! -> SSE (stream to connected clients)
//! -> Reply Dispatcher (route to correct channel)
//! -> Side Effects (memory, cron, webhooks, …)
//! ```

use std::sync::Arc;

pub mod reply_dispatcher;
pub mod side_effects;
pub mod sse;
pub mod stage;
pub mod trajectory;

pub use reply_dispatcher::{ReplyDispatchConfig, ReplyDispatcher};
pub use side_effects::{SideEffect, SideEffectContext, SideEffectExecutor, SideEffectRegistry};
pub use sse::{SseEvent, SseStreamer};
pub use trajectory::{TrajectoryEntry, TrajectoryLog, TrajectoryWriter};

use self::stage::{default_outbound_stages_from_arcs, OutboundStageContext, run_outbound_stages};
use crate::channels::reply_prefix::ReplyPrefixEngine;

/// A fully-processed outbound result ready for delivery.
#[derive(Debug, Clone)]
pub struct OutboundResult {
    /// The final text to send to the user.
    pub text: String,
    /// Optional canvas update to push.
    pub canvas_update: Option<crate::canvas::CanvasUpdate>,
    /// SSE events that should be streamed.
    pub sse_events: Vec<SseEvent>,
    /// Side effects to execute after delivery.
    pub side_effects: Vec<SideEffect>,
    /// Target session ID.
    pub session_id: String,
    /// Target channel.
    pub channel: String,
}

/// The outbound pipeline trait.
#[async_trait::async_trait]
pub trait OutboundPipeline: Send + Sync {
    /// Process an agent output through the full outbound pipeline.
    async fn process(&self, ctx: OutboundContext) -> OutboundResult;
}

/// Context passed through the outbound pipeline.
#[derive(Debug, Clone)]
pub struct OutboundContext {
    pub session_id: String,
    pub channel: String,
    pub agent_id: String,
    pub raw_output: String,
    pub tool_calls: Vec<crate::providers::ToolCall>,
    pub trajectory: TrajectoryLog,
    /// Optional token usage statistics.
    pub usage: Option<crate::providers::Usage>,
    /// Side effects to execute after the reply is dispatched.
    pub side_effects: Vec<SideEffect>,
}

/// Default outbound pipeline implementation.
///
/// Wires all stages together: trajectory -> canvas -> sse -> reply -> side
/// effects.
pub struct DefaultOutboundPipeline {
    reply_dispatcher: Arc<ReplyDispatcher>,
    side_effects: Arc<SideEffectExecutor>,
    sse: Option<Arc<SseStreamer>>,
    trajectory_writer: Option<Arc<TrajectoryWriter>>,
    /// Optional reply prefix engine for prepending model info / metadata.
    reply_prefix_engine: Option<ReplyPrefixEngine>,
}

impl DefaultOutboundPipeline {
    pub fn new(
        reply_dispatcher: Arc<ReplyDispatcher>,
        side_effects: Arc<SideEffectExecutor>,
        sse: Option<Arc<SseStreamer>>,
        trajectory_writer: Option<Arc<TrajectoryWriter>>,
    ) -> Self {
        Self {
            reply_dispatcher,
            side_effects,
            sse,
            trajectory_writer,
            reply_prefix_engine: None,
        }
    }

    /// Attach a reply prefix engine to prepend model info / metadata
    /// before dispatching messages.
    pub fn with_reply_prefix_engine(mut self, engine: ReplyPrefixEngine) -> Self {
        self.reply_prefix_engine = Some(engine);
        self
    }
}

#[async_trait::async_trait]
impl OutboundPipeline for DefaultOutboundPipeline {
    async fn process(&self, ctx: OutboundContext) -> OutboundResult {
        let stages = default_outbound_stages_from_arcs(
            self.trajectory_writer.clone(),
            self.sse.clone(),
            self.reply_prefix_engine.clone(),
            self.reply_dispatcher.clone(),
            self.side_effects.clone(),
        );

        let mut stage_ctx = OutboundStageContext::new(ctx);
        run_outbound_stages(&stages, &mut stage_ctx).await
    }
}
