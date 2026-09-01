//! Cloud session HTTP endpoint (feature `cloud`). The OAuth login must be a
//! browser redirect, so it stays HTTP; the web SPA reads `#token=` on the
//! callback and persists it over WS (`cloud.token`). All other session actions
//! (token/logout/subscription/usage) are WS-only via `ws/admin_ws.rs`.

#![cfg(feature = "cloud")]

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::cloud::session;
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
