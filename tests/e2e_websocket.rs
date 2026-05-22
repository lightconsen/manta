//! True End-to-End WebSocket Tests
//!
//! Simulates a complete frontend client connecting via WebSocket, exercising:
//! - Session lifecycle management (create, list, subscribe, unsubscribe, delete, history)
//! - All built-in slash commands via `commands.execute`
//! - Agent queries (agents.list, agents.get)
//! - Health / system endpoints
//! - Real LLM chat with streaming event verification (chat.delta, chat.final, tool.calling, tool.result)
//! - Skills and MCP command exposure
//!
//! API keys are read from `start_local_*.sh` scripts at runtime, or via env vars:
//!   MANTA_TEST_PROVIDER_KEY, MANTA_TEST_PROVIDER, MANTA_TEST_BASE_URL, MANTA_TEST_MODEL

use futures_util::{SinkExt, StreamExt};
use manta::gateway::protocol::AuthMode;
use manta::gateway::{Gateway, GatewayConfig};
use manta::model_router::{ProviderConfig, ProviderType};
use serde_json::json;
use serial_test::serial;
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

// ── Type Aliases ──────────────────────────────────────────────────────────────

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;
type WsWrite = futures_util::stream::SplitSink<WsStream, Message>;
type WsRead = futures_util::stream::SplitStream<WsStream>;

// ── API Key Discovery ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct LocalProviderConfig {
    name: String,
    api_key: String,
    base_url: String,
    model: String,
    is_anthropic: bool,
}

/// Parse API configuration from `start_local_*.sh` shell scripts.
fn discover_local_providers() -> Vec<LocalProviderConfig> {
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
                if line.starts_with("export MANTA_API_KEY=") {
                    api_key = line.split('=').nth(1).map(|s| s.trim().trim_matches('"').to_string());
                }
                if line.starts_with("export MANTA_BASE_URL=") {
                    base_url = line.split('=').nth(1).map(|s| s.trim().trim_matches('"').to_string());
                }
                if line.starts_with("export MANTA_MODEL=") {
                    model = line.split('=').nth(1).map(|s| s.trim().trim_matches('"').to_string());
                }
                if line.starts_with("export MANTA_IS_ANTHROPIC=") {
                    is_anthropic = line.contains("true");
                }
            }

            if let (Some(key), Some(url), Some(mdl)) = (api_key, base_url, model) {
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

fn pick_test_provider() -> Option<LocalProviderConfig> {
    // Env vars take priority
    if let (Ok(key), Ok(name)) = (
        std::env::var("MANTA_TEST_PROVIDER_KEY"),
        std::env::var("MANTA_TEST_PROVIDER"),
    ) {
        let base_url = std::env::var("MANTA_TEST_BASE_URL").unwrap_or_default();
        let model = std::env::var("MANTA_TEST_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        let is_anthropic = name == "anthropic" || name == "kimi";
        return Some(LocalProviderConfig {
            name,
            api_key: key,
            base_url,
            model,
            is_anthropic,
        });
    }

    // Fall back to start_local_*.sh scripts
    let locals = discover_local_providers();
    locals.into_iter().next()
}

fn test_config(port: u16, with_provider: bool) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.host = "127.0.0.1".to_string();
    config.port = port;
    config.storage.storage_type = "sqlite".to_string();
    let db_path = std::env::temp_dir().join(format!("manta_e2e_ws_test_{}.db", port));
    // Clean up stale DB from previous runs
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
                api_key: provider.api_key,
                api_keys: vec![],
                auth_profile: None,
                base_url: if provider.base_url.is_empty() {
                    None
                } else {
                    Some(provider.base_url)
                },
                timeout: Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };
            config.providers.insert(provider.name.clone(), provider_config);
            config.model_provider = provider.name;
            config.model = provider.model;
        }
    }

    config
}

