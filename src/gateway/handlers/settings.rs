
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use std::sync::Arc;

use crate::gateway::GatewayState;
use crate::gateway::*;

#[allow(dead_code)]
/// `GET /api/settings` — list all runtime key/value settings.
pub async fn list_settings_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let settings = state.infra.runtime_settings.read().await.clone();
    Json(settings)
}

#[allow(dead_code)]
/// `POST /api/settings` — upsert a runtime setting.
pub async fn set_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SetSettingRequest>,
) -> impl IntoResponse {
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert(req.key.clone(), req.value.clone());
    Json(serde_json::json!({ "ok": true, "key": req.key }))
}

#[allow(dead_code)]
/// `GET /api/settings/:key` — read one setting by key.
pub async fn get_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let settings = state.infra.runtime_settings.read().await;
    match settings.get(&key) {
        Some(val) => Json(serde_json::json!({ "key": key, "value": val })).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Setting '{}' not found", key) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `DELETE /api/settings/:key` — remove one setting.
pub async fn delete_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let mut settings = state.infra.runtime_settings.write().await;
    if settings.remove(&key).is_some() {
        Json(serde_json::json!({ "ok": true, "key": key })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Setting '{}' not found", key) })),
        )
            .into_response()
    }
}
