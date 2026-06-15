//! Browser bridge server — HTTP API for browser operations
//!
//! Provides REST endpoints for remote browser control via Axum.
//! Requires `browser` feature.

use super::pool::BrowserPool;
use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

/// Shared state for the bridge server
#[derive(Debug, Clone)]
pub struct BridgeState {
    pool: Arc<BrowserPool>,
    token: String,
}

/// Browser bridge HTTP server
#[derive(Debug, Clone)]
pub struct BrowserBridge {
    state: BridgeState,
    port: u16,
}

/// Navigate request
#[derive(Debug, Deserialize)]
pub struct NavigateRequest {
    pub profile: String,
    pub url: String,
}

/// Navigate response
#[derive(Debug, Serialize)]
pub struct NavigateResponse {
    pub success: bool,
    pub target_id: String,
    pub url: String,
    pub title: String,
}

/// Snapshot request
#[derive(Debug, Deserialize)]
pub struct SnapshotRequest {
    pub profile: String,
    pub target_id: String,
    pub max_chars: Option<usize>,
}

/// Snapshot response
#[derive(Debug, Serialize)]
pub struct SnapshotResponse {
    pub success: bool,
    pub snapshot: String,
    pub url: String,
    pub title: String,
    pub interactive_count: usize,
    pub truncated: bool,
}

/// Act request
#[derive(Debug, Deserialize)]
pub struct ActRequest {
    pub profile: String,
    pub target_id: String,
    pub ref_id: usize,
    #[serde(flatten)]
    pub action: super::aria_snapshot::ActKind,
}

/// Act response
#[derive(Debug, Serialize)]
pub struct ActResponse {
    pub success: bool,
    pub message: String,
}

/// Screenshot request
#[derive(Debug, Deserialize)]
pub struct ScreenshotRequest {
    pub profile: String,
    pub target_id: String,
    pub full_page: Option<bool>,
}

/// Screenshot response
#[derive(Debug, Serialize)]
pub struct ScreenshotResponse {
    pub success: bool,
    pub format: String,
    pub data: String,
}

/// Status response
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub profiles: Vec<ProfileStatus>,
}

/// Per-profile status
#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileStatus {
    pub name: String,
    pub page_count: usize,
}

/// Start request
#[derive(Debug, Deserialize)]
pub struct StartRequest {
    pub profile: String,
}

/// Stop request
#[derive(Debug, Deserialize)]
pub struct StopRequest {
    pub profile: String,
}

/// Generic success/error response
#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub success: bool,
    pub message: String,
}

impl BrowserBridge {
    /// Create a new bridge server
    pub fn new(pool: Arc<BrowserPool>, port: u16) -> Self {
        let token = uuid::Uuid::new_v4().to_string();
        let state = BridgeState { pool, token: token.clone() };
        Self { state, port }
    }

    /// Get the bearer token for auth
    pub fn token(&self) -> &str {
        &self.state.token
    }

    /// Get the server port
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Start the bridge server
    pub async fn start(&mut self) -> crate::Result<u16> {
        let app = Self::router(self.state.clone());
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));

        let listener = TcpListener::bind(addr).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: format!("Failed to bind bridge server to {}", addr),
                cause: Some(Box::new(e)),
            }
        })?;

        let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(self.port);
        self.port = actual_port;
        info!(port = actual_port, "Browser bridge server starting");

        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                error!("Bridge server error: {}", e);
            }
        });

        Ok(actual_port)
    }

    /// Shut down the browser pool used by this bridge.
    pub async fn shutdown(&self) {
        self.state.pool.shutdown().await;
    }

    /// Build the Axum router
    fn router(state: BridgeState) -> Router {
        Router::new()
            .route("/navigate", post(navigate_handler))
            .route("/snapshot", post(snapshot_handler))
            .route("/act", post(act_handler))
            .route("/screenshot", post(screenshot_handler))
            .route("/status", get(status_handler))
            .route("/start", post(start_handler))
            .route("/stop", post(stop_handler))
            .route("/health", get(health_handler))
            .with_state(state)
    }
}

