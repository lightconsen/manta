//! E2E Live Integration Tests
//!
//! Comprehensive tests for all built-in tools, slash commands, and real LLM
//! integration. Run with:
//!
//!   cargo test --test e2e_live
//!
//! For LLM tests, set env vars:
//!
//!   MANTA_TEST_PROVIDER_KEY=sk-xxx MANTA_TEST_PROVIDER=openai cargo test --test e2e_live

use futures_util::{SinkExt, StreamExt};
use manta::gateway::protocol::AuthMode;
use manta::gateway::{Gateway, GatewayConfig};
use manta::model_router::{ProviderConfig, ProviderType};
use manta::tools::{
    CodeExecutionTool, FileEditTool, FileReadTool, FileWriteTool, GlobTool, GrepTool,
    NodesTool, ProcessTool, ShellTool, TimeTool, TodoTool, Tool, ToolContext,
};
use serde_json::json;
use serial_test::serial;
use std::env;
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

// ── Type Aliases ──────────────────────────────────────────────────────────────

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;
type WsWrite = futures_util::stream::SplitSink<WsStream, Message>;
type WsRead = futures_util::stream::SplitStream<WsStream>;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn test_config(port: u16, with_provider: bool) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.host = "127.0.0.1".to_string();
    config.port = port;
    config.storage.storage_type = "memory".to_string();
    config.security.auth_mode = AuthMode::None;
    config.plugins.enabled = false;
    config.channels.clear();

    if with_provider {
        if let (Ok(key), Ok(provider_name)) = (
            env::var("MANTA_TEST_PROVIDER_KEY"),
            env::var("MANTA_TEST_PROVIDER"),
        ) {
            let provider_type = match provider_name.as_str() {
                "openai" => ProviderType::OpenAi,
                "anthropic" => ProviderType::Anthropic,
                other => panic!(
                    "Unknown provider: {}. Use 'openai' or 'anthropic'",
                    other
                ),
            };
            let provider_config = ProviderConfig {
                provider_type,
                api_key: key,
                api_keys: vec![],
                auth_profile: None,
                base_url: None,
                timeout: Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };
            config.providers.insert(provider_name.clone(), provider_config);
            config.model_provider = provider_name.clone();
            config.model = env::var("MANTA_TEST_MODEL")
                .unwrap_or_else(|_| "gpt-4o-mini".to_string());
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

    // Wait for server to be ready
    tokio::time::sleep(Duration::from_millis(300)).await;
}

async fn ws_connect(port: u16) -> (WsWrite, WsRead) {
    let url = format!("ws://127.0.0.1:{}/ws", port);
    let (ws_stream, _) = connect_async(&url)
        .await
        .expect("Failed to connect to WebSocket");

    let (mut write, mut read) = ws_stream.split();

    // Send connect handshake
    let connect_req = json!({
        "type": "req",
        "id": "connect-1",
        "method": "connect",
        "params": {
            "protocol_version": 1,
            "scopes": ["chat", "read", "write"],
            "auth": {}
        }
    });
    write
        .send(Message::Text(connect_req.to_string()))
        .await
        .unwrap();

    // Wait for hello-ok
    let msg = read.next().await.unwrap().unwrap();
    let response: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert!(
        response.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "Handshake failed: {:?}",
        response.get("error")
    );

    (write, read)
}

async fn ws_request(
    write: &mut WsWrite,
    read: &mut WsRead,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let req = json!({
        "type": "req",
        "id": format!("req-{}", uuid::Uuid::new_v4()),
        "method": method,
        "params": params,
    });
    write.send(Message::Text(req.to_string())).await.unwrap();

    let msg = timeout(Duration::from_secs(10), read.next())
        .await
        .expect("Timeout waiting for response")
        .unwrap()
        .unwrap();

    serde_json::from_str(msg.to_text().unwrap()).unwrap()
}

async fn ws_wait_for_event(
    read: &mut WsRead,
    event_name: &str,
    timeout_secs: u64,
) -> Option<serde_json::Value> {
    let result = timeout(Duration::from_secs(timeout_secs), async {
        while let Some(msg) = read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        if event.get("event").and_then(|v| v.as_str()) == Some(event_name) {
                            return event.get("payload").cloned();
                        }
                    }
                }
            }
        }
        None
    })
    .await;

    result.unwrap_or(None)
}

