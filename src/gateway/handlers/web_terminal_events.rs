#![allow(unused_imports)]

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::{Agent, AgentConfig};
use crate::canvas::{CanvasEvent, CanvasManager};
use crate::channels::{Channel, ChannelExtension, ChannelType};
use crate::config::hot_reload::{ConfigFileType, HotReloadManager};
use crate::inbound::*;
use crate::memory::vector::{
    ApiEmbeddingProvider, CachedEmbeddingProvider, EmbeddingConfig, LocalGgufEmbeddingProvider,
    MemoryVectorStore, VectorMemoryService,
};
use crate::model_router::ModelRouter;
use crate::plugins::PluginManager;
use crate::security::pairing::DmPolicy;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter, ApprovalQueue};
use crate::tools::mcp::{McpManager, McpSettings, McpToolWrapper};
use crate::tools::ToolRegistry;
use crate::gateway::GatewayState;
use crate::gateway::*;

/// SSE events handler for web terminal
/// Streams gateway events to the browser in the format expected by the web UI
#[allow(dead_code)]
pub async fn web_terminal_events_handler(
    State(state): State<Arc<GatewayState>>,
) -> axum::response::sse::Sse<
    impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::StreamExt;
    use tokio_stream::wrappers::BroadcastStream;

    // Subscribe to gateway events
    let rx = state.event_tx.subscribe();

    let stream = BroadcastStream::new(rx).filter_map(|result| async move {
        match result {
            Ok(evt) => {
                // Serialize GatewayEvent directly - let terminals handle display logic
                // Add event_type field to help terminals identify event type
                let mut json_value = serde_json::to_value(&evt).unwrap_or_default();
                if let serde_json::Value::Object(ref mut map) = json_value {
                    // Add event_type field based on the variant
                    let event_type = match &evt {
                        GatewayEvent::AgentResponse { .. } => "agent_response",
                        GatewayEvent::Thinking { .. } => "thinking",
                        GatewayEvent::ContentDelta { .. } => "content_delta",
                        GatewayEvent::ToolCalling { .. } => "tool_calling",
                        GatewayEvent::ToolResult { .. } => "tool_result",
                        GatewayEvent::AgentStatus { .. } => "agent_status",
                        GatewayEvent::ProcessingError { .. } => "processing_error",
                        GatewayEvent::Completed { .. } => "completed",
                        GatewayEvent::MessageReceived { .. } => "message_received",
                        GatewayEvent::ChannelStatus { .. } => "channel_status",
                        GatewayEvent::ApprovalRequired { .. } => "approval_required",
                        GatewayEvent::RepairAction { .. } => "repair_action",
                        GatewayEvent::DevicePairRequested { .. } => "device_pair_requested",
                        GatewayEvent::SessionCreated { .. } => "session_created",
                        GatewayEvent::SessionRenamed { .. } => "session_renamed",
                        GatewayEvent::CronAnnounce { .. } => "cron_announce",
                    };
                    map.insert("event_type".to_string(), serde_json::json!(event_type));
                }
                let data = json_value.to_string();
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[allow(dead_code)]
/// List all registered event hooks
pub async fn list_hooks_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let hooks = state.hook_registry.list_hooks().await;
    Json(hooks)
}

#[allow(dead_code)]
/// Unregister a hook by name
pub async fn unregister_hook_handler(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let removed = state.hook_registry.unregister(&name).await;
    if removed {
        (StatusCode::OK, Json(serde_json::json!({"status": "removed", "name": name})))
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Hook not found", "name": name})),
        )
            .into_response()
    }
}