/// Auth middleware helper: verify bearer token
fn check_auth(token: &str, state: &BridgeState) -> Result<(), StatusCode> {
    if token == state.token {
        Ok(())
    } else {
        warn!("Bridge auth failed: invalid token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Extract bearer token from Authorization header
fn extract_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}

/// Health check (no auth required)
async fn health_handler() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// Navigate to a URL and return page handle
async fn navigate_handler(
    State(state): State<BridgeState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<NavigateRequest>,
) -> Result<Json<NavigateResponse>, StatusCode> {
    let token = extract_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    check_auth(token, &state)?;

    debug!(profile = %req.profile, url = %req.url, "Bridge navigate");

    let handle = state
        .pool
        .new_page(&req.profile, &req.url)
        .await
        .map_err(|e| {
            error!("Navigate failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let title = handle
        .page
        .get_title()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    Ok(Json(NavigateResponse {
        success: true,
        target_id: handle.target_id,
        url: req.url,
        title,
    }))
}

/// Take an ARIA snapshot of a page
async fn snapshot_handler(
    State(state): State<BridgeState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SnapshotRequest>,
) -> Result<Json<SnapshotResponse>, StatusCode> {
    let token = extract_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    check_auth(token, &state)?;

    debug!(profile = %req.profile, target_id = %req.target_id, "Bridge snapshot");

    let instance = state.pool.get_or_create(&req.profile).await.map_err(|e| {
        error!("Failed to get browser instance: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let page_handle = instance.get_page(&req.target_id).await.ok_or_else(|| {
        warn!("Page not found: {}", req.target_id);
        StatusCode::NOT_FOUND
    })?;

    let max_chars = req.max_chars.unwrap_or(8000);
    let snapshot = super::aria_snapshot::aria_snapshot(&page_handle.page, max_chars)
        .await
        .map_err(|e| {
            error!("Snapshot failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(SnapshotResponse {
        success: true,
        snapshot: snapshot.to_text(),
        url: snapshot.url.clone(),
        title: snapshot.title.clone(),
        interactive_count: snapshot.interactive_count(),
        truncated: snapshot.truncated,
    }))
}

/// Act on an element by ref_id
async fn act_handler(
    State(state): State<BridgeState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ActRequest>,
) -> Result<Json<ActResponse>, StatusCode> {
    let token = extract_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    check_auth(token, &state)?;

    debug!(profile = %req.profile, target_id = %req.target_id, ref_id = req.ref_id, "Bridge act");

    let instance = state.pool.get_or_create(&req.profile).await.map_err(|e| {
        error!("Failed to get browser instance: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let page_handle = instance.get_page(&req.target_id).await.ok_or_else(|| {
        warn!("Page not found: {}", req.target_id);
        StatusCode::NOT_FOUND
    })?;

    let message = super::aria_snapshot::act_by_ref(&page_handle.page, req.ref_id, req.action)
        .await
        .map_err(|e| {
            error!("Act failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(ActResponse { success: true, message }))
}

/// Take a screenshot of a page
async fn screenshot_handler(
    State(state): State<BridgeState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ScreenshotRequest>,
) -> Result<Json<ScreenshotResponse>, StatusCode> {
    let token = extract_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    check_auth(token, &state)?;

    debug!(profile = %req.profile, target_id = %req.target_id, "Bridge screenshot");

    let instance = state.pool.get_or_create(&req.profile).await.map_err(|e| {
        error!("Failed to get browser instance: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let page_handle = instance.get_page(&req.target_id).await.ok_or_else(|| {
        warn!("Page not found: {}", req.target_id);
        StatusCode::NOT_FOUND
    })?;

    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParamsBuilder;

    let params = if req.full_page.unwrap_or(false) {
        ScreenshotParamsBuilder::default()
            .format(CaptureScreenshotFormat::Png)
            .full_page(true)
            .build()
    } else {
        ScreenshotParamsBuilder::default()
            .format(CaptureScreenshotFormat::Png)
            .build()
    };

    let data = page_handle.page.screenshot(params).await.map_err(|e| {
        error!("Screenshot failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let base64 = base64::engine::general_purpose::STANDARD.encode(&data);

    Ok(Json(ScreenshotResponse {
        success: true,
        format: "png".to_string(),
        data: format!("data:image/png;base64,{}", base64),
    }))
}

/// Get status of all browser instances
async fn status_handler(
    State(state): State<BridgeState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<StatusResponse>, StatusCode> {
    let token = extract_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    check_auth(token, &state)?;

    let status = state.pool.status().await;
    let profiles = status
        .into_iter()
        .map(|(name, page_count)| ProfileStatus { name, page_count })
        .collect();

    Ok(Json(StatusResponse { profiles }))
}

/// Start (ensure) a browser instance for a profile
async fn start_handler(
    State(state): State<BridgeState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<StartRequest>,
) -> Result<Json<MessageResponse>, StatusCode> {
    let token = extract_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    check_auth(token, &state)?;

    debug!(profile = %req.profile, "Bridge start");

    state.pool.get_or_create(&req.profile).await.map_err(|e| {
        error!("Failed to start browser instance: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(MessageResponse {
        success: true,
        message: format!("Browser instance '{}' started", req.profile),
    }))
}

/// Stop a browser instance for a profile
async fn stop_handler(
    State(state): State<BridgeState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<StopRequest>,
) -> Result<Json<MessageResponse>, StatusCode> {
    let token = extract_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    check_auth(token, &state)?;

    debug!(profile = %req.profile, "Bridge stop");

    state.pool.close_profile(&req.profile).await;

    Ok(Json(MessageResponse {
        success: true,
        message: format!("Browser instance '{}' stopped", req.profile),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_create() {
        let config = BrowserPoolConfig::default();
        let pool = Arc::new(BrowserPool::new(config));
        let bridge = BrowserBridge::new(pool, 18800);
        assert_eq!(bridge.port(), 18800);
        assert!(!bridge.token().is_empty());
    }

    #[tokio::test]
    async fn test_bridge_health_no_auth() {
        let config = BrowserPoolConfig::default();
        let pool = Arc::new(BrowserPool::new(config));
        let mut bridge = BrowserBridge::new(pool, 0);

        // Start on a random port
        let port = bridge.start().await.unwrap();
        let client = reqwest::Client::new();
        let res = client
            .get(format!("http://127.0.0.1:{}/health", port))
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), 200);
        let body: serde_json::Value = res.json().await.unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_bridge_status_requires_auth() {
        let config = BrowserPoolConfig::default();
        let pool = Arc::new(BrowserPool::new(config));
        let mut bridge = BrowserBridge::new(pool, 0);
        let port = bridge.start().await.unwrap();

        let client = reqwest::Client::new();
        let res = client
            .get(format!("http://127.0.0.1:{}/status", port))
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), 401);
    }

    #[tokio::test]
    async fn test_bridge_status_with_auth() {
        let config = BrowserPoolConfig::default();
        let pool = Arc::new(BrowserPool::new(config));
        let mut bridge = BrowserBridge::new(pool, 0);
        let port = bridge.start().await.unwrap();

        let client = reqwest::Client::new();
        let res = client
            .get(format!("http://127.0.0.1:{}/status", port))
            .bearer_auth(bridge.token())
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), 200);
        let body: StatusResponse = res.json().await.unwrap();
        assert!(body.profiles.is_empty());
    }

    use super::super::profile::BrowserPoolConfig;
}
