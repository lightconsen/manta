//! Outbound Pipeline for Manta
//!
//! The outbound pipeline handles everything that happens *after* the agent
//! produces a response.  It mirrors OpenClaw's layered output architecture:
//!
//! ```text
//! Agent Output
//!     -> Trajectory (capture execution trace)
//!     -> Canvas (render dynamic UI)
//!     -> SSE (stream to connected clients)
//!     -> Reply Dispatcher (route to correct channel)
//!     -> Side Effects (memory, cron, webhooks, …)
//! ```

use std::sync::Arc;

pub mod reply_dispatcher;
pub mod side_effects;
pub mod sse;
pub mod trajectory;

pub use reply_dispatcher::{ReplyDispatchConfig, ReplyDispatcher};
pub use side_effects::{SideEffect, SideEffectContext, SideEffectExecutor, SideEffectRegistry};
pub use sse::{SseEvent, SseStreamer};
pub use trajectory::{TrajectoryEntry, TrajectoryLog};

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
}

/// Default outbound pipeline implementation.
///
/// Wires all stages together: trajectory -> canvas -> sse -> reply -> side effects.
pub struct DefaultOutboundPipeline {
    reply_dispatcher: Arc<ReplyDispatcher>,
    side_effects: Arc<SideEffectExecutor>,
    sse: Option<Arc<SseStreamer>>,
}

impl DefaultOutboundPipeline {
    pub fn new(
        reply_dispatcher: Arc<ReplyDispatcher>,
        side_effects: Arc<SideEffectExecutor>,
        sse: Option<Arc<SseStreamer>>,
    ) -> Self {
        Self {
            reply_dispatcher,
            side_effects,
            sse,
        }
    }
}

#[async_trait::async_trait]
impl OutboundPipeline for DefaultOutboundPipeline {
    async fn process(&self, ctx: OutboundContext) -> OutboundResult {
        // Stage 1: Trajectory is already built by the agent; we just forward it.
        let _trajectory = ctx.trajectory;

        // Stage 2: Canvas rendering — detect A2UI components in agent output
        let canvas_update = if let Ok(component) =
            serde_json::from_str::<crate::canvas::CanvasComponent>(&ctx.raw_output
            ) {
            Some(crate::canvas::CanvasUpdate::Init {
                canvas_id: ctx.session_id.clone(),
                root: component,
            })
        } else {
            None
        };

        // Stage 3: SSE streaming — emit tool call and completion events
        let mut sse_events = Vec::new();
        if let Some(ref sse) = self.sse {
            for tc in &ctx.tool_calls {
                let evt = SseEvent::ToolStart {
                    name: tc.function.name.clone(),
                };
                sse.send(&ctx.session_id, evt.clone()).await;
                sse_events.push(evt);
            }
            let done_evt = SseEvent::Done;
            sse.send(&ctx.session_id, done_evt.clone()).await;
            sse_events.push(done_evt);
        }

        // Stage 4: Build the outbound result
        let result = OutboundResult {
            text: ctx.raw_output.clone(),
            canvas_update,
            sse_events,
            side_effects: Vec::new(),
            session_id: ctx.session_id.clone(),
            channel: ctx.channel.clone(),
        };

        // Stage 5: Dispatch via reply dispatcher
        let outbound_msg = crate::channels::OutgoingMessage {
            conversation_id: crate::channels::ConversationId::new(&ctx.session_id),
            content: ctx.raw_output,
            formatted_content: None,
            attachments: vec![],
            reply_to: None,
            options: crate::channels::MessageOptions {
                silent: false,
                show_typing: false,
                custom: std::collections::HashMap::new(),
            },
            usage: ctx.usage,
        };
        if let Err(e) = self.reply_dispatcher.dispatch(&ctx.channel, outbound_msg).await {
            tracing::warn!("Reply dispatch failed for channel {}: {}", ctx.channel, e);
        }

        // Stage 6: Side effects (memory storage, cron, etc.)
        if !result.side_effects.is_empty() {
            self.side_effects.execute_batch(&result.side_effects).await;
        }

        result
    }
}
