//! End-to-End WebSocket Tests
//!
//! Simulates a complete frontend client connecting via WebSocket.

pub use std::collections::VecDeque;
pub use std::path::Path;
use std::sync::Arc;
pub use std::time::Duration;

pub use futures_util::{SinkExt, StreamExt};
pub use serde_json::json;
pub use serial_test::serial;
pub use syscity::gateway::protocol::AuthMode;
pub use syscity::gateway::{Gateway, GatewayConfig};
pub use syscity::model_router::{ModelAlias, ProviderConfig, ProviderType};
pub use syscity::providers::{
    mock::MockProvider, FunctionCall, Message as ProviderMessage, Role, ToolCall,
};
pub use tokio::time::timeout;
pub use tokio_tungstenite::{
    connect_async, connect_async_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig,
};

// ── Type Aliases
// ──────────────────────────────────────────────────────────────

pub type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
pub type WsWrite = futures_util::stream::SplitSink<WsStream, Message>;
pub type WsRead = futures_util::stream::SplitStream<WsStream>;

// ── API Key Discovery
// ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LocalProviderConfig {
    pub name: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub is_anthropic: bool,
}

/// Parse API configuration from `start_local_*.sh` shell scripts.
pub fn discover_local_providers() -> Vec<LocalProviderConfig> {
    let mut providers = Vec::new();
    let scripts = ["start-local-qwen.sh", "start-local-kimi.sh"];

    for script in &scripts {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(script);
        if !path.exists() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let mut api_key = None;
            let mut base_url = None;
            let mut model = None;
            let mut is_anthropic = false;

            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("export SYSCITY_API_KEY=") {
                    api_key = line
                        .split('=')
                        .nth(1)
                        .map(|s| s.trim().trim_matches('"').to_string());
                }
                if line.starts_with("export SYSCITY_BASE_URL=") {
                    base_url = line
                        .split('=')
                        .nth(1)
                        .map(|s| s.trim().trim_matches('"').to_string());
                }
                if line.starts_with("export SYSCITY_MODEL=") {
                    model = line
                        .split('=')
                        .nth(1)
                        .map(|s| s.trim().trim_matches('"').to_string());
                }
                if line.starts_with("export SYSCITY_IS_ANTHROPIC=") {
                    is_anthropic = line.contains("true");
                }
            }

            // Default model if not specified in script
            let mdl = model.unwrap_or_else(|| "gpt-4o-mini".to_string());

            if let (Some(key), Some(url)) = (api_key, base_url) {
                let name = if script.contains("qwen") {
                    "qwen".to_string()
                } else if script.contains("kimi") {
                    "kimi".to_string()
                } else {
                    "local".to_string()
                };
                providers.push(LocalProviderConfig {
                    name,
                    api_key: key,
                    base_url: url,
                    model: mdl,
                    is_anthropic,
                });
            }
        }
    }

    providers
}

pub fn pick_test_provider() -> Option<LocalProviderConfig> {
    if let (Ok(key), Ok(name)) = (
        std::env::var("SYSCITY_TEST_PROVIDER_KEY"),
        std::env::var("SYSCITY_TEST_PROVIDER"),
    ) {
        let base_url = std::env::var("SYSCITY_TEST_BASE_URL").unwrap_or_default();
        let model =
            std::env::var("SYSCITY_TEST_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        let is_anthropic = name == "anthropic" || name == "kimi";
        return Some(LocalProviderConfig {
            name,
            api_key: key,
            base_url,
            model,
            is_anthropic,
        });
    }

    discover_local_providers().into_iter().next()
}

pub fn skip_if_no_provider() -> Option<LocalProviderConfig> {
    let provider = pick_test_provider();
    if provider.is_none() {
        eprintln!(
            "Skipping LLM test: no provider configured. Set SYSCITY_TEST_PROVIDER_KEY + \
             SYSCITY_TEST_PROVIDER env vars, or create start-local-*.sh scripts in the project \
             root."
        );
    }
    provider
}

// ── Gateway Setup
// ─────────────────────────────────────────────────────────────

pub fn test_config(port: u16, with_provider: bool) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.host = "127.0.0.1".to_string();
    config.port = port;
    config.storage.storage_type = "sqlite".to_string();
    let db_path = std::env::temp_dir().join(format!("syscity_e2e_ws_test_{}.db", port));
    let _ = std::fs::remove_file(&db_path);
    config.storage.database_url = Some(format!("sqlite:{}", db_path.display()));
    config.security.auth_mode = AuthMode::None;
    config.plugins.enabled = false;
    config.channels.clear();
    config.vector_memory.enabled = false;

    if with_provider {
        if let Some(provider) = pick_test_provider() {
            let provider_type = if provider.is_anthropic {
                ProviderType::Anthropic
            } else {
                ProviderType::OpenAi
            };
            let provider_config = ProviderConfig {
                provider_type,
                api_key: provider.api_key.into(),
                api_keys: vec![],
                auth_profile: None,
                oauth: None,
                base_url: if provider.base_url.is_empty() {
                    None
                } else {
                    Some(provider.base_url)
                },
                timeout: Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };
            config
                .providers
                .insert(provider.name.clone(), provider_config);
            config.model_provider = provider.name;
            config.model = provider.model;
        }
    }
    config
}

