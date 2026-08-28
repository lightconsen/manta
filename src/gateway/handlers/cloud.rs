//! Cloud session HTTP endpoints (feature `cloud`). Login redirects to the
//! cloud OAuth; the web SPA reads `#token=` on the callback and POSTs it here
//! to persist the session. Status/logout manage the stored token.

#![cfg(feature = "cloud")]

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::cloud::{client::CloudClient, device, session};
use crate::gateway::state::GatewayState;

#[derive(Debug, Deserialize)]
pub struct ProviderQuery {
    #[serde(default)]
    pub provider: Option<String>,
}

/// GET /api/v1/cloud/login?provider=github|google|wechat → 302 to cloud OAuth.
pub async fn login_handler(
    State(state): State<Arc<GatewayState>>,
    Query(q): Query<ProviderQuery>,
) -> Response {
    let cfg = { state.config.read().await.cloud.clone() };
    if !cfg.enabled {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "cloud is not enabled (config cloud.enabled)".to_string(),
        )
            .into_response();
    }
    let provider = q.provider.as_deref().unwrap_or("github");
    let url = session::login_url(&cfg, provider);
    Redirect::temporary(&url).into_response()
}

#[derive(Debug, Deserialize)]
pub struct TokenBody {
    pub token: String,
}

/// POST /api/v1/cloud/token — persist a session token and return the user.
pub async fn token_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<TokenBody>,
) -> Response {
    let cfg = { state.config.read().await.cloud.clone() };
    if !cfg.enabled {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "cloud is not enabled (config cloud.enabled)".to_string(),
        )
            .into_response();
    }
    if let Err(e) = session::set_token(&body.token).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    match CloudClient::new(&cfg, body.token).me().await {
        Ok(Some(user)) => {
            // Best-effort device registration (P2-9): a stable device identity
            // for future cloud sync. Never fails the login on bind errors.
            if let Err(e) = device::bind(&cfg).await {
                warn!("Cloud device bind failed: {e}");
            }
            Json(json!({ "ok": true, "user": user })).into_response()
        }
        Ok(None) => (StatusCode::UNAUTHORIZED, "invalid cloud token".to_string()).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// GET /api/v1/cloud/status — enabled + logged-in + user.
pub async fn status_handler(State(state): State<Arc<GatewayState>>) -> Response {
    let cfg = { state.config.read().await.cloud.clone() };
    if !cfg.enabled {
        return Json(json!({ "enabled": false, "logged_in": false, "user": null })).into_response();
    }
    let logged_in = session::logged_in().await;
    let mut user = None;
    if logged_in {
        if let Some(token) = session::get_token().await {
            if let Ok(Some(u)) = CloudClient::new(&cfg, token).me().await {
                user = Some(u);
            }
        }
    }
    Json(json!({ "enabled": true, "logged_in": logged_in, "user": user })).into_response()
}

/// POST /api/v1/cloud/logout — forget the stored session token.
pub async fn logout_handler(State(_state): State<Arc<GatewayState>>) -> Response {
    let _ = session::clear_token().await;
    StatusCode::NO_CONTENT.into_response()
}

// ─────────────────────────────────────────────
// Subscription / usage (P2-10)
// ─────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct UsageQuery {
    #[serde(default = "default_usage_days")]
    pub days: i64,
}

fn default_usage_days() -> i64 {
    30
}

async fn cloud_client(state: &GatewayState) -> Option<CloudClient> {
    let cfg = { state.config.read().await.cloud.clone() };
    if !cfg.enabled {
        return None;
    }
    let token = session::get_token().await?;
    Some(CloudClient::new(&cfg, token))
}

/// GET /api/v1/cloud/subscription — plan, credit balance, overdraft state
/// (proxies `GET /api/v1/subscription` with the session token).
pub async fn subscription_handler(State(state): State<Arc<GatewayState>>) -> Response {
    let Some(client) = cloud_client(&state).await else {
        return (StatusCode::UNAUTHORIZED, "cloud not enabled or not signed in".to_string())
            .into_response();
    };
    match client.subscription().await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// GET /api/v1/cloud/usage?days=30 — credit usage for the period (proxies
/// `GET /api/v1/usage`).
pub async fn usage_handler(
    State(state): State<Arc<GatewayState>>,
    Query(q): Query<UsageQuery>,
) -> Response {
    let Some(client) = cloud_client(&state).await else {
        return (StatusCode::UNAUTHORIZED, "cloud not enabled or not signed in".to_string())
            .into_response();
    };
    match client.usage(q.days as u32).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}
