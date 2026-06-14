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
use std::sync::atomic::Ordering;
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
use crate::gateway::*;
use crate::gateway::{DreamHealthReport, GatewayState, HealthReport, SubsystemHealth};
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

// HTTP Handlers

/// Comprehensive health check with all subsystem statuses.
/// Returns 200 if healthy, 503 if any critical subsystem is down.
pub async fn health_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let report = build_health_report(&state).await;
    let status_code = if report.overall_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(report))
}

/// Readiness probe — returns 200 when the gateway is ready to serve traffic.
/// Checks: agents, providers, channels.
pub async fn ready_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let agents = state.agents.read().await;
    let agent_ready = agents.get("default").is_some();
    let agent_count = agents.len();
    drop(agents);

    let router_health = state.model_router.get_health_status().await;
    let healthy_providers = router_health
        .values()
        .filter(|h| matches!(h.state, crate::model_router::CircuitState::Closed))
        .count();

    let channels = state.channels.read().await;
    let channel_count = channels.len();
    drop(channels);

    let ready = agent_ready && healthy_providers > 0 && channel_count > 0;

    let status_code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status_code,
        Json(serde_json::json!({
            "ready": ready,
            "agents": { "ready": agent_ready, "count": agent_count },
            "providers": { "healthy": healthy_providers, "total": router_health.len() },
            "channels": { "count": channel_count },
        })),
    )
}

/// Liveness probe — returns 200 if the gateway process is alive.
/// Lightweight check that just confirms the process is running.
pub async fn live_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "alive": true,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })),
    )
}

/// Metrics endpoint — returns Prometheus text format metrics.
pub async fn metrics_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let metrics = build_prometheus_metrics(&state).await;
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; version=0.0.4")], metrics)
}

