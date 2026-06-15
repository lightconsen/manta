#![allow(unused_imports)]

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use futures::{SinkExt, StreamExt};
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
use crate::gateway::GatewayState;
use crate::gateway::*;
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

#[allow(dead_code)]
pub async fn chat_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<ChatRequestCompat>,
) -> impl IntoResponse {
    let conversation_id = body
        .conversation_id
        .unwrap_or_else(|| "default".to_string());

    // Use the default agent to process the message
    let agents = state.agents.agents.read().await;
    if let Some(agent_handle) = agents.get("default") {
        // Subscribe to events before sending the command to avoid race condition
        let mut event_rx = state.events.tx.subscribe();

        // Send ProcessMessage command to agent
        let cmd = AgentCommand::ProcessMessage {
            session_id: conversation_id.clone(),
            message: body.message.clone(),
            user_id: "web_user".to_string(),
            channel: "web".to_string(),
            model_override: None,
        };

        if let Err(e) = agent_handle.tx.send(cmd).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to send message to agent: {}", e),
                })),
            );
        }

        // Drop the agents lock so we don't hold it while waiting
        drop(agents);

        // Wait for response with timeout
        let timeout = tokio::time::Duration::from_secs(120);
        let start = tokio::time::Instant::now();

        loop {
            // Check for timeout
            if start.elapsed() > timeout {
                return (
                    StatusCode::REQUEST_TIMEOUT,
                    Json(serde_json::json!({
                        "error": "Request timeout",
                    })),
                );
            }

            // Wait for event with a smaller timeout to allow checking
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), event_rx.recv())
                .await
            {
                Ok(Ok(GatewayEvent::AgentResponse {
                    session_id,
                    agent_id: _,
                    content,
                    ..
                })) => {
                    if session_id == conversation_id {
                        let resp = serde_json::json!({
                            "response": content,
                            "conversation_id": conversation_id,
                        });
                        return (StatusCode::OK, Json(resp));
                    }
                    // Not our session, continue waiting
                }
                Ok(Ok(_)) => {
                    // Some other event, continue waiting
                    continue;
                }
                Ok(Err(_)) => {
                    // Event channel closed
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Event channel closed",
                        })),
                    );
                }
                Err(_) => {
                    // Timeout on recv, continue loop to check overall timeout
                    continue;
                }
            }
        }
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "No default agent available",
            })),
        )
    }
}

#[allow(dead_code)]
/// `POST /api/chat` — Send a message from the web terminal.
///
/// The message is queued for processing and a 202 Accepted is returned immediately.
/// The actual response(s) will be streamed via SSE on `GET /api/events`.
pub async fn web_terminal_chat_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<WebTerminalChatRequest>,
) -> impl IntoResponse {
    let message_id = uuid::Uuid::new_v4().to_string();
    let user_id = body.user_id.unwrap_or_else(|| "web_user".to_string());
    let conversation_id = body
        .conversation_id
        .unwrap_or_else(|| AgentRouter::derive_session_key("web", &user_id));

    // Access control check
    if let Err(reason) = state
        .check_incoming_access(
            "web",
            &user_id,
            &body.message,
            &crate::channels::MentionState::DirectMessage,
        )
        .await
    {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": reason })))
            .into_response();
    }

    // Route through unified inbound entry
    let incoming =
        crate::channels::IncomingMessage::new(user_id, conversation_id.clone(), body.message)
            .with_provenance(crate::channels::InputProvenance::ExternalUser {
                channel: "web".to_string(),
                is_direct: true,
            });
    if let Err(e) = state.pipelines.inbound_entry.send(incoming).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to enqueue message: {}", e) })),
        )
            .into_response();
    }

    let resp = WebTerminalChatResponse {
        message_id,
        conversation_id,
        status: "processing".to_string(),
    };
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}

