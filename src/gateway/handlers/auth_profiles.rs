
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use std::sync::Arc;

use crate::gateway::GatewayState;

// Auth Profile Handlers

#[allow(dead_code)]
pub async fn get_auth_profile_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.get_auth_profile_status(&id).await {
        Some(status) => (StatusCode::OK, Json(serde_json::json!(status))).into_response(),
        None => {
            let error = serde_json::json!({
                "error": format!("No auth profile found for provider '{}'", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn rotate_auth_profile_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.rotate_auth_key(&id).await {
        Ok(_new_key) => {
            let response = serde_json::json!({
                "success": true,
                "provider": id,
                "message": format!("Auth key rotated for provider '{}'", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to rotate auth key: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn list_auth_profiles_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let profiles = state.infra.model_router.list_auth_profiles().await;
    Json(serde_json::json!({
        "profiles": profiles,
        "count": profiles.len(),
    }))
}
