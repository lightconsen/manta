use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::error;

use crate::gateway::GatewayState;
use crate::gateway::*;

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
        if agent_handle.busy.load(Ordering::Acquire) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": "Agent is busy processing another message",
                })),
            );
        }

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
/// The message is queued for processing and a 202 Accepted is returned
/// immediately. The actual response(s) will be streamed via SSE on `GET
/// /api/events`.
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
        match state
            .infra
            .model_router
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;
    use axum::extract::{Path, Query};
    use std::collections::HashMap;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    #[tokio::test]
    async fn chat_handler_no_default_agent_503() {
        let state = state().await;
        let body = Json(ChatRequestCompat {
            message: "hello".to_string(),
            conversation_id: None,
        });
        let (status, json) =
            body_json(chat_handler(State(state), body).await.into_response()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"], "No default agent available");
    }

    #[tokio::test]
    async fn web_terminal_chat_no_receiver_500() {
        // The test state's inbound_entry channel has no receiver, so enqueue
        // fails and the handler reports an internal error.
        let state = state().await;
        let body = Json(WebTerminalChatRequest {
            message: "hello".to_string(),
            conversation_id: None,
            user_id: None,
        });
        let (status, json) = body_json(
            web_terminal_chat_handler(State(state), body)
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("Failed to enqueue message"));
    }

    #[tokio::test]
    async fn send_message_no_provider_no_receiver_500() {
        let state = state().await;
        let body = Json(SendMessageRequest {
            message: "hello".to_string(),
            user_id: None,
            provider_override: None,
            model_alias: None,
            model_id: None,
        });
        let (status, json) = body_json(
            send_message_handler(State(state), Path("sess-1".into()), body)
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("Failed to enqueue message"));
    }

    #[tokio::test]
    async fn get_conversation_history_empty_ok() {
        let state = state().await;
        let (status, json) = body_json(
            get_conversation_history_handler(
                State(state),
                Path("conv-1".into()),
                Query(HashMap::new()),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["conversation_id"], "conv-1");
        assert!(json["messages"].is_array());
        assert!(json["messages"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_last_conversation_empty_ok() {
        let state = state().await;
        let mut params = HashMap::new();
        params.insert("user_id".to_string(), "web_user".to_string());
        let (status, json) = body_json(
            get_last_conversation_handler(State(state), Query(params))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["user_id"], "web_user");
    }

    #[tokio::test]
    async fn list_conversations_empty_ok() {
        let state = state().await;
        let mut params = HashMap::new();
        params.insert("user_id".to_string(), "web_user".to_string());
        let (status, json) = body_json(
            list_conversations_handler(State(state), Query(params))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["conversations"].is_array());
        assert!(json["conversations"].as_array().unwrap().is_empty());
    }
}