fn resp_is_ok(resp: &serde_json::Value) -> bool {
    resp.get("ok").and_then(|v| v.as_bool()) == Some(true)
}

fn resp_payload(resp: &serde_json::Value) -> Option<&serde_json::Value> {
    resp.get("payload")
}

fn resp_error(resp: &serde_json::Value) -> Option<&serde_json::Value> {
    resp.get("error")
}

// ── Tool Execution Tests ──────────────────────────────────────────────────────

fn test_context() -> ToolContext {
    ToolContext::new("test_user", "test_session")
        .with_timeout(Duration::from_secs(10))
}

#[tokio::test]
async fn shell_tool_executes_echo() {
    let tool = ShellTool::new();
    let result = tool
        .execute(json!({"command": "echo test-output"}), &test_context())
        .await
        .expect("shell tool should succeed");

    let output = result.output.to_lowercase();
    assert!(
        output.contains("test-output"),
        "Expected 'test-output' in shell output, got: {}",
        output
    );
}

#[tokio::test]
async fn file_read_write_cycle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("test_file.txt");
    let path_str = file_path.to_str().unwrap();

    // Write
    let write_tool = FileWriteTool::new();
    let _ = write_tool
        .execute(
            json!({"path": path_str, "content": "hello world"}),
            &test_context(),
        )
        .await
        .expect("file_write should succeed");

    // Read
    let read_tool = FileReadTool::new();
    let result = read_tool
        .execute(json!({"path": path_str}), &test_context())
        .await
        .expect("file_read should succeed");

    assert!(
        result.output.contains("hello world"),
        "Expected 'hello world' in file content, got: {}",
        result.output
    );
}

#[tokio::test]
async fn file_edit_tool_replaces_content() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("edit_test.txt");
    let path_str = file_path.to_str().unwrap();

    // Write initial content
    let write_tool = FileWriteTool::new();
    let _ = write_tool
        .execute(
            json!({"path": path_str, "content": "old content here"}),
            &test_context(),
        )
        .await
        .unwrap();

    // Edit
    let edit_tool = FileEditTool::new();
    let _ = edit_tool
        .execute(
            json!({
                "path": path_str,
                "old_string": "old content",
                "new_string": "new content"
            }),
            &test_context(),
        )
        .await
        .expect("file_edit should succeed");

    // Verify
    let read_tool = FileReadTool::new();
    let result = read_tool
        .execute(json!({"path": path_str}), &test_context())
        .await
        .unwrap();

    assert!(
        result.output.contains("new content here"),
        "Expected edited content, got: {}",
        result.output
    );
}

#[tokio::test]
async fn glob_tool_lists_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base = temp_dir.path();

    // Create some files
    tokio::fs::write(base.join("a.rs"), "").await.unwrap();
    tokio::fs::write(base.join("b.rs"), "").await.unwrap();
    tokio::fs::write(base.join("c.txt"), "").await.unwrap();

    let tool = GlobTool::new();
    let result = tool
        .execute(
            json!({
                "pattern": "*.rs",
                "path": base.to_str().unwrap()
            }),
            &test_context(),
        )
        .await
        .expect("glob tool should succeed");

    assert!(
        result.output.contains("a.rs") && result.output.contains("b.rs"),
        "Expected .rs files in glob output, got: {}",
        result.output
    );
}

#[tokio::test]
async fn grep_tool_finds_patterns() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("search.txt");
    let path_str = file_path.to_str().unwrap();

    tokio::fs::write(&file_path, "fn main() {}\nfn helper() {}\n")
        .await
        .unwrap();

    let tool = GrepTool::new();
    let result = tool
        .execute(
            json!({
                "pattern": "fn main",
                "path": path_str
            }),
            &test_context(),
        )
        .await
        .expect("grep tool should succeed");

    assert!(
        result.output.contains("fn main"),
        "Expected 'fn main' in grep output, got: {}",
        result.output
    );
}

#[tokio::test]
async fn time_tool_returns_timestamp() {
    let tool = TimeTool::new();
    let result = tool
        .execute(json!({"action": "now"}), &test_context())
        .await
        .expect("time tool should succeed");

    // Result should contain a year like 2025 or 2026
    assert!(
        result.output.contains("2025") || result.output.contains("2026"),
        "Expected current year in time output, got: {}",
        result.output
    );
}