pub async fn start_test_gateway(port: u16, with_provider: bool) {
    let config = test_config(port, with_provider);
    let gateway = Gateway::new(config, None)
        .await
        .expect("Failed to create test gateway");

    tokio::spawn(async move {
        let _ = gateway.start().await;
    });

    let url = format!("ws://127.0.0.1:{}/ws", port);
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if connect_async(&url).await.is_ok() {
            return;
        }
    }
    panic!("Gateway did not start within 10 seconds");
}

/// Start a test Gateway with a programmable MockProvider injected.
///
/// The `mock` is pre-configured with responses (sequence or callback) and
/// registered as the default provider under the name "mock".
pub async fn start_test_gateway_with_mock(port: u16, mock: MockProvider) {
    let mut config = test_config(port, false);
    config.model_provider = "mock".to_string();
    config.model = "mock-model".to_string();

    let gateway = Gateway::new(config, None)
        .await
        .expect("Failed to create test gateway");

    let router = gateway.model_router();
    router
        .add_provider_instance("mock", std::sync::Arc::new(mock))
        .await
        .expect("Failed to register mock provider");

    // Register a model alias so the router can resolve "mock-model" -> mock
    // provider
    router
        .set_alias(ModelAlias {
            name: "mock-model".to_string(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            temperature: None,
            max_tokens: None,
        })
        .await;

    tokio::spawn(async move {
        let _ = gateway.start().await;
    });

    let url = format!("ws://127.0.0.1:{}/ws", port);
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if connect_async(&url).await.is_ok() {
            return;
        }
    }
    panic!("Gateway did not start within 10 seconds");
}

// ── Mock Provider Builders ───────────────────────────────────────────────────

/// Build a MockProvider that drives a two-turn tool conversation.
///
/// First turn emits a `ToolCall` for `expected_tool` (arguments `{}`).
/// Second turn returns a final answer after seeing the tool result.
/// Handles NOCACHE cache-check prompts automatically.
pub fn tool_mock_provider(expected_tool: &str) -> MockProvider {
    let tool = expected_tool.to_string();
    MockProvider::new().with_callback(move |messages| {
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }
        let has_tool_result = messages.iter().any(|m| m.role == Role::Tool);
        if has_tool_result {
            return ProviderMessage::assistant("Done! I've completed the task.");
        }
        ProviderMessage::assistant("I'll help with that.").with_tool_calls(vec![ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: tool.clone(),
                arguments: "{}".to_string(),
            },
            index: None,
            result: None,
        }])
    })
}

/// Build a MockProvider for simple LLM streaming tests.
///
/// Returns a fixed response for normal prompts.
/// Handles NOCACHE cache-check prompts automatically.
pub fn llm_mock_provider_for_streaming() -> MockProvider {
    MockProvider::new().with_callback(|messages| {
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }
        ProviderMessage::assistant("pong-from-llm")
    })
}

/// Build a MockProvider for LLM tool-invocation tests.
///
/// First turn emits a `ToolCall` for the given tool name.
/// Second turn returns a final answer after seeing the tool result.
/// Handles NOCACHE cache-check prompts automatically.
pub fn llm_mock_provider_for_tool(tool_name: &str) -> MockProvider {
    let tool = tool_name.to_string();
    MockProvider::new().with_callback(move |messages| {
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }
        let has_tool_result = messages.iter().any(|m| m.role == Role::Tool);
        if has_tool_result {
            return ProviderMessage::assistant("Done!");
        }
        ProviderMessage::assistant("Let me check that.").with_tool_calls(vec![ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: tool.clone(),
                arguments: "{}".to_string(),
            },
            index: None,
            result: None,
        }])
    })
}

