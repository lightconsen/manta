//! First-launch identity onboarding HTTP handlers.
//!
//! `GET /onboarding` reports whether the identity wizard still needs to run;
//! `POST /onboarding` writes SOUL.md / IDENTITY.md / USER.md and marks setup
//! completed. These run before login (the wizard is the very first thing the
//! user sees after configuring their first LLM model), so they read/write the
//! workspace directly via [`crate::memory::onboarding`].

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;
use crate::memory::onboarding::{self, OnboardingPayload, OnboardingStatus};

/// GET /onboarding — `{ "status": "pending" | "done" }`.
pub async fn onboarding_status_handler(
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let dir = crate::dirs::workspace_data_dir();
    match onboarding::status(&dir).await {
        Ok(status) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": match status {
                    OnboardingStatus::Done => "done",
                    OnboardingStatus::Pending => "pending",
                }
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

/// POST /onboarding — `{ "ok": true }` on success.
pub async fn onboarding_apply_handler(
    State(_state): State<Arc<GatewayState>>,
    Json(payload): Json<OnboardingPayload>,
) -> impl IntoResponse {
    let dir = crate::dirs::workspace_data_dir();
    match onboarding::apply(&dir, &payload).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}
