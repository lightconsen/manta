//! WS admin handlers: cloud.

use std::sync::Arc;

#[cfg(feature = "cloud")]
use serde::Deserialize;

#[cfg(feature = "cloud")]
use super::super::parse_params;
use super::super::{WsRequest, WsResponse};
use super::{cloud_status_json, cloud_unavailable};
use crate::gateway::GatewayState;

/// `cloud.status` — `{ enabled, logged_in, user }` (or null without the cloud
/// feature).
pub(crate) async fn handle_cloud_status(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    WsResponse::ok(&req.id, cloud_status_json(state).await)
}

/// `cloud.subscription` — plan + credit balance.
pub(crate) async fn handle_cloud_subscription(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return cloud_unavailable(req);
        }
        let Some(token) = crate::cloud::session::get_token().await else {
            return cloud_unavailable(req);
        };
        match crate::cloud::client::CloudClient::new(&cfg, token)
            .subscription()
            .await
        {
            Ok(v) => WsResponse::ok(&req.id, v),
            Err(e) => WsResponse::err(&req.id, "BAD_GATEWAY", e.to_string()),
        }
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = state;
        cloud_unavailable(req)
    }
}

/// `cloud.usage` — `{ days }` credit usage for the period.
pub(crate) async fn handle_cloud_usage(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[cfg(feature = "cloud")]
    #[derive(Deserialize)]
    struct UsageParams {
        #[serde(default = "default_usage_days")]
        days: u32,
    }
    #[cfg(feature = "cloud")]
    fn default_usage_days() -> u32 {
        30
    }
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return cloud_unavailable(req);
        }
        let Some(token) = crate::cloud::session::get_token().await else {
            return cloud_unavailable(req);
        };
        let days = parse_params::<UsageParams>(req)
            .map(|p| p.days)
            .unwrap_or(30);
        match crate::cloud::client::CloudClient::new(&cfg, token)
            .usage(days)
            .await
        {
            Ok(v) => WsResponse::ok(&req.id, v),
            Err(e) => WsResponse::err(&req.id, "BAD_GATEWAY", e.to_string()),
        }
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = (state, req);
        cloud_unavailable(req)
    }
}

/// `cloud.token` — `{ token }` persist a cloud session token (OAuth result).
pub(crate) async fn handle_cloud_token(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[cfg(feature = "cloud")]
    #[derive(Deserialize)]
    struct TokenParams {
        token: String,
    }
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return cloud_unavailable(req);
        }
        let params: TokenParams = match parse_params(req) {
            Ok(p) => p,
            Err(res) => return res,
        };
        if let Err(e) = crate::cloud::session::set_token(&params.token).await {
            return WsResponse::err(&req.id, "INTERNAL", e.to_string());
        }
        match crate::cloud::client::CloudClient::new(&cfg, params.token.clone())
            .me()
            .await
        {
            Ok(Some(v)) => {
                // Same unwrap as `cloud_status_json`: /auth/me wraps the
                // identity ({ "user": { ... } }) — return it flat.
                let user = v.get("user").cloned().or(Some(v));
                // Best-effort device registration (P2-9): a stable device
                // identity for future cloud sync. Never fails the login on
                // bind errors (mirrors the removed REST token handler).
                if let Err(e) = crate::cloud::device::bind(&cfg).await {
                    tracing::warn!("Cloud device bind failed: {e}");
                }
                WsResponse::ok(&req.id, serde_json::json!({ "ok": true, "user": user }))
            }
            Ok(None) => WsResponse::err(&req.id, "UNAUTHORIZED", "invalid cloud token"),
            Err(e) => WsResponse::err(&req.id, "BAD_GATEWAY", e.to_string()),
        }
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = (state, req);
        cloud_unavailable(req)
    }
}

/// `cloud.logout` — forget the stored session token.
pub(crate) async fn handle_cloud_logout(req: &WsRequest, _state: &Arc<GatewayState>) -> WsResponse {
    #[cfg(feature = "cloud")]
    {
        let _ = crate::cloud::session::clear_token().await;
    }
    WsResponse::ok(&req.id, serde_json::json!({ "ok": true }))
}