// ── Frontend Simulator
// ────────────────────────────────────────────────────────

/// Simulates a web frontend connected over WebSocket.
pub struct FrontendSimulator {
    pub write: WsWrite,
    pub read: WsRead,
    pub session_id: Option<String>,
    event_buffer: VecDeque<serde_json::Value>,
}

impl FrontendSimulator {
    pub async fn connect(port: u16) -> Self {
        let url = format!("ws://127.0.0.1:{}/ws", port);
        let config = WebSocketConfig {
            max_frame_size: Some(128 << 20), // 128 MB — large enough for screenshot base64
            max_message_size: Some(128 << 20),
            ..Default::default()
        };
        let (ws_stream, _) = connect_async_with_config(&url, Some(config), false)
            .await
            .expect("Failed to connect to WebSocket");
        let (mut write, mut read) = ws_stream.split();

        let connect_req = json!({
            "type": "req",
            "id": "connect-1",
            "method": "connect",
            "params": {
                "protocol_version": 1,
                "scopes": ["chat", "read", "write", "admin"],
                "auth": {}
            }
        });
        write
            .send(Message::Text(connect_req.to_string()))
            .await
            .unwrap();

        let msg = read.next().await.unwrap().unwrap();
        let response: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        assert!(
            response.get("ok").and_then(|v| v.as_bool()) == Some(true),
            "Handshake failed: {:?}",
            response.get("error")
        );

        Self {
            write,
            read,
            session_id: None,
            event_buffer: VecDeque::new(),
        }
    }

    pub async fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let req_id = format!("req-{}", uuid::Uuid::new_v4());
        let req = json!({
            "type": "req",
            "id": &req_id,
            "method": method,
            "params": params,
        });
        self.write
            .send(Message::Text(req.to_string()))
            .await
            .unwrap();