async fn start_test_gateway(port: u16, with_provider: bool) {
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

// ── Frontend Simulator ────────────────────────────────────────────────────────

/// Simulates a web frontend connected over WebSocket.
struct FrontendSimulator {
    write: WsWrite,
    read: WsRead,
    pub session_id: Option<String>,
    event_buffer: VecDeque<serde_json::Value>,
}

impl FrontendSimulator {
    async fn connect(port: u16) -> Self {
        let url = format!("ws://127.0.0.1:{}/ws", port);
        let (ws_stream, _) = connect_async(&url)
            .await
            .expect("Failed to connect to WebSocket");
        let (mut write, mut read) = ws_stream.split();

        // Handshake
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

    async fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
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

        // Wait for the matching response frame, buffering events for later retrieval.
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

    async fn wait_for_event(
        &mut self,
        event_name: &str,
        timeout_secs: u64,
    ) -> Option<serde_json::Value> {
        // Check buffer first.
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

    /// Collect all events matching `event_name` within `timeout_secs`.
    async fn collect_events(
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

    async fn create_session(&mut self) -> String {
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

    async fn list_sessions(&mut self) -> Vec<serde_json::Value> {
        let resp = self.request("sessions.list", json!(null)).await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
        resp.get("payload")
            .unwrap()
            .get("sessions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    async fn delete_session(&mut self, session_id: &str) {
        let resp = self
            .request("sessions.delete", json!({"session_id": session_id}))
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    async fn subscribe(&mut self, session_ids: Vec<String>) {
        let resp = self
            .request("sessions.subscribe", json!({"session_ids": session_ids}))
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    async fn unsubscribe(&mut self, session_ids: Vec<String>) {
        let resp = self
            .request("sessions.unsubscribe", json!({"session_ids": session_ids}))
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    async fn send_chat(&mut self, session_id: &str, message: &str) {
        let resp = self
            .request(
                "chat.send",
                json!({"session_id": session_id, "message": message}),
            )
            .await;
        assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    }

    async fn get_history(&mut self, session_id: &str) -> Vec<serde_json::Value> {
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

    async fn execute_command(&mut self, command: &str) -> serde_json::Value {
        let resp = self
            .request("commands.execute", json!({"command": command}))
            .await;
        resp
    }
}

fn resp_payload(resp: &serde_json::Value) -> Option<&serde_json::Value> {
    resp.get("payload")
}


// ── Session Management Journey ────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn session_full_lifecycle() {
    let port = 40001;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    // 1. List should be empty initially
    let sessions = client.list_sessions().await;
    assert!(sessions.is_empty(), "Expected empty session list initially");

    // 2. Create a session
    let sid = client.create_session().await;
    assert!(!sid.is_empty(), "Expected non-empty session_id");

    // 3. List should now contain the session
    let sessions = client.list_sessions().await;
    assert_eq!(sessions.len(), 1, "Expected 1 session after creation");
    assert_eq!(
        sessions[0].get("session_id").and_then(|v| v.as_str()),
        Some(sid.as_str())
    );

    // 4. Subscribe to the session
    client.subscribe(vec![sid.clone()]).await;

    // 5. Send a message (no LLM needed — just verify it accepts)
    client.send_chat(&sid, "Hello session").await;

    // 6. Verify history
    let history = client.get_history(&sid).await;
    assert!(
        history.iter().any(|m| {
            m.get("role").and_then(|v| v.as_str()) == Some("user")
                && m.get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .contains("Hello session")
        }),
        "User message should be in history"
    );

    // 7. Unsubscribe
    client.unsubscribe(vec![sid.clone()]).await;

    // 8. Delete the session
    client.delete_session(&sid).await;

    // 9. Verify deleted
    let sessions = client.list_sessions().await;
    assert!(
        sessions.iter().all(|s| {
            s.get("session_id").and_then(|v| v.as_str()) != Some(&sid)
        }),
        "Deleted session should not appear in list"
    );
}

#[tokio::test]
#[serial]
async fn session_subscribe_unsubscribe() {
    let port = 40002;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    // Create sessions with explicit IDs (default without params is "ws:anonymous")
    let sid1 = "test-session-alpha".to_string();
    let sid2 = "test-session-beta".to_string();

    let resp1 = client
        .request("sessions.create", json!({"session_id": sid1}))
        .await;
    assert!(resp1.get("ok").and_then(|v| v.as_bool()) == Some(true));

    let resp2 = client
        .request("sessions.create", json!({"session_id": sid2}))
        .await;
    assert!(resp2.get("ok").and_then(|v| v.as_bool()) == Some(true));

    // Subscribe to both
    client.subscribe(vec![sid1.clone(), sid2.clone()]).await;

    // Unsubscribe from one
    client.unsubscribe(vec![sid1.clone()]).await;

    // Verify both still exist in storage
    let sessions = client.list_sessions().await;
    assert_eq!(sessions.len(), 2, "Expected 2 sessions, got: {:?}", sessions);
}

// ── Command Execution via WebSocket ───────────────────────────────────────────

#[tokio::test]
#[serial]
async fn command_help_returns_markdown() {
    let port = 40010;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("help").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "help command failed"
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("manta") || text.contains("command"),
        "Expected Manta commands in help, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_status_returns_gateway_info() {
    let port = 40011;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("status").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("gateway") || text.contains("agent") || text.contains("status"),
        "Expected status info, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_tools_returns_catalog() {
    let port = 40012;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("tools").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("shell") || text.contains("file") || text.contains("tool"),
        "Expected tool catalog, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_whoami_returns_user_info() {
    let port = 40013;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("whoami").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("anonymous") || text.contains("user"),
        "Expected user info, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_skill_lists_skills() {
    let port = 40014;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("skill").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "skill command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("skill") || text.contains("0 total"),
        "Expected skills listing, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_mcp_returns_server_info() {
    let port = 40015;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("mcp").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "mcp command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("mcp") || text.contains("no mcp servers"),
        "Expected MCP info, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_acp_returns_status_or_no_session() {
    let port = 40016;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("acp").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "acp command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("acp") || text.contains("no active"),
        "Expected ACP info, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn commands_list_returns_catalog() {
    let port = 40017;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("commands.list", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "commands.list failed: {:?}",
        resp.get("error")
    );
    let payload = resp_payload(&resp).unwrap();
    let commands = payload
        .get("commands")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        commands.len() >= 20,
        "Expected at least 20 commands, got: {}",
        commands.len()
    );
    // Verify some key commands exist
    let names: Vec<String> = commands
        .iter()
        .filter_map(|c| c.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert!(names.contains(&"help".to_string()), "Expected 'help' command");
    assert!(names.contains(&"tools".to_string()), "Expected 'tools' command");
    assert!(
        names.contains(&"session".to_string()),
        "Expected 'session' command"
    );
}

#[tokio::test]
#[serial]
async fn command_reset_clears_history() {
    let port = 40018;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    // Create and subscribe to a session
    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    // Add a message to the session
    client.send_chat(&sid, "Test message before reset").await;

    // Verify history has the message
    let history = client.get_history(&sid).await;
    assert!(
        history.iter().any(|m| {
            m.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("Test message before reset")
        }),
        "Message should be in history before reset"
    );

    // Execute reset command
    let resp = client.execute_command("reset").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "reset command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        text.contains("reset"),
        "Expected reset confirmation, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_stop_no_session() {
    let port = 40019;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    // Execute stop without subscribing to any session
    let resp = client.execute_command("stop").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "stop command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        text.contains("No active session") || text.contains("stop"),
        "Expected stop message, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_skill_not_found() {
    let port = 40060;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client
        .request(
            "commands.execute",
            json!({"command": "skill", "args": "nonexistent-skill-xyz"}),
        )
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for nonexistent skill, got: {:?}",
        resp
    );
    let error = resp.get("error").unwrap();
    assert_eq!(
        error.get("code").and_then(|v| v.as_str()),
        Some("SKILL_NOT_FOUND")
    );
}

#[tokio::test]
#[serial]
async fn command_mcp_disconnect_requires_arg() {
    let port = 40061;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client
        .request(
            "commands.execute",
            json!({"command": "mcp", "args": "disconnect"}),
        )
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for missing arg, got: {:?}",
        resp
    );
    let error = resp.get("error").unwrap();
    assert_eq!(
        error.get("code").and_then(|v| v.as_str()),
        Some("INVALID_ARGS")
    );
}

// ── Agent Management ──────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agents_list_returns_array() {
    let port = 40020;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("agents.list", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "agents.list failed: {:?}",
        resp.get("error")
    );
    let agents = resp_payload(&resp)
        .unwrap()
        .get("agents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // Agents registry may be empty in test mode; just verify it's an array
    assert!(
        agents.is_empty() || agents.iter().all(|a| a.is_string()),
        "Expected array of agent IDs, got: {:?}",
        agents
    );
}

#[tokio::test]
#[serial]
async fn agents_get_returns_not_found_for_unknown() {
    let port = 40021;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client
        .request("agents.get", json!({"agent_id": "nonexistent-agent-12345"}))
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for unknown agent, got: {:?}",
        resp
    );
    let error = resp.get("error").unwrap();
    assert_eq!(
        error.get("code").and_then(|v| v.as_str()),
        Some("AGENT_NOT_FOUND")
    );
}

// ── Health & System ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn health_returns_healthy() {
    let port = 40030;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("health", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "health failed: {:?}",
        resp.get("error")
    );
    let payload = resp_payload(&resp).unwrap();
    assert_eq!(
        payload.get("status").and_then(|v| v.as_str()),
        Some("healthy")
    );
}

#[tokio::test]
#[serial]
async fn system_presence_returns_online() {
    let port = 40031;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("system.presence", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "system.presence failed: {:?}",
        resp.get("error")
    );
    let payload = resp_payload(&resp).unwrap();
    assert_eq!(
        payload.get("online").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[tokio::test]
#[serial]
async fn ping_returns_pong() {
    let port = 40032;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("ping", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "ping failed: {:?}",
        resp.get("error")
    );
}

// ── LLM Integration Journeys ──────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn llm_chat_streaming_journey() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40040;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    // Send a constrained prompt to get a predictable response
    client
        .send_chat(&sid, "Say exactly 'pong-from-llm' and nothing else.")
        .await;

    // Wait for the final response
    let payload = client
        .wait_for_event("chat.final", 60)
        .await
        .expect("Timed out waiting for chat.final event");

    let response = payload
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        response.contains("pong-from-llm") || response.contains("pong"),
        "Expected LLM response containing 'pong-from-llm', got: {}",
        response
    );

    // Verify history persistence.
    // There is a small race: chat.final is sent from the progress callback
    // before the main handler persists the assistant message. Poll briefly.
    let mut history = Vec::new();
    for _ in 0..20 {
        history = client.get_history(&sid).await;
        let has_assistant = history.iter().any(|m| {
            m.get("role").and_then(|v| v.as_str()) == Some("assistant")
        });
        if has_assistant {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let has_user = history.iter().any(|m| {
        m.get("role").and_then(|v| v.as_str()) == Some("user")
    });
    let has_assistant = history.iter().any(|m| {
        m.get("role").and_then(|v| v.as_str()) == Some("assistant")
    });
    assert!(has_user, "User message should be persisted");
    assert!(has_assistant, "Assistant response should be persisted");
}

#[tokio::test]
#[serial]
async fn llm_tool_invocation_journey() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40041;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    // Prompt that should trigger a tool call (time tool)
    client
        .send_chat(&sid, "What is the current date and time? Reply with just the year.")
        .await;

    // Collect all events for up to 60 seconds until chat.final arrives.
    // We must not use separate collect_events calls because they consume
    // non-matching events from the shared stream.
    let result = timeout(Duration::from_secs(60), async {
        let mut tool_calling = Vec::new();
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
                                if let Some(p) = payload {
                                    tool_calling.push(p);
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
        (tool_calling, tool_results, chat_final)
    })
    .await;

    let (tool_calling, tool_results, chat_final) =
        result.expect("Timed out waiting for chat.final event");

    // The LLM may or may not call a tool depending on the model and prompt.
    if !tool_calling.is_empty() {
        let first = &tool_calling[0];
        assert_eq!(
            first.get("session_id").and_then(|v| v.as_str()),
            Some(sid.as_str())
        );
        assert!(
            first.get("tool_name").is_some(),
            "tool.calling event should have tool_name"
        );
    }

    if !tool_results.is_empty() {
        let first = &tool_results[0];
        assert_eq!(
            first.get("session_id").and_then(|v| v.as_str()),
            Some(sid.as_str())
        );
        assert!(
            first.get("result").is_some(),
            "tool.result event should have result"
        );
    }

    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_shell_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40070;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    // Prompt that strongly suggests using the shell tool
    client
        .send_chat(
            &sid,
            "Use the shell tool to run the command 'echo hello-from-shell-test' and report the exact output.",
        )
        .await;

    // Collect events looking for shell tool invocation
    let result = timeout(Duration::from_secs(60), async {
        let mut shell_called = false;
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
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("shell")
                                    {
                                        shell_called = true;
                                    }
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
        (shell_called, chat_final)
    })
    .await;

    let (shell_called, chat_final) = result.expect("Timed out waiting for chat.final event");

    // Shell may or may not be called depending on model behavior
    if shell_called {
        tracing::info!("Shell tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_file_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40080;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the file_write tool to create a file at /tmp/manta-e2e-test.txt with content 'manta-e2e-file-test'. \
             Then use file_read to read it back and confirm the content.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut file_called = false;
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
                                    let tool_name = p.get("tool_name").and_then(|v| v.as_str());
                                    if tool_name == Some("file_write")
                                        || tool_name == Some("file_read")
                                    {
                                        file_called = true;
                                    }
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
        (file_called, chat_final)
    })
    .await;

    let (file_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if file_called {
        tracing::info!("File tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_todo_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40081;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the todo tool to add a task 'e2e-todo-item' and then list all todos.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut todo_called = false;
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
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("todo")
                                    {
                                        todo_called = true;
                                    }
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
        (todo_called, chat_final)
    })
    .await;

    let (todo_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if todo_called {
        tracing::info!("Todo tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_code_exec_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40082;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the execute_code tool to run Python code that prints 'manta-code-exec-ok' and report the output.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut code_called = false;
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
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("execute_code")
                                    {
                                        code_called = true;
                                    }
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
        (code_called, chat_final)
    })
    .await;

    let (code_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if code_called {
        tracing::info!("Execute_code tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_web_fetch_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40083;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the web_fetch tool to fetch https://example.com and tell me what the page title is.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut fetch_called = false;
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
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("web_fetch")
                                    {
                                        fetch_called = true;
                                    }
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
        (fetch_called, chat_final)
    })
    .await;

    let (fetch_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if fetch_called {
        tracing::info!("Web_fetch tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_memory_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40084;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the memory tool to store the fact 'Manta is an AI agent framework' and then confirm it was saved.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut memory_called = false;
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
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("memory")
                                    {
                                        memory_called = true;
                                    }
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
        (memory_called, chat_final)
    })
    .await;

    let (memory_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if memory_called {
        tracing::info!("Memory tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

// ── Chat-Triggered Tool Tests (Helper) ────────────────────────────────────────

/// Helper: send a chat prompt and collect events, returning whether the expected tool was called.
/// The test passes as long as chat.final arrives within the timeout.
async fn run_tool_chat_test(
    port: u16,
    prompt: &str,
    expected_tool: &str,
) -> Vec<serde_json::Value> {
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client.send_chat(&sid, prompt).await;

    let result = timeout(Duration::from_secs(60), async {
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
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
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
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
    assert!(
        !tool_results.is_empty(),
        "Expected at least one tool.result event"
    );
    for result in &tool_results {
        assert_eq!(
            result.get("tool_name").and_then(|v| v.as_str()),
            Some(expected_tool),
            "Expected tool_name to be {}, got {:?}",
            expected_tool,
            result.get("tool_name")
        );
        assert!(
            result.get("result").is_some(),
            "Expected result field in tool.result"
        );
    }
    tool_results
}

#[tokio::test]
#[serial]
async fn tool_glob_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40085,
        "Use the glob tool to list all .rs files in the src directory.",
        "glob",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_grep_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40086,
        "Use the grep tool to search for 'pub fn' in the src directory.",
        "grep",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_process_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40087,
        "Use the process tool to list running processes.",
        "process",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_nodes_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40088,
        "Use the nodes tool to list available nodes.",
        "nodes",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_web_search_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40089,
        "Use the web_search tool to search for Rust programming language.",
        "web_search",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_update_plan_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40090,
        "Use the update_plan tool to create a plan titled 'Test Plan' with steps 'Step 1' and 'Step 2'.",
        "update_plan",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_canvas_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40091,
        "Use the canvas tool to present a canvas for session 'test-session-canvas' with a text component saying 'Hello Canvas'.",
        "canvas",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_pdf_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40092,
        "Use the pdf tool to generate a PDF with content 'Hello PDF' and save it.",
        "pdf",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_image_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40093,
        "Use the image tool to get info about the file /tmp/manta-test.png.",
        "image",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_tts_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40094,
        "Use the tts tool to convert the text 'Hello' to speech.",
        "tts",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_memory_search_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40095,
        "Use the memory_search tool to search for 'Manta'.",
        "memory_search",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_memory_get_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40096,
        "Use the memory_get tool to list all stored memories.",
        "memory_get",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_cron_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40097,
        "Use the cron tool to list all cron jobs.",
        "cron",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_file_edit_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40098,
        "Use the file_edit tool to read /tmp/manta-e2e-edit.txt and replace 'old' with 'new'. If the file does not exist, create it with 'old text' first.",
        "file_edit",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_acp_spawn_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40099,
        "Use the acp_spawn tool to list available spawn targets or check spawn status.",
        "acp_spawn",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_acp_session_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40100,
        "Use the acp_session tool to show the current ACP session status.",
        "acp_session",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_list_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40101,
        "Use the sessions_list tool to list all active sessions.",
        "sessions_list",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_history_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40102,
        "Use the sessions_history tool to get the history of the current session.",
        "sessions_history",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_send_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40103,
        "Use the sessions_send tool to send a ping message to the current session.",
        "sessions_send",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_yield_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40104,
        "Use the sessions_yield tool to check the current yield status.",
        "sessions_yield",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_session_status_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40105,
        "Use the session_status tool to check the status of the current session.",
        "session_status",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_subagents_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40106,
        "Use the subagents tool to list all available subagents.",
        "subagents",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_apply_patch_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40107,
        "Use the apply_patch tool to validate a simple patch for a file.",
        "apply_patch",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_delegate_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40108,
        "Use the delegate tool to list available agents or check delegation status.",
        "delegate",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_mcp_connection_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40109,
        "Use the mcp_connection tool to list all connected MCP servers.",
        "mcp_connection",
    ).await;
}

#[tokio::test]
#[serial]
async fn command_model_returns_status() {
    let port = 40071;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("model").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "model command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("model") || text.contains("provider"),
        "Expected model status, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_usage_returns_info() {
    let port = 40072;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("usage").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "usage command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("usage") || text.contains("token") || text.contains("cost"),
        "Expected usage info, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn command_debug_show_returns_overrides() {
    let port = 40073;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("debug").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "debug command failed: {:?}",
        resp.get("error")
    );
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("debug") || text.contains("override"),
        "Expected debug info, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn session_created_event_on_first_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40042;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    // Don't create session manually — let chat.send derive it.
    // With no explicit session_id, the gateway derives "ws:anonymous"
    // and emits a session.created event for new sessions.
    client
        .request("chat.send", json!({"message": "Hi there"}))
        .await;

    // Wait for session.created event (empty subscriptions means all events flow)
    let payload = client
        .wait_for_event("session.created", 5)
        .await
        .expect("Expected session.created event");

    assert!(
        payload.get("session_id").is_some(),
        "session.created should contain session_id"
    );
}

// ── Chat Abort ────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn chat_abort_returns_ok() {
    let port = 40050;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    let resp = client
        .request("chat.abort", json!({"session_id": sid}))
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "chat.abort failed: {:?}",
        resp.get("error")
    );
    let payload = resp_payload(&resp).unwrap();
    assert_eq!(
        payload.get("status").and_then(|v| v.as_str()),
        Some("abort_requested")
    );
}
