//! HTTP Server for Manta
//!
//! Provides REST API endpoints for interacting with the Manta engine.

use crate::core::Engine;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
}

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
        }
    }
}

/// Start the HTTP server
pub async fn start_server(config: ServerConfig, engine: Arc<Engine>) -> crate::Result<()> {
    let state = AppState { engine };

    let app = create_router(state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| crate::error::MantaError::Validation(format!("Invalid address: {}", e)))?;

    info!("Starting HTTP server on {}", addr);

    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| crate::error::MantaError::Internal(format!("Failed to bind: {}", e)))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| crate::error::MantaError::Internal(format!("Server error: {}", e)))?;

    Ok(())
}

/// Create the router with all routes
fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/health", get(health_check))
        .route("/entities", post(create_entity))
        .route("/entities/:id", get(get_entity))
        .route("/entities/:id", put(update_entity))
        .with_state(state)
}

/// Root endpoint
async fn root() -> impl IntoResponse {
    Json(serde_json::json!({
        "name": "Manta",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "running"
    }))
}

/// Health check endpoint
async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    // Simple health check - just return ok
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

/// Request to create an entity
#[derive(Debug, Deserialize)]
pub struct CreateEntityRequest {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Entity response
#[derive(Debug, Serialize)]
pub struct EntityResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

/// Create a new entity
async fn create_entity(
    State(state): State<AppState>,
    Json(request): Json<CreateEntityRequest>,
) -> impl IntoResponse {
    let req = crate::core::models::CreateEntityRequest {
        name: request.name,
        description: request.description,
        tags: request.tags,
    };

    match state.engine.create_entity(req) {
        Ok(entity) => {
            let response = EntityResponse {
                id: entity.id.to_string(),
                name: entity.name,
                description: entity.description,
                status: entity.status.to_string(),
                tags: entity.metadata.tags,
                created_at: entity.metadata.created_at.to_rfc3339(),
                updated_at: entity.metadata.updated_at.to_rfc3339(),
            };
            (StatusCode::CREATED, Json(serde_json::json!(response)))
        }
        Err(e) => {
            error!("Failed to create entity: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        }
    }
}

/// Get an entity by ID
async fn get_entity(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match crate::core::models::Id::parse(&id) {
        Ok(id) => match state.engine.get_entity(id) {
            Ok(entity) => {
                let response = EntityResponse {
                    id: entity.id.to_string(),
                    name: entity.name,
                    description: entity.description,
                    status: entity.status.to_string(),
                    tags: entity.metadata.tags,
                    created_at: entity.metadata.created_at.to_rfc3339(),
                    updated_at: entity.metadata.updated_at.to_rfc3339(),
                };
                (StatusCode::OK, Json(serde_json::json!(response)))
            }
            Err(e) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": e.to_string()})),
            ),
        },
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid ID: {}", e)})),
        ),
    }
}

/// Request to update an entity
#[derive(Debug, Deserialize)]
pub struct UpdateEntityRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Update an entity
async fn update_entity(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateEntityRequest>,
) -> impl IntoResponse {
    match crate::core::models::Id::parse(&id) {
        Ok(id) => {
            let status = request.status.and_then(|s| match s.as_str() {
                "active" => Some(crate::core::models::Status::Active),
                "paused" => Some(crate::core::models::Status::Paused),
                "completed" => Some(crate::core::models::Status::Completed),
                "failed" => Some(crate::core::models::Status::Failed),
                _ => None,
            });

            let req = crate::core::models::UpdateEntityRequest {
                name: request.name,
                description: request.description,
                status,
                tags: request.tags,
            };

            match state.engine.update_entity(id, req) {
                Ok(entity) => {
                    let response = EntityResponse {
                        id: entity.id.to_string(),
                        name: entity.name,
                        description: entity.description,
                        status: entity.status.to_string(),
                        tags: entity.metadata.tags,
                        created_at: entity.metadata.created_at.to_rfc3339(),
                        updated_at: entity.metadata.updated_at.to_rfc3339(),
                    };
                    (StatusCode::OK, Json(serde_json::json!(response)))
                }
                Err(e) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": e.to_string()})),
                ),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid ID: {}", e)})),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 3000);
    }
}