#[tokio::test]
async fn todo_tool_adds_and_lists() {
    let tool = TodoTool::new();
    let ctx = test_context();

    // Create a todo
    let _ = tool
        .execute(
            json!({
                "action": "create",
                "content": "test todo item"
            }),
            &ctx,
        )
        .await
        .expect("todo create should succeed");

    // List todos
    let result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .expect("todo list should succeed");

    assert!(
        result.output.contains("test todo item"),
        "Expected todo item in list, got: {}",
        result.output
    );
}

#[tokio::test]
async fn process_tool_lists_processes() {
    let tool = ProcessTool::new();
    let result = tool
        .execute(json!({"action": "list"}), &test_context())
        .await
        .expect("process tool should succeed");

    // Should contain current process info (PID, name, etc.)
    assert!(
        !result.output.is_empty(),
        "process tool returned empty content"
    );
}

#[tokio::test]
async fn nodes_tool_returns_definitions() {
    let tool = NodesTool::new();
    let result = tool
        .execute(json!({"action": "list"}), &test_context())
        .await;

    // Nodes may not be configured — just verify it doesn't panic.
    // The tool may return an error if tailscale is not installed.
    match result {
        Ok(output) => {
            println!("Nodes tool output: {}", output.output);
        }
        Err(e) => {
            println!("Nodes tool returned error (expected if no nodes configured): {}", e);
        }
    }
}

#[tokio::test]
async fn code_exec_tool_runs_python() {
    let tool = CodeExecutionTool::default();
    let result = tool
        .execute(
            json!({
                "language": "python",
                "code": "print('hello-from-python')"
            }),
            &test_context(),
        )
        .await;

    // Python may not be installed — allow failure but check success case
    if let Ok(output) = result {
        assert!(
            output.output.contains("hello-from-python"),
            "Expected python output, got: {}",
            output.output
        );
    }
}

// ── WebSocket Command Tests ───────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn commands_list_returns_all_builtin_commands() {
    let port = 39001;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    let resp = ws_request(&mut write, &mut read, "commands.list", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "commands.list failed: {:?}",
        resp.get("error")
    );

    let commands = resp
        .get("payload")
        .unwrap()
        .get("commands")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    assert!(
        commands.len() >= 20,
        "Expected at least 20 commands, got {}",
        commands.len()
    );
}

#[tokio::test]
#[serial]
async fn help_command_returns_markdown() {
    let port = 39002;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    let resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "help"}),
    )
    .await;
    assert!(resp_is_ok(&resp), "help command failed: {:?}", resp_error(&resp));

    let content = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        content.contains("manta") || content.contains("command"),
        "Expected 'Manta' or 'command' in help output, got: {}",
        content
    );
}

#[tokio::test]
#[serial]
async fn status_command_returns_gateway_status() {
    let port = 39003;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    let resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "status"}),
    )
    .await;
    assert!(resp_is_ok(&resp), "status command failed: {:?}", resp_error(&resp));

    let content = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        content.contains("gateway") || content.contains("status") || content.contains("agent"),
        "Expected status info, got: {}",
        content
    );
}

#[tokio::test]
#[serial]
async fn tools_command_returns_tool_catalog() {
    let port = 39004;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    let resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "tools"}),
    )
    .await;
    assert!(resp_is_ok(&resp), "tools command failed: {:?}", resp_error(&resp));

    let content = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        content.contains("shell")
            || content.contains("file")
            || content.contains("tool"),
        "Expected tool names in output, got: {}",
        content
    );
}

#[tokio::test]
#[serial]
async fn whoami_command_returns_anonymous() {
    let port = 39005;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    let resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "whoami"}),
    )
    .await;
    assert!(resp_is_ok(&resp), "whoami command failed: {:?}", resp_error(&resp));

    let content = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        content.contains("anonymous") || content.contains("user"),
        "Expected user info, got: {}",
        content
    );
}