/// Build Prometheus text format metrics from GatewayState.
pub async fn build_prometheus_metrics(state: &Arc<GatewayState>) -> String {
    let mut lines = Vec::new();

    // Helper to emit a gauge
    let mut gauge = |name: &str, value: f64, help: &str| {
        lines.push(format!("# HELP {name} {help}"));
        lines.push(format!("# TYPE {name} gauge"));
        lines.push(format!("{name} {value}"));
    };

    // Uptime
    let uptime_secs = state.start_time.elapsed().as_secs() as f64;
    gauge(
        "syscity_uptime_seconds",
        uptime_secs,
        "Number of seconds since the gateway started",
    );

    // Agents
    let agents = state.agents.read().await;
    let agent_count = agents.len() as f64;
    drop(agents);
    gauge("syscity_agents_active", agent_count, "Number of active agents");

    // Channels
    let channels = state.channels.read().await;
    let channel_count = channels.len() as f64;
    drop(channels);
    gauge("syscity_channels_configured", channel_count, "Number of configured channels");

    // Providers
    let router_health = state.model_router.get_health_status().await;
    let healthy_providers = router_health
        .values()
        .filter(|h| matches!(h.state, crate::model_router::CircuitState::Closed))
        .count() as f64;
    let total_providers = router_health.len() as f64;
    gauge(
        "syscity_providers_healthy",
        healthy_providers,
        "Number of healthy LLM providers",
    );
    gauge(
        "syscity_providers_total",
        total_providers,
        "Total number of configured LLM providers",
    );

    // Memory subsystems
    let vector_memory_ready = if state.vector_memory.read().await.is_some() {
        1.0
    } else {
        0.0
    };
    let memory_manager_ready = if state.memory_manager.read().await.is_some() {
        1.0
    } else {
        0.0
    };
    gauge(
        "syscity_vector_memory_ready",
        vector_memory_ready,
        "Whether vector memory is initialized (1 = ready, 0 = not)",
    );
    gauge(
        "syscity_memory_manager_ready",
        memory_manager_ready,
        "Whether memory manager is initialized (1 = ready, 0 = not)",
    );

    // Cron
    let cron_ready = if state.cron_scheduler.read().await.is_some() {
        1.0
    } else {
        0.0
    };
    gauge(
        "syscity_cron_ready",
        cron_ready,
        "Whether cron scheduler is running (1 = ready, 0 = not)",
    );

    // Plugins
    let plugin_count = state.plugin_manager.list_plugins().await.len() as f64;
    gauge("syscity_plugins_loaded", plugin_count, "Number of loaded plugins");

    // MCP
    let mcp_count = state.mcp_manager.list_servers().await.len() as f64;
    gauge("syscity_mcp_servers_connected", mcp_count, "Number of connected MCP servers");

    // Storage
    let storage_healthy = match state.storage.read().await.health_check().await {
        Ok(_) => 1.0,
        Err(_) => 0.0,
    };
    gauge(
        "syscity_storage_healthy",
        storage_healthy,
        "Whether storage backend is healthy (1 = healthy, 0 = not)",
    );

    // Cost guard
    let daily_spend = state.cost_guard.daily_spend_cents() as f64;
    let hourly_actions = state.cost_guard.hourly_action_count() as f64;
    let budget_exceeded = if state.cost_guard.is_exceeded() {
        1.0
    } else {
        0.0
    };
    gauge("syscity_cost_daily_spend_cents", daily_spend, "Daily LLM spend in cents");
    gauge(
        "syscity_cost_hourly_actions",
        hourly_actions,
        "Number of LLM actions in the current hour",
    );
    gauge(
        "syscity_cost_budget_exceeded",
        budget_exceeded,
        "Whether cost budget is exceeded (1 = yes, 0 = no)",
    );

    // Audit log
    let audit_entries = state.audit_log.persisted_count().await as f64;
    gauge("syscity_audit_log_entries", audit_entries, "Total number of audit log entries");

    // Core engine metrics
    if let Some(ref em) = state.engine_metrics {
        gauge(
            "syscity_engine_entities_created",
            em.entities_created
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total entities created by the core engine",
        );
        gauge(
            "syscity_engine_entities_updated",
            em.entities_updated
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total entities updated by the core engine",
        );
        gauge(
            "syscity_engine_entities_deleted",
            em.entities_deleted
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total entities deleted by the core engine",
        );
        gauge(
            "syscity_engine_errors_total",
            em.errors.load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total errors encountered by the core engine",
        );
        gauge(
            "syscity_engine_archive_runs_total",
            em.archive_runs.load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total archive sweep runs executed by the core engine",
        );
        gauge(
            "syscity_engine_entities_archived_total",
            em.entities_archived
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total entities archived by the core engine",
        );
    }

    // Dream metrics
    if let Some(ref dm) = *state.dream_scheduler.read().await {
        let metrics = dm.metrics();
        gauge(
            "syscity_dreams_total",
            metrics
                .dreams_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total number of dream cycles run",
        );
        gauge(
            "syscity_dreams_failed_total",
            metrics
                .dreams_failed
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total number of dream cycles that failed",
        );
        gauge(
            "syscity_dream_memories_processed_total",
            metrics
                .memories_processed_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total memories processed by dream cycles",
        );
        gauge(
            "syscity_dream_memories_created_total",
            metrics
                .memories_created_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total memories created by dream cycles",
        );
        gauge(
            "syscity_dream_memories_removed_total",
            metrics
                .memories_removed_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total memories removed by dream cycles",
        );
        gauge(
            "syscity_dream_memories_promoted_total",
            metrics
                .memories_promoted_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total memories promoted by dream cycles",
        );
        gauge(
            "syscity_dream_memories_demoted_total",
            metrics
                .memories_demoted_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total memories demoted by dream cycles",
        );
        gauge(
            "syscity_dream_duration_ms_total",
            metrics
                .dream_duration_ms_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total dream cycle duration in milliseconds",
        );
        gauge(
            "syscity_dream_llm_tokens_input_total",
            metrics
                .llm_tokens_input_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total estimated LLM input tokens consumed during dreams",
        );
        gauge(
            "syscity_dream_llm_tokens_output_total",
            metrics
                .llm_tokens_output_total
                .load(std::sync::atomic::Ordering::Relaxed) as f64,
            "Total estimated LLM output tokens produced during dreams",
        );
    }

    // Per-plugin metrics
    let plugin_snapshots = state.plugin_manager.metrics().all_snapshots().await;
    for (plugin_id, snap) in &plugin_snapshots {
        let plugin_label = &plugin_id.replace('"', "");
        lines.push(format!("# HELP syscity_plugin_tool_calls_total Total tool calls per plugin"));
        lines.push(format!("# TYPE syscity_plugin_tool_calls_total counter"));
        lines.push(format!(
            "syscity_plugin_tool_calls_total{{plugin=\"{}\"}} {}",
            plugin_label, snap.tool_calls
        ));

        lines.push(format!("# HELP syscity_plugin_tool_errors_total Total tool errors per plugin"));
        lines.push(format!("# TYPE syscity_plugin_tool_errors_total counter"));
        lines.push(format!(
            "syscity_plugin_tool_errors_total{{plugin=\"{}\"}} {}",
            plugin_label, snap.tool_errors
        ));

        lines.push(format!(
            "# HELP syscity_plugin_http_requests_total Total HTTP requests per plugin"
        ));
        lines.push(format!("# TYPE syscity_plugin_http_requests_total counter"));
        lines.push(format!(
            "syscity_plugin_http_requests_total{{plugin=\"{}\"}} {}",
            plugin_label, snap.http_requests
        ));

        lines.push(format!("# HELP syscity_plugin_http_errors_total Total HTTP errors per plugin"));
        lines.push(format!("# TYPE syscity_plugin_http_errors_total counter"));
        lines.push(format!(
            "syscity_plugin_http_errors_total{{plugin=\"{}\"}} {}",
            plugin_label, snap.http_errors
        ));

        lines.push(format!("# HELP syscity_plugin_memory_bytes Current memory usage per plugin"));
        lines.push(format!("# TYPE syscity_plugin_memory_bytes gauge"));
        lines.push(format!(
            "syscity_plugin_memory_bytes{{plugin=\"{}\"}} {}",
            plugin_label, snap.memory_usage_bytes
        ));
    }

    lines.push(String::new());
    lines.join("\n")
}

