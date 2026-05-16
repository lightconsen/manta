//! E2E Server Flow Tests
//!
//! These tests start a real HTTP server with an Agent, Engine, and mock
//! Provider, then exercise the REST API via reqwest. This is the closest
//! Manta gets to true end-to-end testing without spawning the binary.

use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use manta::{
    agent::{Agent, AgentConfig},
    core::Engine,
    providers::{CompletionResponse, Message, OpenAiProvider, Provider, Role},
    server::AppState,
    tools::ToolRegistry,
};
use serde_json::json;
use serial_test::serial;
use std::sync::Arc;
use tokio::net::TcpListener;
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

// Simple health handler for E2E
async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let agent_status = if state.agent.is_some() {
        "ready"
    } else {
        "disabled"
    };
    Json(json!({
        "status": "healthy",
        "agent": agent_status,
    }))
}

// Simple root handler for E2E
async fn root_handler(State(state): State<AppState>) -> impl IntoResponse {
    let agent_status = if state.agent.is_some() {
        "available"
    } else {
        "not configured"
    };
    Json(json!({
        "name": "Manta",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "running",
        "agent": agent_status,
    }))
}

// Chat handler for E2E
async fn chat_handler(
    State(state): State<AppState>,
    Json(request): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(agent) = &state.agent {
        let message = request
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let incoming = manta::channels::IncomingMessage::new("user", "e2e-session", message);

        match agent.process_message(incoming).await {
            Ok(response) => {
                let resp = json!({
                    "response": response.content,
                    "conversation_id": "e2e-session",
                });
                (StatusCode::OK, Json(resp))
            }
            Err(e) => {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            }
        }
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "AI agent not configured"})),
        )
    }
}

/// Start a test server and return its base URL
async fn start_test_server(state: AppState) -> String {
    let app = Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .route("/chat", post(chat_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    base_url
}

// ── E2E Tests ────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn e2e_health_endpoint_without_agent() {
    let engine = Arc::new(Engine::new());
    let state = AppState {
        engine,
        agent: None,
        cron_tx: {
            let (tx, _) = tokio::sync::broadcast::channel(10);
            tx
        },
    };

    let base_url = start_test_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["agent"], "disabled");
}

#[tokio::test]
#[serial]
async fn e2e_root_endpoint_with_agent() {
    let mock_server = MockServer::start().await;

    let response_body = json!({
        "id": "chatcmpl-e2e",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o-mini",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from E2E!"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 4,
            "total_tokens": 9
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer e2e-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let provider = Arc::new(
        OpenAiProvider::with_base_url("e2e-key", &mock_server.uri())
            .expect("create provider"),
    );

    let tool_registry = Arc::new(ToolRegistry::new());
    let agent = Arc::new(Agent::new(
        AgentConfig::default(),
        provider,
        tool_registry,
    ));

    let engine = Arc::new(Engine::new());
    let state = AppState {
        engine,
        agent: Some(agent),
        cron_tx: {
            let (tx, _) = tokio::sync::broadcast::channel(10);
            tx
        },
    };

    let base_url = start_test_server(state).await;

    let client = reqwest::Client::new();

    // Test root endpoint
    let resp = client
        .get(&base_url)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Manta");
    assert_eq!(body["agent"], "available");

    // Test health endpoint with agent
    let resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["agent"], "ready");
}

#[tokio::test]
#[serial]
async fn e2e_chat_endpoint_full_roundtrip() {
    let mock_server = MockServer::start().await;

    let response_body = json!({
        "id": "chatcmpl-e2e-chat",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o-mini",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "E2E response received!"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 8,
            "completion_tokens": 5,
            "total_tokens": 13
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer e2e-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let provider = Arc::new(
        OpenAiProvider::with_base_url("e2e-key", &mock_server.uri())
            .expect("create provider"),
    );

    let tool_registry = Arc::new(ToolRegistry::new());
    let agent = Arc::new(Agent::new(
        AgentConfig::default(),
        provider,
        tool_registry,
    ));

    let engine = Arc::new(Engine::new());
    let state = AppState {
        engine,
        agent: Some(agent),
        cron_tx: {
            let (tx, _) = tokio::sync::broadcast::channel(10);
            tx
        },
    };

    let base_url = start_test_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/chat", base_url))
        .json(&json!({"message": "Hello from E2E test"}))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["response"], "E2E response received!");
    assert_eq!(body["conversation_id"], "e2e-session");
}

#[tokio::test]
#[serial]
async fn e2e_chat_without_agent_returns_503() {
    let engine = Arc::new(Engine::new());
    let state = AppState {
        engine,
        agent: None,
        cron_tx: {
            let (tx, _) = tokio::sync::broadcast::channel(10);
            tx
        },
    };

    let base_url = start_test_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/chat", base_url))
        .json(&json!({"message": "Hello"}))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), 503);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "AI agent not configured");
}