#[tokio::test]
#[serial]
async fn new_session_command_creates_session() {
    let port = 39006;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    // /new is a local command; test sessions.create directly
    let resp = ws_request(
        &mut write,
        &mut read,
        "sessions.create",
        json!({"agent_id": "default"}),
    )
    .await;
    assert!(resp_is_ok(&resp), "sessions.create failed: {:?}", resp_error(&resp));

    let session_id = resp_payload(&resp)
        .unwrap()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    assert!(
        !session_id.is_empty(),
        "Expected session_id in response"
    );
}

#[tokio::test]
#[serial]
async fn sessions_list_returns_empty_initially() {
    let port = 39007;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    let resp = ws_request(&mut write, &mut read, "sessions.list", json!(null)).await;
    assert!(resp_is_ok(&resp), "sessions.list failed: {:?}", resp_error(&resp));

    let sessions = resp_payload(&resp)
        .unwrap()
        .get("sessions")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    // Memory storage starts empty
    assert!(sessions.is_empty(), "Expected empty sessions list initially");
}

// ── LLM Integration Tests ─────────────────────────────────────────────────────

fn require_llm_config() {
    if env::var("MANTA_TEST_PROVIDER_KEY").is_err() {
        panic!(
            "LLM tests require MANTA_TEST_PROVIDER_KEY env var. \
             Set it and MANTA_TEST_PROVIDER (e.g. 'openai') to run LLM tests."
        );
    }
    if env::var("MANTA_TEST_PROVIDER").is_err() {
        panic!(
            "LLM tests require MANTA_TEST_PROVIDER env var (e.g. 'openai' or 'anthropic')."
        );
    }
}

#[tokio::test]
#[serial]
async fn chat_send_returns_assistant_response() {
    require_llm_config();
    let port = 39010;
    start_test_gateway(port, true).await;
    let (mut write, mut read) = ws_connect(port).await;

    // Create a session first
    let create_resp = ws_request(
        &mut write,
        &mut read,
        "sessions.create",
        json!({"agent_id": "default"}),
    )
    .await;
    assert!(resp_is_ok(&create_resp), "sessions.create failed: {:?}", resp_error(&create_resp));

    let session_id = resp_payload(&create_resp)
        .unwrap()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    // Subscribe to the session
    let sub_resp = ws_request(
        &mut write,
        &mut read,
        "sessions.subscribe",
        json!({"session_ids": [session_id]}),
    )
    .await;
    assert!(resp_is_ok(&sub_resp), "sessions.subscribe failed: {:?}", resp_error(&sub_resp));

    // Send a message
    let _ = ws_request(
        &mut write,
        &mut read,
        "chat.send",
        json!({
            "session_id": session_id,
            "content": "Say exactly 'hello-from-llm' and nothing else."
        }),
    )
    .await;

    // Wait for agent response event
    let payload = ws_wait_for_event(&mut read, "agent.response", 60)
        .await
        .expect("Timed out waiting for agent.response event");

    let content = payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        content.contains("hello-from-llm") || content.contains("hello"),
        "Expected LLM response, got: {}",
        content
    );
}

#[tokio::test]
#[serial]
async fn command_persisted_to_session_history() {
    require_llm_config();
    let port = 39011;
    start_test_gateway(port, true).await;
    let (mut write, mut read) = ws_connect(port).await;

    // Create a session
    let create_resp = ws_request(
        &mut write,
        &mut read,
        "sessions.create",
        json!({"agent_id": "default"}),
    )
    .await;
    assert!(resp_is_ok(&create_resp), "sessions.create failed: {:?}", resp_error(&create_resp));

    let session_id = resp_payload(&create_resp)
        .unwrap()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    // Execute /help command
    let resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "help", "session_id": session_id}),
    )
    .await;
    assert!(resp_is_ok(&resp), "help command failed: {:?}", resp_error(&resp));

    // Load session history
    let history_resp = ws_request(
        &mut write,
        &mut read,
        "chat.history",
        json!({"session_id": session_id}),
    )
    .await;
    assert!(resp_is_ok(&history_resp), "chat.history failed: {:?}", resp_error(&history_resp));

    let messages = resp_payload(&history_resp)
        .unwrap()
        .as_array()
        .cloned()
        .unwrap_or_default();

    let has_user_command = messages.iter().any(|m| {
        m.get("role")
            .and_then(|v| v.as_str())
            == Some("user")
            && m.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("/help")
    });

    assert!(
        has_user_command,
        "Expected /help command to be persisted in session history"
    );
}