        let resp = timeout(Duration::from_secs(10), async {
            while let Some(msg) = self.read.next().await {
                let msg = msg.unwrap();
                if let Message::Text(text) = msg {
                    if let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) {
                        if frame.get("type").and_then(|v| v.as_str()) == Some("res")
                            && frame.get("id").and_then(|v| v.as_str()) == Some(&req_id)
                        {
                            return frame;
                        }
                        self.event_buffer.push_back(frame);
                    }
                }
            }
            panic!("WebSocket closed before response received");
        })
        .await
        .expect("Timeout waiting for response");

        resp
    }

    pub async fn wait_for_event(
        &mut self,
        event_name: &str,
        timeout_secs: u64,
    ) -> Option<serde_json::Value> {
        for i in 0..self.event_buffer.len() {
            if self.event_buffer[i].get("type").and_then(|v| v.as_str()) == Some("event")
                && self.event_buffer[i].get("event").and_then(|v| v.as_str()) == Some(event_name)
            {
                let event = self.event_buffer.remove(i).unwrap();
                return event.get("payload").cloned();
            }
        }

        let result = timeout(Duration::from_secs(timeout_secs), async {
            while let Some(msg) = self.read.next().await {
                let msg = msg.unwrap();
                if let Message::Text(text) = msg {
                    if let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) {
                        if frame.get("type").and_then(|v| v.as_str()) == Some("event") {
                            if frame.get("event").and_then(|v| v.as_str()) == Some(event_name) {
                                return frame.get("payload").cloned();
                            }
                        }
                        self.event_buffer.push_back(frame);
                    }
                }
            }
            None
        })
        .await;

        result.unwrap_or(None)
    }

    pub async fn collect_events(
        &mut self,
        event_name: &str,
        timeout_secs: u64,
    ) -> Vec<serde_json::Value> {
        let result = timeout(Duration::from_secs(timeout_secs), async {
            let mut events = Vec::new();
            while let Some(msg) = self.read.next().await {
                let msg = msg.unwrap();
                if let Message::Text(text) = msg {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                        if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                            if event.get("event").and_then(|v| v.as_str()) == Some(event_name) {
                                if let Some(payload) = event.get("payload").cloned() {
                                    events.push(payload);
                                }
                            }
                        }
                    }
                }
            }
            events
        })
        .await;

        result.unwrap_or_default()
    }

    pub async fn create_session(&mut self) -> String {
        let resp = self.request("sessions.create", json!({})).await;
        assert!(
            resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
            "sessions.create failed: {:?}",
            resp.get("error")
        );
        let sid = resp
            .get("payload")
            .unwrap()
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        self.session_id = Some(sid.clone());
        sid
    }

    pub async fn list_sessions(&mut self) -> Vec<serde_json::Value> {
        let resp = self.request("sessions.list", json!(null)).await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
        resp.get("payload")
            .unwrap()
            .get("sessions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    pub async fn delete_session(&mut self, session_id: &str) {
        let resp = self
            .request("sessions.delete", json!({"session_id": session_id}))
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    pub async fn subscribe(&mut self, session_ids: Vec<String>) {
        let resp = self
            .request("sessions.subscribe", json!({"session_ids": session_ids}))
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    pub async fn unsubscribe(&mut self, session_ids: Vec<String>) {
        let resp = self
            .request("sessions.unsubscribe", json!({"session_ids": session_ids}))
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    pub async fn send_chat(&mut self, session_id: &str, message: &str) {
        let resp = self
            .request("chat.send", json!({"session_id": session_id, "message": message}))
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    pub async fn get_history(&mut self, session_id: &str) -> Vec<serde_json::Value> {
        let resp = self
            .request("chat.history", json!({"session_id": session_id}))
            .await;
        assert!(
            resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
            "chat.history failed: {:?}",
            resp.get("error")
        );
        resp.get("payload")
            .unwrap()
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    pub async fn execute_command(&mut self, command: &str) -> serde_json::Value {
        self.request("commands.execute", json!({"command": command}))
            .await
    }
}

pub fn resp_payload(resp: &serde_json::Value) -> Option<&serde_json::Value> {
    resp.get("payload")
}

// ── Chat-Triggered Tool Test Helper
// ───────────────────────────────────────────

/// Helper: send a chat prompt and collect events, returning tool.result
/// payloads. The test passes as long as chat.final arrives within the timeout.
pub async fn run_tool_chat_test(
    port: u16,
    prompt: &str,
    expected_tool: &str,
) -> Vec<serde_json::Value> {
    if pick_test_provider().is_some() {
        start_test_gateway(port, true).await;
    } else {
        start_test_gateway_with_mock(port, tool_mock_provider(expected_tool)).await;
    }
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client.send_chat(&sid, prompt).await;

    let result = timeout(Duration::from_secs(120), async {
        let mut tool_called = false;
        let mut tool_results = Vec::new();
        let mut chat_final = None;

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    if p.get("tool_name").and_then(|v| v.as_str())
                                        == Some(expected_tool)
                                    {
                                        tool_called = true;
                                    }
                                }
                            }
                            Some("tool.result") => {
                                if let Some(p) = payload {
                                    tool_results.push(p);
                                }
                            }
                            Some("chat.final") => {
                                chat_final = payload;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (tool_called, tool_results, chat_final)
    })
    .await;

    let (tool_called, tool_results, chat_final) =
        result.expect("Timed out waiting for chat.final event");
    assert!(
        tool_called,
        "Expected {} tool to be invoked, but it was not called",
        expected_tool
    );
    assert!(chat_final.is_some(), "Expected chat.final event within 120s");
    assert!(!tool_results.is_empty(), "Expected at least one tool.result event");
    for result in &tool_results {
        assert_eq!(
            result.get("tool_name").and_then(|v| v.as_str()),
            Some(expected_tool),
            "Expected tool_name to be {}, got {:?}",
            expected_tool,
            result.get("tool_name")
        );
        assert!(result.get("result").is_some(), "Expected result field in tool.result");
    }
    tool_results
}

mod agent_tests;
#[cfg(feature = "browser")]
mod browser_chat_tests;
mod command_tests;
mod computer_tests;
mod goal_tests;
mod health_tests;
mod llm_chat_tests;
mod mock_chat_tests;
mod planner_tests;
mod screen_recorder_tests;
mod session_tests;
mod tool_chat_tests;
mod vision_tests;
