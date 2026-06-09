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

#[allow(dead_code)]
/// `GET /api/v1/pairing/pending` — list pending pairing requests.
pub async fn list_pairing_pending_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<PairingChannelQuery>,
) -> impl IntoResponse {
    let pending = if let Some(channel) = query.channel {
        state.pairing_store.list_pending(&channel).await
    } else {
        // List all pending across all channels
        let mut all = Vec::new();
        let channels = {
            let cfg = state.config.read().await;
            cfg.channels.keys().cloned().collect::<Vec<_>>()
        };
        for channel in channels {
            let mut channel_pending = state.pairing_store.list_pending(&channel).await;
            all.append(&mut channel_pending);
        }
        all
    };
    Json(pending)
}

#[allow(dead_code)]
/// `GET /api/v1/pairing/authorized` — list authorized users.
pub async fn list_pairing_authorized_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<PairingChannelQuery>,
) -> impl IntoResponse {
    let authorized = if let Some(channel) = query.channel {
        state
            .pairing_store
            .list_authorized_for_channel(&channel)
            .await
    } else {
        state.pairing_store.list_authorized().await
    };
    Json(authorized)
}

#[allow(dead_code)]
/// `POST /api/v1/pairing/approve` — approve a pending request by code.
pub async fn approve_pairing_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ApprovePairingRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    match state
        .pairing_store
        .approve(&req.channel, &req.code, Some("admin"))
        .await
    {
        Some(user) => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingApprove,
                    "admin",
                    &req.channel,
                    true,
                    format!("Approved user {} on channel {}", user.user_id, user.channel),
                    Some(serde_json::json!({"user_id": user.user_id, "code": req.code})),
                )
                .await;
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "approved",
                    "user_id": user.user_id,
                    "channel": user.channel,
                })),
            )
                .into_response()
        }
        None => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingApprove,
                    "admin",
                    &req.channel,
                    false,
                    format!("Approve failed: code {} not found or expired", req.code),
                    None,
                )
                .await;
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Pairing request not found or expired",
                    "code": req.code,
                    "channel": req.channel,
                })),
            )
                .into_response()
        }
    }
}

#[allow(dead_code)]
/// `POST /api/v1/pairing/reject` — reject a pending request by code.
pub async fn reject_pairing_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<RejectPairingRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    match state.pairing_store.reject(&req.channel, &req.code).await {
        Some(r) => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingReject,
                    "admin",
                    &req.channel,
                    true,
                    format!("Rejected user {} on channel {}", r.user_id, r.channel),
                    Some(serde_json::json!({"user_id": r.user_id, "code": req.code})),
                )
                .await;
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "rejected",
                    "user_id": r.user_id,
                    "channel": r.channel,
                })),
            )
                .into_response()
        }
        None => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingReject,
                    "admin",
                    &req.channel,
                    false,
                    format!("Reject failed: code {} not found", req.code),
                    None,
                )
                .await;
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Pairing request not found",
                    "code": req.code,
                    "channel": req.channel,
                })),
            )
                .into_response()
        }
    }
}

#[allow(dead_code)]
/// `POST /api/v1/pairing/revoke` — revoke an authorized user.
pub async fn revoke_pairing_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<RevokePairingRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    let removed = state.pairing_store.revoke(&req.channel, &req.user_id).await;
    if removed {
        state
            .audit_log
            .log(
                AuditEventType::PairingRevoke,
                "admin",
                &req.channel,
                true,
                format!("Revoked user {} on channel {}", req.user_id, req.channel),
                Some(serde_json::json!({"user_id": req.user_id})),
            )
            .await;
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "revoked",
                "user_id": req.user_id,
                "channel": req.channel,
            })),
        )
            .into_response()
    } else {
        state
            .audit_log
            .log(
                AuditEventType::PairingRevoke,
                "admin",
                &req.channel,
                false,
                format!("Revoke failed: user {} not found in authorized list", req.user_id),
                None,
            )
            .await;
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "User not found in authorized list",
                "user_id": req.user_id,
                "channel": req.channel,
            })),
        )
            .into_response()
    }
}

