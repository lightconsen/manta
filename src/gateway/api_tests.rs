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

// ── GET /api/v1/agents ──

#[tokio::test]
async fn list_agents_returns_empty_array() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/agents", get(super::list_agents_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/agents")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 0);
}

// ── GET /api/v1/channels ──

#[tokio::test]
async fn list_channels_returns_empty_array() {
    // list_channels_handler reads from state.channels.channels (running channels),
    // not config.channels. make_test_state does not populate running channels.
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/channels", get(super::list_channels_handler))
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
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 0);
}

// ── GET /api/v1/providers ──

#[tokio::test]
async fn list_providers_returns_array() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/providers", get(super::list_providers_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/providers")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["providers"].is_array());
    assert!(json["count"].is_number());
}

// ── GET /api/v1/providers/:id/health (not found) ──

#[tokio::test]
async fn get_provider_health_not_found() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/providers/:id/health", get(super::get_provider_health_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/providers/nonexistent/health")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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

// ── GET /api/v1/plugins ──

#[tokio::test]
async fn list_plugins_returns_empty() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/plugins", get(super::list_plugins_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/plugins")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["plugins"].is_array());
}

// ── GET /api/v1/skills ──

#[tokio::test]
async fn list_skills_returns_empty() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/skills", get(super::list_skills_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/skills")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["skills"].is_array());
}

// ── GET /api/v1/cron ──

#[tokio::test]
async fn list_cron_jobs_returns_empty() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/cron", get(super::list_cron_jobs_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/cron")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["jobs"].is_array());
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

// ── GET /api/v1/status ──

#[tokio::test]
async fn api_status_returns_summary() {
    let state =
        Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
    let app = Router::new()
        .route("/api/v1/status", get(super::status_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/v1/status")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["agents"]["total"].is_number());
    assert!(json["channels"].is_number());
    assert!(json["version"].is_string());
}

// ── Auth mode ambiguity detection ──

use crate::gateway::protocol::AuthMode;

#[test]
fn validate_auth_passes_when_security_disabled() {
    let mut config = GatewayConfig::default();
    config.security.enabled = false;
    config.security.shared_token = Some("test-token".into());
    config.security.oauth.enabled = true;
    config.security.oauth.github = Some(crate::gateway::auth::OAuthProviderConfig {
        client_id: "abc".into(),
        client_secret: "secret".into(),
        auth_url: None,
        token_url: None,
        redirect_uri: "http://localhost/callback".into(),
        scopes: vec!["user:email".into()],
    });
    // auth_mode is None by default — should NOT fail
    assert!(super::validate_auth_config(&config).is_ok());
}

#[test]
fn validate_auth_passes_when_only_token_configured() {
    let mut config = GatewayConfig::default();
    config.security.auth_required = true;
    config.security.shared_token = Some("test-token".into());
    config.security.oauth.enabled = false;
    config.security.auth_mode = AuthMode::None;
    // Only token, no OAuth — should warn but not fail
    assert!(super::validate_auth_config(&config).is_ok());
}

#[test]
fn validate_auth_fails_when_both_token_and_oauth_configured() {
    let mut config = GatewayConfig::default();
    config.security.enabled = true;
    config.security.auth_required = true;
    config.security.shared_token = Some("test-token".into());
    config.security.oauth.enabled = true;
    config.security.oauth.github = Some(crate::gateway::auth::OAuthProviderConfig {
        client_id: "abc".into(),
        client_secret: "secret".into(),
        auth_url: None,
        token_url: None,
        redirect_uri: "http://localhost/callback".into(),
        scopes: vec!["user:email".into()],
    });
    config.security.auth_mode = AuthMode::None;
    // Both configured, mode unset — should fail
    assert!(super::validate_auth_config(&config).is_err());
}

#[test]
fn validate_auth_passes_when_mode_explicitly_set() {
    let mut config = GatewayConfig::default();
    config.security.enabled = true;
    config.security.auth_required = true;
    config.security.shared_token = Some("test-token".into());
    config.security.oauth.enabled = true;
    config.security.oauth.github = Some(crate::gateway::auth::OAuthProviderConfig {
        client_id: "abc".into(),
        client_secret: "secret".into(),
        auth_url: None,
        token_url: None,
        redirect_uri: "http://localhost/callback".into(),
        scopes: vec!["user:email".into()],
    });
    config.security.auth_mode = AuthMode::Token;
    // Both configured, mode explicitly set — should pass
    assert!(super::validate_auth_config(&config).is_ok());
}