#[allow(dead_code)]
pub async fn send_message_handler(
    State(state): State<Arc<GatewayState>>,
    Path(session_id): Path<String>,
    Json(body): Json<SendMessageRequest>,
) -> impl IntoResponse {
    // Check if provider override is specified
    let provider_override = body.provider_override.clone();

    // Queue message for processing with provider override
    let message_id = uuid::Uuid::new_v4().to_string();
    let user_id = body
        .user_id
        .clone()
        .unwrap_or_else(|| "api_user".to_string());

    // Access control check
    if let Err(reason) = state
        .check_incoming_access(
            "api",
            &user_id,
            &body.message,
            &crate::channels::MentionState::DirectMessage,
        )
        .await
    {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": reason })))
            .into_response();
    }

    // If provider override is specified, we route through that provider
    if let Some(provider_name) = provider_override {
        match state.infra.model_router
            .complete_with_provider(
                &provider_name,
                body.model_id,
                vec![crate::providers::Message::user(body.message.clone())],
                None,
            )
            .await
        {
            Ok(response) => {
                let resp = serde_json::json!({
                    "message_id": message_id,
                    "session_id": session_id,
                    "provider_override": provider_name,
                    "response": response.message.content,
                    "status": "completed",
                });
                return (StatusCode::OK, Json(resp)).into_response();
            }
            Err(e) => {
                let resp = serde_json::json!({
                    "message_id": message_id,
                    "session_id": session_id,
                    "error": format!("Provider override failed: {}", e),
                    "status": "failed",
                });
                return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
            }
        }
    }

    // Otherwise, route through unified inbound entry for normal agent processing
    let incoming = crate::channels::IncomingMessage::new(user_id, session_id.clone(), body.message)
        .with_provenance(crate::channels::InputProvenance::ExternalUser {
            channel: "api".to_string(),
            is_direct: true,
        });
    if let Err(e) = state.pipelines.inbound_entry.send(incoming).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to enqueue message: {}", e) })),
        )
            .into_response();
    }

    let resp = serde_json::json!({
        "message_id": message_id,
        "session_id": session_id,
        "queued": true,
        "status": "processing",
    });
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}

#[allow(dead_code)]
/// Get conversation history
pub async fn get_conversation_history_handler(
    State(state): State<Arc<GatewayState>>,
    Path(conversation_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let limit: usize = params
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(100);

    // Access storage directly to get chat history
    let storage = state.infra.storage.read().await;

    match storage
        .get_conversation_history(&conversation_id, limit)
        .await
    {
        Ok(messages) => {
            let messages_json: Vec<_> = messages
                .into_iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "conversation_id": m.conversation_id,
                        "user_id": m.user_id,
                        "role": m.role,
                        "content": m.content,
                        "created_at": m.created_at,
                    })
                })
                .collect();

            let resp = serde_json::json!({
                "conversation_id": conversation_id,
                "messages": messages_json,
            });
            (StatusCode::OK, Json(resp))
        }
        Err(e) => {
            error!("Failed to get conversation history: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to get conversation history: {}", e)
                })),
            )
        }
    }
}

#[allow(dead_code)]
/// Get last conversation for a user
pub async fn get_last_conversation_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let user_id = params
        .get("user_id")
        .cloned()
        .unwrap_or_else(|| "web_user".to_string());

    // Access storage directly to get last conversation
    let storage = state.infra.storage.read().await;

    match storage.get_last_conversation(&user_id).await {
        Ok(conversation_id) => {
            let resp = serde_json::json!({
                "conversation_id": conversation_id,
                "user_id": user_id,
            });
            (StatusCode::OK, Json(resp))
        }
        Err(e) => {
            error!("Failed to get last conversation: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to get last conversation: {}", e)
                })),
            )
        }
    }
}

#[allow(dead_code)]
/// List all conversations for a user
pub async fn list_conversations_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let user_id = params
        .get("user_id")
        .cloned()
        .unwrap_or_else(|| "web_user".to_string());

    let storage = state.infra.storage.read().await;

    match storage.get_user_conversations(&user_id, 100).await {
        Ok(conversation_ids) => {
            let conversations: Vec<serde_json::Value> = conversation_ids
                .into_iter()
                .map(|id| serde_json::json!({"id": id}))
                .collect();

            let resp = serde_json::json!({
                "conversations": conversations,
                "user_id": user_id,
            });
            (StatusCode::OK, Json(resp))
        }
        Err(e) => {
            error!("Failed to list conversations: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to list conversations: {}", e)
                })),
            )
        }
    }
}
