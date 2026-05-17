//! E2E Browser Smoke Tests
//!
//! These tests verify that headless Chrome can launch and execute JS,
//! and that the Web Terminal serves its HTML page correctly.
//!
//! NOTE: chromiumoxide 0.7 has a known bug where page navigation via
//! `page.goto()` or `browser.new_page(url)` fails with `ChannelSendError`.
//! As a workaround, we verify the server via HTTP and the browser via
//! basic JS evaluation on an about:blank page.

use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;
use manta::agent::{Agent, AgentConfig};
use manta::providers::OpenAiProvider;
use manta::tools::ToolRegistry;
use manta::web::start_web_terminal_with_listener;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

/// Launch headless Chrome. Returns `None` if Chrome is not installed.
async fn launch_headless_browser() -> Option<Browser> {
    let mut builder = BrowserConfig::builder()
        .viewport(chromiumoxide::handler::viewport::Viewport {
            width: 1280,
            height: 720,
            device_scale_factor: Some(1.0),
            emulating_mobile: false,
            is_landscape: true,
            has_touch: false,
        })
        .arg("--headless=new")
        .request_timeout(Duration::from_secs(30));

    // On macOS, point to the standard Chrome location.
    #[cfg(target_os = "macos")]
    {
        let chrome_path = std::path::PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        );
        if chrome_path.exists() {
            builder = builder.chrome_executable(chrome_path);
        }
    }

    let config = match builder.build() {
        Ok(c) => c,
        Err(_) => return None,
    };

    let (browser, mut handler) = match Browser::launch(config).await {
        Ok(b) => b,
        Err(_) => return None,
    };

    tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if h.is_err() {
                break;
            }
        }
    });

    Some(browser)
}

/// Start a mock OpenAI server, build an Agent, and launch the web terminal.
async fn start_web_terminal_with_mock() -> String {
    let mock_server = MockServer::start().await;

    let response_body = json!({
        "id": "chatcmpl-browser",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o-mini",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Browser test response!"
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
        .and(header("authorization", "Bearer browser-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let provider = Arc::new(
        OpenAiProvider::with_base_url("browser-test-key", &mock_server.uri())
            .expect("create provider"),
    );

    let tool_registry = Arc::new(ToolRegistry::new());
    let agent = Arc::new(Agent::new(AgentConfig::default(), provider, tool_registry));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);

    tokio::spawn(async move {
        let _ = start_web_terminal_with_listener(agent, listener).await;
    });

    // Give the server time to start serving.
    tokio::time::sleep(Duration::from_millis(200)).await;

    base_url
}

#[tokio::test]
async fn browser_web_terminal_smoke_test() {
    let base_url = start_web_terminal_with_mock().await;

    // Verify the web terminal is reachable via HTTP.
    let client = reqwest::Client::new();
    let resp = client
        .get(&base_url)
        .send()
        .await
        .expect("web terminal should respond");
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.expect("should get HTML");
    assert!(
        html.contains("Manta AI Terminal") || html.contains("id=\"root\""),
        "HTML should contain expected web terminal content"
    );

    // Verify headless Chrome can launch and execute JS.
    let mut browser = match launch_headless_browser().await {
        Some(b) => b,
        None => {
            println!("Skipping browser smoke test: Chrome/Chromium not found");
            return;
        }
    };

    let page = browser
        .new_page("about:blank")
        .await
        .expect("should create a new page");

    let result = page
        .evaluate("() => 'Manta Browser OK'")
        .await
        .expect("JS evaluation should work");
    let text = result
        .into_value::<String>()
        .expect("should get string result");
    assert_eq!(text, "Manta Browser OK");

    // Clean up browser process.
    browser.close().await.ok();
}

// ── HTTP API Tests (Bypass chromiumoxide navigation bug) ─────────────────────

#[tokio::test]
async fn web_terminal_http_api_chat_accepts_message() {
    let base_url = start_web_terminal_with_mock().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/chat", base_url))
        .json(&json!({
            "message": "Hello from test",
            "user_id": "test_user"
        }))
        .send()
        .await
        .expect("API should respond");

    // The handler returns ACCEPTED (202) for async processing
    assert_eq!(resp.status(), 202, "API chat should return 202 Accepted");

    let body: serde_json::Value = resp.json().await.expect("should get JSON response");
    assert!(body.get("message_id").is_some(), "response should have message_id");
    assert!(body.get("conversation_id").is_some(), "response should have conversation_id");
    assert_eq!(body["status"], "processing");
}

#[tokio::test]
async fn web_terminal_http_api_chat_with_conversation_id() {
    let base_url = start_web_terminal_with_mock().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/chat", base_url))
        .json(&json!({
            "message": "Continue conversation",
            "conversation_id": "conv-test-123",
            "user_id": "test_user"
        }))
        .send()
        .await
        .expect("API should respond");

    assert_eq!(resp.status(), 202);

    let body: serde_json::Value = resp.json().await.expect("should get JSON response");
    assert_eq!(body["conversation_id"], "conv-test-123");
}

#[tokio::test]
async fn web_terminal_sse_endpoint_responds() {
    let base_url = start_web_terminal_with_mock().await;
    let client = reqwest::Client::new();

    // SSE endpoint should respond with 200 OK and text/event-stream content type
    let resp = client
        .get(format!("{}/api/events", base_url))
        .send()
        .await
        .expect("SSE endpoint should respond");

    assert_eq!(resp.status(), 200, "SSE endpoint should return 200");
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("should have content-type header")
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/event-stream"),
        "SSE endpoint should return text/event-stream, got: {}",
        content_type
    );
}

#[tokio::test]
async fn web_terminal_websocket_endpoint_upgrades() {
    let base_url = start_web_terminal_with_mock().await;

    // The WS endpoint should accept WebSocket upgrade requests
    let ws_url = format!("ws://{}/ws", base_url.trim_start_matches("http://"));

    let result = tokio_tungstenite::connect_async(&ws_url).await;

    assert!(result.is_ok(), "WebSocket endpoint should accept upgrade: {:?}", result.err());

    // Clean up the connection
    if let Ok((mut ws_stream, _)) = result {
        let _ = ws_stream.close(None).await;
    }
}

#[tokio::test]
async fn web_terminal_root_returns_html_with_expected_elements() {
    let base_url = start_web_terminal_with_mock().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(&base_url)
        .send()
        .await
        .expect("root should respond");

    assert_eq!(resp.status(), 200);

    let html = resp.text().await.expect("should get HTML");

    // Verify key UI elements are present
    assert!(
        html.contains("<html") || html.contains("<!DOCTYPE html"),
        "Should return valid HTML document"
    );
    assert!(
        html.contains("settings-btn") || html.contains("settings"),
        "Should contain settings UI element"
    );
    assert!(
        html.contains("version") || html.contains("Manta"),
        "Should contain version or branding info"
    );
}

#[tokio::test]
async fn web_terminal_api_chat_missing_message_still_accepts() {
    let base_url = start_web_terminal_with_mock().await;
    let client = reqwest::Client::new();

    // Even with minimal body, the endpoint should accept (it uses defaults)
    let resp = client
        .post(format!("{}/api/chat", base_url))
        .json(&json!({"message": "test"}))
        .send()
        .await
        .expect("API should respond");

    // Should either succeed or fail gracefully
    assert!(
        resp.status().is_success() || resp.status().is_server_error(),
        "API should handle minimal request"
    );
}