/// Build a comprehensive health report covering all subsystems.
pub async fn build_health_report(state: &Arc<GatewayState>) -> HealthReport {
    // Agents
    let agents = state.agents.read().await;
    let agent_ready = agents.get("default").is_some();
    let agent_count = agents.len();
    drop(agents);

    // Providers
    let router_health = state.model_router.get_health_status().await;
    let healthy_providers = router_health
        .values()
        .filter(|h| matches!(h.state, crate::model_router::CircuitState::Closed))
        .count();
    let total_providers = router_health.len();

    // Channels
    let channels = state.channels.read().await;
    let channel_count = channels.len();
    drop(channels);

    // Vector memory
    let vector_memory_ready = state.vector_memory.read().await.is_some();

    // Memory manager
    let memory_manager_ready = state.memory_manager.read().await.is_some();

    // Cron scheduler
    let cron_ready = state.cron_scheduler.read().await.is_some();

    // Plugins
    let plugin_count = state.plugin_manager.list_plugins().await.len();

    // MCP servers
    let mcp_count = state.mcp_manager.list_servers().await.len();

    // Storage
    let storage_healthy = state.storage.read().await.health_check().await.is_ok();

    // Cost guard
    let cost_exceeded = state.cost_guard.is_exceeded();
    let daily_spend = state.cost_guard.daily_spend_cents() as f64 / 100.0;

    // Dream metrics
    let dream_report = state
        .dream_scheduler
        .read()
        .await
        .as_ref()
        .map(|scheduler| {
            let metrics = scheduler.metrics();
            crate::gateway::DreamHealthReport {
                dreams_total: metrics.dreams_total.load(Ordering::Relaxed),
                dreams_failed: metrics.dreams_failed.load(Ordering::Relaxed),
                memories_processed_total: metrics.memories_processed_total.load(Ordering::Relaxed),
                memories_created_total: metrics.memories_created_total.load(Ordering::Relaxed),
                memories_removed_total: metrics.memories_removed_total.load(Ordering::Relaxed),
                memories_promoted_total: metrics.memories_promoted_total.load(Ordering::Relaxed),
                memories_demoted_total: metrics.memories_demoted_total.load(Ordering::Relaxed),
                dream_duration_ms_total: metrics.dream_duration_ms_total.load(Ordering::Relaxed),
                llm_tokens_input_total: metrics.llm_tokens_input_total.load(Ordering::Relaxed),
                llm_tokens_output_total: metrics.llm_tokens_output_total.load(Ordering::Relaxed),
            }
        });

    // Overall: agents + providers are critical; others are warnings
    let overall_healthy = agent_ready && healthy_providers > 0;

    HealthReport {
        status: if overall_healthy {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        },
        version: crate::VERSION.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        overall_healthy,
        subsystems: SubsystemHealth {
            agents: HealthStatus {
                healthy: agent_ready,
                message: format!("{} agents active", agent_count),
            },
            providers: HealthStatus {
                healthy: healthy_providers > 0,
                message: format!("{}/{} healthy", healthy_providers, total_providers),
            },
            channels: HealthStatus {
                healthy: channel_count > 0,
                message: format!("{} channels configured", channel_count),
            },
            vector_memory: HealthStatus {
                healthy: vector_memory_ready,
                message: if vector_memory_ready {
                    "ready".to_string()
                } else {
                    "not initialized".to_string()
                },
            },
            memory_manager: HealthStatus {
                healthy: memory_manager_ready,
                message: if memory_manager_ready {
                    "ready".to_string()
                } else {
                    "not initialized".to_string()
                },
            },
            cron: HealthStatus {
                healthy: cron_ready,
                message: if cron_ready {
                    "running".to_string()
                } else {
                    "not initialized".to_string()
                },
            },
            plugins: HealthStatus {
                healthy: true,
                message: format!("{} plugins loaded", plugin_count),
            },
            mcp: HealthStatus {
                healthy: mcp_count > 0,
                message: format!("{} MCP servers connected", mcp_count),
            },
            storage: HealthStatus {
                healthy: storage_healthy,
                message: if storage_healthy {
                    "healthy".to_string()
                } else {
                    "unavailable".to_string()
                },
            },
            cost_guard: HealthStatus {
                healthy: !cost_exceeded,
                message: format!("${:.4} today", daily_spend),
            },
        },
        dream: dream_report,
    }
}

