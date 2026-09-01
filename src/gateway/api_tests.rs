//! Gateway API route integration tests
//!
//! Tests for the admin-tier HTTP API endpoints.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use tower::ServiceExt;

use super::*;

// ── GET /api/v1/channels ──

#[tokio::test]
async fn list_channels_returns_empty_array() {
    // channel_list_handler reads from config.channels, not running channels.
    // make_test_state's default config has no channels.
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/channels", get(super::channel_list_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/channels")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["channels"].is_array());
    assert_eq!(json["channels"].as_array().unwrap().len(), 0);
}

// ── GET /api/v1/models ──

#[tokio::test]
async fn list_models_returns_array() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/models", get(super::list_models_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/models")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["models"].is_array());
}

// ── GET /api/v1/config ──

#[tokio::test]
async fn get_config_returns_json() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/config", get(super::get_config_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/config")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_object());
}

// ── GET /api/v1/mentions/policy ──

#[tokio::test]
async fn get_mention_policy_returns_policy() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/mentions/policy", get(super::get_mention_policy_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/mentions/policy")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["policy"].is_string());
}

// ── GET /ready (not ready by default) ──

#[tokio::test]
async fn ready_handler_returns_503_when_not_ready() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/ready", get(super::ready_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/ready")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ready"], false);
}

// ── POST /api/v1/mentions/policy ──

#[tokio::test]
async fn set_mention_policy_updates_policy() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route(
            "/api/v1/mentions/policy",
            get(super::get_mention_policy_handler).post(super::set_mention_policy_handler),
        )
        .with_state(state.clone());

    // Set policy to block (snake_case deserialization)
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/mentions/policy")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"policy":"block"}"#))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify policy changed
    let req = Request::builder()
        .uri("/api/v1/mentions/policy")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["policy"], "block");
}

// ── Auth mode ambiguity detection ──

use crate::gateway::protocol::AuthMode;

#[test]
fn validate_auth_passes_when_security_disabled() {
    let mut config = GatewayConfig::default();
    config.security.enabled = false;
    config.security.shared_token = Some("test-token".into());
    // auth_mode is None by default — should NOT fail
    assert!(super::validate_auth_config(&config).is_ok());
}

#[test]
fn validate_auth_passes_when_only_token_configured() {
    let mut config = GatewayConfig::default();
    config.security.auth_required = true;
    config.security.shared_token = Some("test-token".into());
    config.security.auth_mode = AuthMode::None;
    // Only token — should warn but not fail
    assert!(super::validate_auth_config(&config).is_ok());
}

#[test]
fn validate_auth_passes_when_token_mode_explicit() {
    let mut config = GatewayConfig::default();
    config.security.enabled = true;
    config.security.auth_required = true;
    config.security.shared_token = Some("test-token".into());
    config.security.auth_mode = AuthMode::Token;
    // Token mode explicitly set — should pass
    assert!(super::validate_auth_config(&config).is_ok());
}
