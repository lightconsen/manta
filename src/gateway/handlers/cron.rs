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
use ::cron::Schedule;

// ── Cron job management ───────────────────────────────────────────────────────

#[allow(dead_code)]
/// `GET /api/v1/cron` — list all scheduled jobs.
pub async fn list_cron_jobs_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.get_opt().await {
        Some(scheduler) => {
            let jobs = scheduler.lock().await.list_jobs().await;
            Json(serde_json::json!({ "jobs": jobs, "count": jobs.len() })).into_response()
        }
        None => Json(serde_json::json!({ "jobs": [], "count": 0 })).into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron` — create a new cron job.
pub async fn add_cron_job_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddCronJobRequest>,
) -> impl IntoResponse {
    use crate::cron::cron::{CronJob, ExecutionTarget, Schedule as CronSchedule};
    use std::str::FromStr;

    let schedule = match cron::Schedule::from_str(&req.schedule) {
        Ok(_) => CronSchedule::Cron {
            expression: req.schedule.clone(),
            timezone: None,
            stagger_ms: None,
        },
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid cron expression: {}", e) })),
            )
                .into_response();
        }
    };

    let job_id = uuid::Uuid::new_v4().to_string();
    let job = CronJob::new(
        job_id.clone(),
        req.name.clone(),
        schedule,
        ExecutionTarget::shell(req.command),
    );

    match state.scheduler.cron_scheduler.get_opt().await {
        Some(scheduler) => match scheduler.lock().await.add_job(job).await {
            Ok(()) => Json(serde_json::json!({
                "success": true,
                "id": job_id,
                "name": req.name,
            }))
            .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to add job: {}", e) })),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `DELETE /api/v1/cron/:id` — remove a cron job.
pub async fn remove_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.get_opt().await {
        Some(scheduler) => match scheduler.lock().await.remove_job(&id).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id })).into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron/:id/enable` — enable a cron job.
pub async fn enable_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.get_opt().await {
        Some(scheduler) => match scheduler.lock().await.set_job_enabled(&id, true).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "enabled": true }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron/:id/disable` — disable a cron job.
pub async fn disable_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.get_opt().await {
        Some(scheduler) => match scheduler.lock().await.set_job_enabled(&id, false).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "enabled": false }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron/:id/run` — trigger a cron job immediately.
pub async fn trigger_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.get_opt().await {
        Some(scheduler) => match scheduler.lock().await.trigger_job(&id).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "triggered": true }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `GET /api/v1/cron/:id/logs` — return job state / last-run info.
pub async fn cron_job_logs_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.get_opt().await {
        Some(scheduler) => match scheduler.lock().await.get_job(&id).await {
            Some(job) => Json(serde_json::json!({
                "id": job.id,
                "name": job.name,
                "enabled": job.enabled,
                "run_count": job.state.run_count,
                "last_run_at": job.state.last_run_at,
                "next_run_at": job.state.next_run_at,
                "last_error": job.state.last_error,
                "consecutive_errors": job.state.consecutive_errors,
            }))
            .into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("Job '{}' not found", id) })),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}