#[allow(dead_code)]
pub async fn status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let agents = state.agents.read().await;
    let channels = state.channels.read().await;

    Json(serde_json::json!({
        "agents": {
            "total": agents.len(),
            "busy": agents.values().filter(|a| a.busy).count(),
        },
        "channels": channels.len(),
        "version": crate::VERSION,
    }))
}

#[allow(dead_code)]
pub async fn repair_status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;
    let last_cycle = state
        .repair_state
        .last_cycle_at
        .read()
        .await
        .map(|t| t.to_rfc3339());
    let loop_running = state.repair_state.loop_running.load(Ordering::Relaxed);
    let records: Vec<_> = state
        .repair_state
        .records
        .read()
        .await
        .values()
        .cloned()
        .collect();
    Json(serde_json::json!({
        "loop_running": loop_running,
        "last_cycle_at": last_cycle,
        "repairs": records,
    }))
}

#[allow(dead_code)]
/// GET /api/v1/cost/status
///
/// Returns current spend and action-rate counters from the live CostGuard.
/// Useful for monitoring budget burn in real-time.
pub async fn cost_status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;

    let daily_cents = state.cost_guard.daily_spend_cents();
    let hourly_actions = state.cost_guard.hourly_action_count();
    let budget_exceeded = state.cost_guard.budget_exceeded.load(Ordering::Relaxed);
    let daily_limit = state.cost_guard.daily_limit_cents;
    let hourly_limit = state.cost_guard.hourly_action_limit;

    Json(serde_json::json!({
        "daily_spend_cents": daily_cents,
        "daily_limit_cents": daily_limit,
        "hourly_actions": hourly_actions,
        "hourly_action_limit": hourly_limit,
        "budget_exceeded": budget_exceeded,
    }))
}
