//! WebSocket RPC handlers for admin-style operations. Each domain lives in
//! its own submodule (`approvals`, `providers`, `plugins`, `device`, `cloud`,
//! ...); this facade re-exports them for the `ws/core.rs` dispatcher and holds
//! the shared cloud-status helpers. The remaining REST surface is only where
//! HTTP is required: OpenAI compatibility, OAuth login, webhooks,
//! artifact/file downloads, and health/metrics probes.

use super::{WsRequest, WsResponse};
use crate::gateway::GatewayState;

mod agents;
mod approvals;
mod audit;
mod auth_profiles;
mod cloud;
mod cron;
mod device;
mod marketplace;
mod mcp;
mod memory;
mod mention;
mod onboarding;
mod plugins;
mod providers;
mod skills;
mod status;
mod system;
mod traces;
mod update;

pub(crate) use agents::*;
pub(crate) use approvals::*;
pub(crate) use audit::*;
pub(crate) use auth_profiles::*;
pub(crate) use cloud::*;
pub(crate) use cron::*;
pub(crate) use device::*;
pub(crate) use marketplace::*;
pub(crate) use mcp::*;
pub(crate) use memory::*;
pub(crate) use mention::*;
pub(crate) use onboarding::*;
pub(crate) use plugins::*;
pub(crate) use providers::*;
pub(crate) use skills::*;
pub(crate) use status::*;
pub(crate) use system::*;
pub(crate) use traces::*;
pub(crate) use update::*;

/// Error helper for the common "cloud not enabled / not signed in" case.
fn cloud_unavailable(req: &WsRequest) -> WsResponse {
    WsResponse::err(&req.id, "UNAUTHORIZED", "cloud not enabled or not signed in")
}

/// Build the cloud status block (mirrors `status_handler`'s cloud JSON).
async fn cloud_status_json(state: &GatewayState) -> serde_json::Value {
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return serde_json::json!({ "enabled": false, "logged_in": false, "user": null });
        }
        let logged_in = crate::cloud::session::logged_in().await;
        let mut user = None;
        if logged_in {
            if let Some(token) = crate::cloud::session::get_token().await {
                if let Ok(Some(u)) = crate::cloud::client::CloudClient::new(&cfg, token)
                    .me()
                    .await
                {
                    user = Some(u);
                }
            }
        }
        serde_json::json!({ "enabled": true, "logged_in": logged_in, "user": user })
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = state;
        serde_json::json!(null)
    }
}

#[cfg(test)]
mod tests;
