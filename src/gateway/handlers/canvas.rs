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

pub(crate) async fn handle_canvas_websocket(
    socket: axum::extract::ws::WebSocket,
    canvas_id: crate::canvas::CanvasId,
    state: Arc<GatewayState>,
) {
    use axum::extract::ws::Message;

    info!("Canvas WebSocket connected: {}", canvas_id.0);

    // Get or create canvas session
    let (event_tx, _event_rx) = mpsc::channel::<CanvasEvent>(100);
    let event_tx_client = event_tx.clone();

    let canvas_session = match state.canvas_manager.get_session(&canvas_id).await {
        Some(session) => session,
        None => state.canvas_manager.create_session(event_tx).await,
    };

    // Subscribe to updates
    let mut update_rx = canvas_session.update_tx.subscribe();

    // Split socket for send/receive
    let (mut sender, mut receiver) = socket.split();

    // Task to receive updates and send to client
    let update_task = tokio::spawn(async move {
        while let Ok(update) = update_rx.recv().await {
            let msg = Message::Text(serde_json::to_string(&update).unwrap_or_default());
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Task to receive client events and forward them into the canvas session
    let event_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<CanvasEvent>(&text) {
                    if event_tx_client.send(event).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = update_task => {}
        _ = event_task => {}
    }

    info!("Canvas WebSocket disconnected: {}", canvas_id.0);
}