#[allow(dead_code)]
/// `POST /api/v1/pairing/allowlist` — add a user directly to the allowlist.
pub async fn add_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddAllowlistRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    let user = state
        .pairing_store
        .add_to_allowlist(&req.channel, &req.user_id, req.username.as_deref(), Some("admin"))
        .await;
    state
        .audit_log
        .log(
            AuditEventType::PairingApprove,
            "admin",
            &req.channel,
            true,
            format!("Added user {} to allowlist on channel {}", req.user_id, req.channel),
            Some(serde_json::json!({"user_id": req.user_id, "username": req.username})),
        )
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "added",
            "user_id": user.user_id,
            "channel": user.channel,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `GET /api/v1/gate/levels` — list all configured user levels.
pub async fn list_gate_levels_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let levels = state.command_gate.user_levels();
    let json_levels: std::collections::HashMap<String, String> = levels
        .into_iter()
        .map(|(k, v)| (k, v.to_string()))
        .collect();
    Json(serde_json::json!({
        "levels": json_levels,
        "default": "chat",
    }))
}

#[allow(dead_code)]
/// `POST /api/v1/gate/levels` — set a user's permission level.
pub async fn set_gate_level_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SetGateLevelRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    let level = match req.level.as_str() {
        "chat" => crate::tools::command_gate::UserLevel::Chat,
        "user" => crate::tools::command_gate::UserLevel::User,
        "admin" => crate::tools::command_gate::UserLevel::Admin,
        _ => {
            state
                .audit_log
                .log(
                    AuditEventType::CommandGate,
                    "admin",
                    "gateway",
                    false,
                    format!("Invalid level '{}' for user {}", req.level, req.user_id),
                    None,
                )
                .await;
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Invalid level '{}'. Expected: chat, user, admin", req.level)
                })),
            )
                .into_response();
        }
    };

    state.command_gate.set_user_level(&req.user_id, level);
    state
        .audit_log
        .log(
            AuditEventType::CommandGate,
            "admin",
            "gateway",
            true,
            format!("Set user {} level to {}", req.user_id, req.level),
            Some(serde_json::json!({"user_id": req.user_id, "level": req.level})),
        )
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "updated",
            "user_id": req.user_id,
            "level": req.level,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `DELETE /api/v1/gate/levels/:user_id` — clear a user's custom level.
pub async fn clear_gate_level_handler(
    State(state): State<Arc<GatewayState>>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    state.command_gate.clear_user_level(&user_id);
    state
        .audit_log
        .log(
            AuditEventType::CommandGate,
            "admin",
            "gateway",
            true,
            format!("Cleared custom level for user {}", user_id),
            Some(serde_json::json!({"user_id": user_id})),
        )
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "cleared",
            "user_id": user_id,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `GET /api/v1/audit/log` — retrieve recent audit log entries.
pub async fn list_audit_log_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<AuditLogQuery>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;

    let entries = if let Some(ref etype) = query.event_type {
        let event_type = match etype.as_str() {
            "access_check" => AuditEventType::AccessCheck,
            "pairing_request" => AuditEventType::PairingRequest,
            "pairing_approve" => AuditEventType::PairingApprove,
            "pairing_reject" => AuditEventType::PairingReject,
            "pairing_revoke" => AuditEventType::PairingRevoke,
            "command_gate" => AuditEventType::CommandGate,
            "config_change" => AuditEventType::ConfigChange,
            "tool_invocation" => AuditEventType::ToolInvocation,
            "tool_deny" => AuditEventType::ToolDeny,
            "security" => AuditEventType::Security,
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("Unknown event_type: {}", etype)
                    })),
                )
                    .into_response();
            }
        };
        state.audit_log.filter(event_type).await
    } else {
        state.audit_log.recent(query.limit.unwrap_or(100)).await
    };

    Json(serde_json::json!({
        "entries": entries,
        "count": entries.len(),
    }))
    .into_response()
}

