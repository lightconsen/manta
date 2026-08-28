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

use crate::cloud::{client::CloudClient, session};
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
        Ok(Some(user)) => Json(json!({ "ok": true, "user": user })).into_response(),
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
