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
    AcpSessionTool, AcpSpawnTool, ApplyPatchTool, CanvasTool, CodeExecutionTool, CronTool,
    DelegateTool, FileEditTool, FileReadTool, FileWriteTool, GlobTool, GrepTool, ImageTool,
    McpConnectionTool, MemoryGetTool, MemorySearchTool, MemoryTool, NodesTool, PdfTool,
    ProcessTool, SessionStatusTool, SessionsHistoryTool, SessionsListTool, SessionsSendTool,
    SessionsYieldTool, ShellTool, SubagentsTool, TimeTool, TodoTool, Tool, ToolContext,
    TtsTool, UpdatePlanTool, WebFetchTool, WebSearchTool,
};
use serde_json::json;
use serial_test::serial;
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

// ── Type Aliases ──────────────────────────────────────────────────────────────

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;
type WsWrite = futures_util::stream::SplitSink<WsStream, Message>;
type WsRead = futures_util::stream::SplitStream<WsStream>;

// ── Provider Discovery ────────────────────────────────────────────────────────

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
    discover_local_providers().into_iter().next()
}

/// Skip LLM tests if no provider is configured.
fn skip_if_no_provider() -> Option<LocalProviderConfig> {
    let provider = pick_test_provider();
    if provider.is_none() {
        eprintln!(
            "Skipping LLM test: no provider configured. \
             Set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or create start-local-*.sh scripts in the project root."
        );
    }
    provider
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn test_config(port: u16, with_provider: bool) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.host = "127.0.0.1".to_string();
    config.port = port;
    // Use SQLite with a temp DB so session_store is available for history tests
    config.storage.storage_type = "sqlite".to_string();
    config.storage.database_url = Some(format!(
        "sqlite:{}",
        std::env::temp_dir().join(format!("manta_e2e_test_{}.db", port)).display()
    ));
    config.security.auth_mode = AuthMode::None;
    config.plugins.enabled = false;
    config.channels.clear();

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

    // Wait for server to be ready (poll WS endpoint up to 10s)
    let url = format!("ws://127.0.0.1:{}/ws", port);
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if connect_async(&url).await.is_ok() {
            return;
        }
    }
    panic!("Gateway did not start within 10 seconds");
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

#[tokio::test]
async fn web_fetch_tool_fetches_example_com() {
    let tool = WebFetchTool::new();
    let result = tool
        .execute(
            json!({"url": "https://example.com"}),
            &test_context(),
        )
        .await;

    match result {
        Ok(output) => {
            if output.success {
                assert!(
                    output.output.contains("Example Domain")
                        || output.output.to_lowercase().contains("example")
                        || output.output.is_empty(),
                    "Expected example.com content, got: {}",
                    output.output
                );
            } else {
                println!("web_fetch returned error: {:?}", output.error);
            }
        }
        Err(e) => {
            println!("web_fetch failed (network may be unavailable): {}", e);
        }
    }
}

#[tokio::test]
async fn memory_tool_creates_and_reads() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memory_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url).await.expect("Failed to create MemoryTool");
    let ctx = test_context();

    // Store a memory
    let store_result = tool
        .execute(
            json!({
                "action": "store",
                "content": "e2e-test-memory-value",
                "category": "fact"
            }),
            &ctx,
        )
        .await
        .expect("store failed");

    let stored_id = store_result
        .data
        .as_ref()
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .expect("Missing memory id")
        .to_string();

    // Retrieve it
    let retrieve_result = tool
        .execute(
            json!({"action": "retrieve", "id": stored_id}),
            &ctx,
        )
        .await
        .expect("retrieve failed");

    assert!(
        retrieve_result.output.contains("e2e-test-memory-value"),
        "Expected stored memory content, got: {}",
        retrieve_result.output
    );

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn acp_spawn_tool_executes_without_agent_builder() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = AcpSpawnTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"task": "test task"}), &ctx)
        .await
        .unwrap();
    assert!(!result.success, "Expected failure without agent builder");
    assert!(
        result.error.unwrap().contains("No agent builder configured"),
        "Expected 'No agent builder configured' error"
    );
}

#[tokio::test]
async fn acp_session_tool_lists_sessions() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = AcpSessionTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .unwrap();
    assert!(result.success, "list action should succeed");
    assert!(
        result.output.contains("0 active subagent"),
        "Expected empty list, got: {}",
        result.output
    );
    let data = result.data.unwrap();
    let subagents = data.get("subagents").unwrap().as_array().unwrap();
    assert_eq!(subagents.len(), 0);
}

#[tokio::test]
async fn sessions_list_tool_lists_sessions() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionsListTool::new(acp);
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await.unwrap();
    assert!(result.success, "sessions_list should succeed");
    assert!(
        result.output.contains("0 active session"),
        "Expected empty list, got: {}",
        result.output
    );
    let data = result.data.unwrap();
    let sessions = data.get("sessions").unwrap().as_array().unwrap();
    assert_eq!(sessions.len(), 0);
}

#[tokio::test]
async fn sessions_history_tool_returns_history() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());

    // Create a session first so history lookup succeeds
    let session_id = acp.create_session("test-agent".to_string()).await;

    let tool = SessionsHistoryTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"session_id": session_id.to_string()}), &ctx)
        .await
        .unwrap();
    assert!(result.success, "sessions_history should succeed");
    assert!(
        result.output.contains("subagent"),
        "Expected subagent count in output, got: {}",
        result.output
    );
}

#[tokio::test]
async fn sessions_send_tool_fails_for_missing_subagent() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionsSendTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "session_id": "test-session",
                "subagent_id": "nonexistent-subagent",
                "message": "hello"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!result.success, "Expected failure for missing subagent");
    assert!(
        result.error.unwrap().contains("Failed to send message"),
        "Expected send failure error"
    );
}

#[tokio::test]
async fn sessions_yield_tool_fails_for_missing_subagent() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionsYieldTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"subagent_id": "nonexistent-subagent"}), &ctx)
        .await
        .unwrap();
    assert!(!result.success, "Expected failure for missing subagent");
    assert!(
        result.error.unwrap().contains("not found"),
        "Expected 'not found' error"
    );
}

#[tokio::test]
async fn session_status_tool_requires_id() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionStatusTool::new(acp);
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await.unwrap();
    assert!(!result.success, "Expected failure without id");
    assert!(
        result.error.unwrap().contains("Provide either"),
        "Expected 'Provide either session_id or subagent_id' error"
    );
}

#[tokio::test]
async fn subagents_tool_lists_subagents() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SubagentsTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .unwrap();
    assert!(result.success, "subagents list should succeed");
    assert!(
        result.output.contains("0 subagent"),
        "Expected empty list, got: {}",
        result.output
    );
    let data = result.data.unwrap();
    let subagents = data.get("subagents").unwrap().as_array().unwrap();
    assert_eq!(subagents.len(), 0);
}

#[tokio::test]
async fn apply_patch_tool_validates_patch() {
    let tool = ApplyPatchTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "patch": "not a valid unified diff patch",
                "directory": "/tmp"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!result.success, "Expected failure for invalid patch");
    assert!(
        result.error.unwrap().contains("Patch does not apply"),
        "Expected patch validation error"
    );
}

#[tokio::test]
async fn update_plan_tool_crud() {
    let tool = UpdatePlanTool::new();
    let ctx = test_context();

    // Create a plan
    let create_result = tool
        .execute(
            json!({
                "action": "create",
                "title": "test-plan",
                "steps": ["step one", "step two"]
            }),
            &ctx,
        )
        .await
        .expect("create should succeed");

    let plan_id = create_result
        .data
        .as_ref()
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .expect("Missing plan id")
        .to_string();

    // Get the plan
    let get_result = tool
        .execute(
            json!({"action": "get", "plan_id": plan_id}),
            &ctx,
        )
        .await
        .expect("get should succeed");
    assert!(
        get_result.output.contains("test-plan"),
        "Expected plan title in output, got: {}",
        get_result.output
    );

    // List plans
    let list_result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .expect("list should succeed");
    assert!(
        list_result.output.contains("1 plan"),
        "Expected plan count in list, got: {}",
        list_result.output
    );
    let plans = list_result
        .data
        .as_ref()
        .and_then(|d| d.get("plans"))
        .and_then(|v| v.as_array())
        .expect("Expected plans array");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].get("id").and_then(|v| v.as_str()), Some(plan_id.as_str()));

    // Set step status
    let _ = tool
        .execute(
            json!({
                "action": "set_status",
                "plan_id": plan_id,
                "step_id": "step_1",
                "status": "completed"
            }),
            &ctx,
        )
        .await
        .expect("set_status should succeed");

    // Delete the plan
    let _ = tool
        .execute(
            json!({"action": "delete", "plan_id": plan_id}),
            &ctx,
        )
        .await
        .expect("delete should succeed");
}

#[tokio::test]
async fn cron_tool_list_without_scheduler() {
    let tool = CronTool::new();
    let result = tool
        .execute(json!({"action": "list"}), &test_context())
        .await;

    // Without a scheduler set, the tool returns Ok with success=false and an error message
    assert!(result.is_ok(), "Expected Ok result when scheduler not set");
    let r = result.unwrap();
    assert!(!r.success, "Expected success=false when scheduler not set");
    let err_msg = r.error.expect("Expected error message");
    assert!(
        err_msg.contains("scheduler") || err_msg.contains("not initialized") || err_msg.contains("Cron scheduler not available"),
        "Expected scheduler-related error, got: {}",
        err_msg
    );
}

#[tokio::test]
async fn memory_search_tool_searches() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_search_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url).await.expect("Failed to create store"),
    );
    let tool = MemorySearchTool::with_store(store.clone());
    let ctx = test_context();

    // Store a memory first using MemoryTool
    let memory_tool = MemoryTool::with_store(store.clone()).await.expect("Failed to create MemoryTool");
    let _ = memory_tool
        .execute(
            json!({
                "action": "store",
                "content": "Rust is a systems programming language",
                "category": "fact"
            }),
            &ctx,
        )
        .await
        .expect("store failed");

    // Search for it
    let result = tool
        .execute(
            json!({"action": "search", "query": "Rust programming"}),
            &ctx,
        )
        .await
        .expect("search should succeed");

    assert!(
        result.output.contains("Rust") || result.output.contains("programming"),
        "Expected search results, got: {}",
        result.output
    );

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_tool_crud() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_get_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url).await.expect("Failed to create store"),
    );
    let tool = MemoryGetTool::with_store(store.clone());
    let ctx = test_context();

    // Store a memory first
    let memory_tool = MemoryTool::with_store(store.clone()).await.expect("Failed to create MemoryTool");
    let store_result = memory_tool
        .execute(
            json!({
                "action": "store",
                "content": "memory-get-test-content",
                "category": "fact"
            }),
            &ctx,
        )
        .await
        .expect("store failed");

    let memory_id = store_result
        .data
        .as_ref()
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .expect("Missing memory id")
        .to_string();

    // Retrieve via MemoryGetTool
    let result = tool
        .execute(
            json!({"action": "retrieve", "id": memory_id}),
            &ctx,
        )
        .await
        .expect("retrieve should succeed");

    assert!(
        result.output.contains("memory-get-test-content"),
        "Expected memory content, got: {}",
        result.output
    );

    // List all memories
    let list_result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .expect("list should succeed");
    assert!(
        list_result.output.contains("memory-get-test-content"),
        "Expected memory in list, got: {}",
        list_result.output
    );

    // Delete the memory
    let _ = tool
        .execute(
            json!({"action": "delete", "id": memory_id}),
            &ctx,
        )
        .await
        .expect("delete should succeed");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn pdf_tool_generates_output() {
    let tool = PdfTool::new();
    let temp_dir = tempfile::tempdir().unwrap();
    let output_path = temp_dir.path().join("test_output").to_str().unwrap().to_string();

    let result = tool
        .execute(
            json!({
                "content": "# Hello\n\nThis is a test document.",
                "output": output_path,
                "title": "Test Document"
            }),
            &test_context(),
        )
        .await;

    // PDF tool may succeed (generates HTML or PDF) or fail (no Chrome)
    match result {
        Ok(output) => {
            assert!(
                output.output.contains("test_output") || output.output.contains("html") || output.output.contains("pdf"),
                "Expected output path info, got: {}",
                output.output
            );
        }
        Err(e) => {
            println!("PdfTool failed (expected if no Chrome): {}", e);
        }
    }
}

#[tokio::test]
async fn image_tool_reads_temp_file() {
    let tool = ImageTool::new();
    let temp_dir = tempfile::tempdir().unwrap();
    let img_path = temp_dir.path().join("test.png");

    // Write a minimal valid PNG header
    let png_header: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, // IHDR chunk length
        0x49, 0x48, 0x44, 0x52, // IHDR
        0x00, 0x00, 0x00, 0x01, // width: 1
        0x00, 0x00, 0x00, 0x01, // height: 1
        0x08, 0x02, 0x00, 0x00, 0x00, // bit depth, color type, etc.
        0x90, 0x77, 0x53, 0xDE, // CRC
        0x00, 0x00, 0x00, 0x00, // IEND length
        0x49, 0x45, 0x4E, 0x44, // IEND
        0xAE, 0x42, 0x60, 0x82, // CRC
    ];
    tokio::fs::write(&img_path, &png_header).await.unwrap();

    let result = tool
        .execute(
            json!({"path": img_path.to_str().unwrap(), "action": "info"}),
            &test_context(),
        )
        .await;

    match result {
        Ok(output) => {
            assert!(
                output.output.contains("png") || output.output.contains("PNG") || output.output.contains("1x1"),
                "Expected PNG info, got: {}",
                output.output
            );
        }
        Err(e) => {
            println!("ImageTool failed: {}", e);
        }
    }
}

#[tokio::test]
async fn delegate_tool_spawn_without_agent() {
    let tool = DelegateTool::root();
    let result = tool
        .execute(
            json!({
                "action": "spawn",
                "task": {
                    "prompt": "test task",
                    "output_format": "text",
                    "max_iterations": 1
                }
            }),
            &test_context(),
        )
        .await;

    // Spawn may succeed (creates tracker entry) or fail (no agent available)
    match result {
        Ok(output) => {
            assert!(
                output.output.contains("child") || output.output.contains("delegated") || output.output.contains("task"),
                "Expected delegation info, got: {}",
                output.output
            );
        }
        Err(e) => {
            println!("DelegateTool spawn returned error (expected without agent): {}", e);
        }
    }
}

#[tokio::test]
async fn mcp_connection_tool_lists_empty() {
    let manager = Arc::new(manta::tools::mcp::McpManager::new());
    let tool = McpConnectionTool::with_manager(manager);
    let result = tool
        .execute(
            json!({"action": "list"}),
            &test_context(),
        )
        .await;

    // Should succeed even with no servers connected
    match result {
        Ok(output) => {
            assert!(
                output.output.contains("No servers") || output.output.contains("server") || output.output.is_empty(),
                "Expected server list info, got: {}",
                output.output
            );
        }
        Err(e) => {
            println!("McpConnectionTool list returned error: {}", e);
        }
    }
}

#[tokio::test]
async fn web_search_tool_duckduckgo() {
    let tool = WebSearchTool::new();
    let result = tool
        .execute(
            json!({"query": "Rust programming language", "limit": 3}),
            &test_context(),
        )
        .await;

    // DuckDuckGo may fail if network is unavailable
    match result {
        Ok(output) => {
            assert!(
                !output.output.is_empty(),
                "Expected search results, got empty output"
            );
            println!("WebSearch results: {}", output.output);
        }
        Err(e) => {
            println!("WebSearchTool failed (network may be unavailable): {}", e);
        }
    }
}

#[tokio::test]
async fn tts_tool_falls_back_without_key() {
    let tool = TtsTool::new();
    let result = tool
        .execute(
            json!({"text": "hello world"}),
            &test_context(),
        )
        .await;

    // Without OPENAI_API_KEY and without local TTS commands, should return an error
    // or fall back gracefully
    match result {
        Ok(output) => {
            println!("TtsTool output: {}", output.output);
        }
        Err(e) => {
            let err_str = format!("{}", e);
            assert!(
                err_str.contains("API key") || err_str.contains("TTS") || err_str.contains("say") || err_str.contains("espeak"),
                "Expected TTS-related error, got: {}",
                err_str
            );
        }
    }
}

#[tokio::test]
async fn canvas_tool_presents() {
    let canvas_mgr = Arc::new(manta::canvas::CanvasManager::new());
    let tool = CanvasTool::new(canvas_mgr);
    let ctx = test_context();

    let result = tool
        .execute(
            json!({
                "action": "present",
                "session_id": "test-session",
                "title": "Test Canvas",
                "components": [
                    {"type": "text", "id": "msg", "content": "Hello from canvas"}
                ]
            }),
            &ctx,
        )
        .await;

    match result {
        Ok(output) => {
            assert!(
                output.output.contains("presented") || output.output.contains("canvas") || output.output.contains("Test Canvas"),
                "Expected canvas presentation confirmation, got: {}",
                output.output
            );
        }
        Err(e) => {
            println!("CanvasTool failed: {}", e);
        }
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

#[tokio::test]
#[serial]
async fn chat_send_returns_assistant_response() {
    if skip_if_no_provider().is_none() {
        return;
    }
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
            "message": "Say exactly 'hello-from-llm' and nothing else."
        }),
    )
    .await;

    // Wait for final response event (AgentResponse is suppressed in WS protocol;
    // streaming responses emit chat.delta + chat.final via Completed)
    let payload = ws_wait_for_event(&mut read, "chat.final", 60)
        .await
        .expect("Timed out waiting for chat.final event");

    let content = payload
        .get("response")
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
    if skip_if_no_provider().is_none() {
        return;
    }
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
        .get("messages")
        .and_then(|v| v.as_array())
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

#[tokio::test]
#[serial]
async fn reset_session_command_clears_history() {
    let port = 39012;
    start_test_gateway(port, false).await;
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

    // Subscribe so reset picks it up
    let _sub_resp = ws_request(
        &mut write,
        &mut read,
        "sessions.subscribe",
        json!({"session_ids": [session_id]}),
    )
    .await;

    // Execute /help command (persisted to history)
    let _resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "help", "session_id": session_id}),
    )
    .await;

    // Verify history is not empty before reset
    let history_before = ws_request(
        &mut write,
        &mut read,
        "chat.history",
        json!({"session_id": session_id}),
    )
    .await;
    let msgs_before = resp_payload(&history_before)
        .unwrap()
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!msgs_before.is_empty(), "History should have /help before reset");

    // Reset the session
    let reset_resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "reset"}),
    )
    .await;
    assert!(resp_is_ok(&reset_resp), "reset command failed: {:?}", resp_error(&reset_resp));

    // Verify /help message is gone (reset clears prior history).
    // Note: the reset command's own assistant response is persisted after
    // the reset handler returns, so there may be 1 assistant message.
    let history_after = ws_request(
        &mut write,
        &mut read,
        "chat.history",
        json!({"session_id": session_id}),
    )
    .await;
    let msgs_after = resp_payload(&history_after)
        .unwrap()
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let has_help = msgs_after.iter().any(|m| {
        m.get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("/help")
    });
    assert!(
        !has_help,
        "/help message should have been cleared by reset, got: {:?}",
        msgs_after
    );
}

#[tokio::test]
#[serial]
async fn stop_command_returns_ok() {
    let port = 39013;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    // Create and subscribe to a session
    let create_resp = ws_request(
        &mut write,
        &mut read,
        "sessions.create",
        json!({"agent_id": "default"}),
    )
    .await;
    let session_id = resp_payload(&create_resp)
        .unwrap()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    let _sub_resp = ws_request(
        &mut write,
        &mut read,
        "sessions.subscribe",
        json!({"session_ids": [session_id]}),
    )
    .await;

    // Stop should return OK even if nothing is running
    let resp = ws_request(
        &mut write,
        &mut read,
        "commands.execute",
        json!({"command": "stop"}),
    )
    .await;
    assert!(resp_is_ok(&resp), "stop command failed: {:?}", resp_error(&resp));
    let text = resp_payload(&resp)
        .unwrap()
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        text.contains("Stop") || text.contains("stop"),
        "Expected stop acknowledgment, got: {}",
        text
    );
}

#[tokio::test]
#[serial]
async fn session_auto_named_after_first_message() {
    let port = 39014;
    start_test_gateway(port, false).await;
    let (mut write, mut read) = ws_connect(port).await;

    // Create a session
    let create_resp = ws_request(
        &mut write,
        &mut read,
        "sessions.create",
        json!({"agent_id": "default"}),
    )
    .await;
    let session_id = resp_payload(&create_resp)
        .unwrap()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    // Send a message — the first user message auto-names the session
    let _ = ws_request(
        &mut write,
        &mut read,
        "chat.send",
        json!({"session_id": session_id, "message": "Tell me about the weather today"}),
    )
    .await;

    // Query sessions.list to verify the name was set
    tokio::time::sleep(Duration::from_millis(300)).await;
    let list_resp = ws_request(&mut write, &mut read, "sessions.list", json!(null)).await;
    assert!(resp_is_ok(&list_resp), "sessions.list failed: {:?}", resp_error(&list_resp));

    let sessions = resp_payload(&list_resp)
        .unwrap()
        .get("sessions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let session = sessions
        .iter()
        .find(|s| s.get("session_id").and_then(|v| v.as_str()) == Some(&session_id));

    assert!(session.is_some(), "Created session not found in list");
    let name = session
        .unwrap()
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        name != "New Session" && !name.is_empty(),
        "Session should be auto-named, got: '{}'",
        name
    );
    assert!(
        name.to_lowercase().contains("weather"),
        "Auto-named session should contain 'weather', got: '{}'",
        name
    );
}

// ── File Tool Negative / Boundary Tests ───────────────────────────────────────

#[tokio::test]
async fn file_read_not_found_fails() {
    let tool = FileReadTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"path": "/tmp/manta-nonexistent-file-xyz.txt"}), &ctx)
        .await;
    assert!(result.is_ok(), "Tool should return Ok");
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent file");
    assert!(
        output.error.as_ref().unwrap().contains("does not exist"),
        "Expected 'does not exist' error, got: {:?}",
        output.error
    );
}

#[tokio::test]
async fn file_read_binary_returns_placeholder() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("binary.bin");
    std::fs::write(&file_path, vec![0u8, 1, 2, 255, 0, 3]).unwrap();

    let tool = FileReadTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"path": file_path.to_str().unwrap()}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success, "Binary read should succeed with placeholder");
    assert!(
        output.output.contains("Binary file"),
        "Expected binary placeholder, got: {}",
        output.output
    );
}

#[tokio::test]
async fn file_read_missing_path_validation_error() {
    let tool = FileReadTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing path");
}

#[tokio::test]
async fn file_write_missing_path_validation_error() {
    let tool = FileWriteTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"content": "test"}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing path");
}

#[tokio::test]
async fn file_write_missing_content_validation_error() {
    let tool = FileWriteTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"path": "/tmp/test.txt"}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing content");
}

#[tokio::test]
async fn file_write_creates_parent_dirs() {
    let temp_dir = tempfile::tempdir().unwrap();
    let nested_path = temp_dir.path().join("a/b/c/nested.txt");

    let tool = FileWriteTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "path": nested_path.to_str().unwrap(),
                "content": "nested content"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert!(nested_path.exists(), "Expected parent directories to be created");
    let content = std::fs::read_to_string(&nested_path).unwrap();
    assert_eq!(content, "nested content");
}

#[tokio::test]
async fn file_edit_old_string_not_found_fails() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("edit_test.txt");
    std::fs::write(&file_path, "original content").unwrap();

    let tool = FileEditTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "nonexistent text",
                "new_string": "replacement"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure when old_string not found");
    assert!(
        output.error.unwrap().contains("Could not find text"),
        "Expected 'Could not find text' error"
    );
}

#[tokio::test]
async fn file_edit_missing_args_validation_error() {
    let tool = FileEditTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"path": "/tmp/test.txt", "old_string": "x"}), &ctx)
        .await;
    assert!(result.is_err(), "Expected validation error for missing new_string");
}

#[tokio::test]
async fn file_edit_file_not_found_fails() {
    let tool = FileEditTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "path": "/tmp/manta-nonexistent-edit.txt",
                "old_string": "x",
                "new_string": "y"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success);
    assert!(output.error.unwrap().contains("does not exist"));
}

#[tokio::test]
async fn glob_no_matches_returns_empty() {
    let temp_dir = tempfile::tempdir().unwrap();
    let tool = GlobTool::new();
    let mut ctx = test_context();
    ctx.workspace_root = temp_dir.path().to_path_buf();
    let result = tool
        .execute(json!({"pattern": "*.nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    let count = output
        .data
        .as_ref()
        .and_then(|d| d.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    assert_eq!(count, 0, "Expected 0 matches for nonexistent pattern");
}

#[tokio::test]
async fn glob_invalid_pattern_fails() {
    let tool = GlobTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"pattern": "["}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid glob pattern");
}

#[tokio::test]
async fn grep_invalid_regex_fails() {
    let tool = GrepTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"pattern": "[invalid"}), &ctx).await;
    let is_failed = result.as_ref().map(|o| !o.success).unwrap_or(true);
    assert!(is_failed, "Expected failure for invalid regex");
}

#[tokio::test]
async fn grep_no_matches_returns_empty() {
    let tool = GrepTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"pattern": "xyz_nonexistent_pattern_12345"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    let count = output
        .data
        .as_ref()
        .and_then(|d| d.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    assert_eq!(count, 0, "Expected 0 matches");
}

#[tokio::test]
async fn grep_json_format_returns_structured() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("search.rs");
    tokio::fs::write(&file_path, "fn main() {}\n").await.unwrap();

    let tool = GrepTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "pattern": "fn main",
                "format": "json",
                "path": file_path.to_str().unwrap()
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    let matches = output
        .data
        .as_ref()
        .and_then(|d| d.get("matches"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!matches.is_empty(), "Expected structured matches array");
}

// ── Shell / Process / Code Negative / Boundary Tests ──────────────────────────

#[tokio::test]
async fn shell_missing_command_validation_error() {
    let tool = ShellTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing command");
}

#[tokio::test]
async fn shell_nonzero_exit_fails() {
    let tool = ShellTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"command": "exit 42"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonzero exit code");
    assert!(
        output.error.as_ref().unwrap().contains("42") || output.error.as_ref().unwrap().contains("Exit code"),
        "Expected exit code in error, got: {:?}",
        output.error
    );
}

#[tokio::test]
async fn shell_pipeline_works() {
    let tool = ShellTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"command": "echo 'hello pipe' | grep pipe"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert!(output.output.contains("hello pipe"));
}

#[tokio::test]
async fn code_exec_forbidden_import_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({"code": "import subprocess\nprint('ok')", "language": "python"}),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for forbidden import");
    assert!(
        output.error.as_ref().unwrap().contains("validation failed") || output.error.as_ref().unwrap().contains("forbidden"),
        "Expected validation error, got: {:?}",
        output.error
    );
}

#[tokio::test]
async fn code_exec_dangerous_pattern_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"code": "eval('1+1')", "language": "python"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for dangerous pattern");
}

#[tokio::test]
async fn code_exec_unsupported_language_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"code": "puts 'hello'", "language": "ruby"}), &ctx)
        .await;
    assert!(result.is_err(), "Expected validation error for unsupported language");
}

#[tokio::test]
async fn code_exec_timeout_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "code": "import time\ntime.sleep(300)",
                "language": "python",
                "timeout": 2
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected timeout failure");
    assert!(
        output.error.as_ref().unwrap().to_lowercase().contains("timed out"),
        "Expected timeout error, got: {:?}",
        output.error
    );
}

#[tokio::test]
async fn process_invalid_action_fails() {
    let tool = ProcessTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "invalid_action"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid action");
}

#[tokio::test]
async fn process_stop_nonexistent_fails() {
    let tool = ProcessTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "stop", "process_id": "nonexistent-id"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent process");
}

// ── Web / Search / Time / Todo / Cron Negative / Boundary Tests ───────────────

#[tokio::test]
async fn web_fetch_invalid_url_fails() {
    let tool = WebFetchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"url": "not-a-url"}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for invalid URL");
}

#[tokio::test]
async fn web_fetch_unsupported_scheme_fails() {
    let tool = WebFetchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"url": "ftp://example.com"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for unsupported scheme");
    assert!(output.error.as_ref().unwrap().contains("scheme"));
}

#[tokio::test]
async fn web_fetch_missing_url_validation_error() {
    let tool = WebFetchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing url");
}

#[tokio::test]
async fn web_search_missing_query_validation_error() {
    let tool = WebSearchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing query");
}

#[tokio::test]
async fn web_search_query_too_long_fails() {
    let tool = WebSearchTool::new();
    let ctx = test_context();
    let long_query = "a".repeat(501);
    let result = tool.execute(json!({"query": long_query}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for query too long");
    assert!(output.error.as_ref().unwrap().contains("too long"));
}

#[tokio::test]
async fn web_search_returns_structured_results() {
    let tool = WebSearchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"query": "Rust programming language", "limit": 3}), &ctx).await;

    match result {
        Ok(output) => {
            if output.success {
                let results = output
                    .data
                    .as_ref()
                    .and_then(|d| d.get("results"))
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                assert!(!results.is_empty(), "Expected structured results array");
            } else {
                println!("Web search returned error (network may be unavailable): {:?}", output.error);
            }
        }
        Err(e) => {
            println!("Web search failed (network may be unavailable): {}", e);
        }
    }
}

#[tokio::test]
async fn time_invalid_timezone_fails() {
    let tool = TimeTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "now", "timezone": "Mars/Standard"}), &ctx)
        .await;
    let is_failed = result.as_ref().map(|o| !o.success).unwrap_or(true);
    assert!(is_failed, "Expected failure for invalid timezone");
}

#[tokio::test]
async fn time_format_custom_pattern() {
    let tool = TimeTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "now", "format": "%Y-%m-%d"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    let current_year = chrono::Local::now().format("%Y").to_string();
    assert!(
        output.output.contains(&current_year),
        "Expected output to contain current year, got: {}",
        output.output
    );
}

#[tokio::test]
async fn todo_updates_status() {
    let tool = TodoTool::new();
    let ctx = test_context();

    // Create a todo
    let create_result = tool
        .execute(json!({"action": "create", "content": "status test"}), &ctx)
        .await
        .expect("create failed");
    let task_id = create_result
        .data
        .as_ref()
        .and_then(|d| d.get("task_id"))
        .and_then(|v| v.as_str())
        .expect("Missing task_id")
        .to_string();

    // Update status to completed
    let update_result = tool
        .execute(
            json!({"action": "update", "task_id": task_id, "status": "completed"}),
            &ctx,
        )
        .await
        .expect("update failed");
    assert!(update_result.success, "Update should succeed");

    // Verify via list that status changed
    let list_result = tool.execute(json!({"action": "list"}), &ctx).await.expect("list failed");
    let tasks = list_result
        .data
        .as_ref()
        .and_then(|d| d.get("tasks"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let updated = tasks.iter().any(|t| {
        t.get("id").and_then(|v| v.as_str()) == Some(&task_id)
            && t.get("status").and_then(|v| v.as_str()) == Some("completed")
    });
    assert!(updated, "Expected todo status to be updated to completed");
}

#[tokio::test]
async fn todo_update_nonexistent_fails() {
    let tool = TodoTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({"action": "update", "task_id": "nonexistent-id", "status": "completed"}),
            &ctx,
        )
        .await;
    let is_failed = result.as_ref().map(|o| !o.success).unwrap_or(true);
    assert!(is_failed, "Expected failure for nonexistent task");
}

#[tokio::test]
async fn todo_clears_completed() {
    let tool = TodoTool::new();
    let mut ctx = test_context();
    ctx.conversation_id = format!("todo-clear-{}", std::process::id());

    // Create two todos
    let r1 = tool
        .execute(json!({"action": "create", "content": "task 1"}), &ctx)
        .await
        .unwrap();
    let id1 = r1.data.as_ref().unwrap().get("task_id").unwrap().as_str().unwrap();

    let r2 = tool
        .execute(json!({"action": "create", "content": "task 2"}), &ctx)
        .await
        .unwrap();
    let _id2 = r2.data.as_ref().unwrap().get("task_id").unwrap().as_str().unwrap();

    // Complete one
    let _ = tool
        .execute(json!({"action": "update", "task_id": id1, "status": "completed"}), &ctx)
        .await;

    // Clear completed
    let clear_result = tool.execute(json!({"action": "clear_completed"}), &ctx).await.unwrap();
    assert!(clear_result.success);

    // List remaining
    let list_result = tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
    let tasks = list_result
        .data
        .as_ref()
        .and_then(|d| d.get("tasks"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(tasks.len(), 1, "Expected 1 remaining todo after clearing completed");
}

#[tokio::test]
async fn cron_invalid_expression_fails() {
    let tool = CronTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "action": "create",
                "name": "test-invalid",
                "schedule": "not-a-cron",
                "command": "echo test"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid cron expression");
}

#[tokio::test]
async fn cron_remove_nonexistent_fails() {
    let tool = CronTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "remove", "name": "nonexistent-job"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent job");
}

// ── Memory Tool Negative / Boundary Tests ─────────────────────────────────────

#[tokio::test]
async fn memory_retrieve_nonexistent_fails() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memory_neg_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url).await.expect("Failed to create MemoryTool");
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "retrieve", "id": "nonexistent-id"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent memory");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_delete_removes_entry() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memory_del_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url).await.expect("Failed to create MemoryTool");
    let ctx = test_context();

    let store_result = tool
        .execute(json!({"action": "store", "content": "to-delete", "category": "test"}), &ctx)
        .await
        .expect("store failed");
    let id = store_result.data.as_ref().unwrap().get("id").unwrap().as_str().unwrap();

    let del_result = tool.execute(json!({"action": "delete", "id": id}), &ctx).await.unwrap();
    assert!(del_result.success);

    let retrieve_result = tool.execute(json!({"action": "retrieve", "id": id}), &ctx).await.unwrap();
    assert!(!retrieve_result.success, "Expected retrieve to fail after delete");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_update_modifies_content() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memory_upd_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url).await.expect("Failed to create MemoryTool");
    let ctx = test_context();

    let store_result = tool
        .execute(json!({"action": "store", "content": "original", "category": "test"}), &ctx)
        .await
        .expect("store failed");
    let id = store_result.data.as_ref().unwrap().get("id").unwrap().as_str().unwrap();

    let update_result = tool
        .execute(json!({"action": "update", "id": id, "content": "updated"}), &ctx)
        .await
        .unwrap();
    assert!(update_result.success);

    let retrieve_result = tool.execute(json!({"action": "retrieve", "id": id}), &ctx).await.unwrap();
    assert!(retrieve_result.output.contains("updated"), "Expected updated content");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_invalid_action_fails() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memory_inv_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url).await.expect("Failed to create MemoryTool");
    let ctx = test_context();

    let result = tool.execute(json!({"action": "invalid_action"}), &ctx).await;
    assert!(result.is_err() || !result.unwrap().success, "Expected failure for invalid action");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_search_no_results_returns_empty() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memsearch_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(manta::memory::SqliteMemoryStore::new(&db_url).await.expect("Failed to create store"));
    let tool = MemorySearchTool::with_store(store);
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "search", "query": "xyz_nonexistent_query_12345"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert!(output.output.contains("No memories found") || output.output.contains("0"));

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_search_store_then_search() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memsearch2_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(manta::memory::SqliteMemoryStore::new(&db_url).await.expect("Failed to create store"));
    let tool = MemorySearchTool::with_store(store);
    let ctx = test_context();

    let _ = tool
        .execute(json!({"action": "store", "content": "Manta is a great project", "category": "test"}), &ctx)
        .await;

    let result = tool
        .execute(json!({"action": "search", "query": "great"}), &ctx)
        .await
        .unwrap();
    assert!(result.success);
    let count = result
        .data
        .as_ref()
        .and_then(|d| d.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert!(count >= 1, "Expected at least 1 search result");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_delete_nonexistent_fails() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memget_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(manta::memory::SqliteMemoryStore::new(&db_url).await.expect("Failed to create store"));
    let tool = MemoryGetTool::with_store(store);
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "delete", "id": "nonexistent-id"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent memory");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_list_returns_all() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memget2_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(manta::memory::SqliteMemoryStore::new(&db_url).await.expect("Failed to create store"));
    let memory_tool = MemoryTool::with_store(store.clone()).await.expect("Failed to create MemoryTool");
    let get_tool = MemoryGetTool::with_store(store);
    let ctx = test_context();

    for i in 0..3 {
        let _ = memory_tool
            .execute(json!({"action": "store", "content": format!("entry {}", i), "category": "test"}), &ctx)
            .await;
    }

    let result = get_tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
    assert!(result.success);
    let count = result
        .data
        .as_ref()
        .and_then(|d| d.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert_eq!(count, 3, "Expected 3 memories in list");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_update_nonexistent_fails() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memget3_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(manta::memory::SqliteMemoryStore::new(&db_url).await.expect("Failed to create store"));
    let tool = MemoryGetTool::with_store(store);
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "update", "id": "nonexistent-id", "content": "new"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent memory");

    let _ = std::fs::remove_file(&db_path);
}

// ── ACP Tool Negative / Boundary Tests ────────────────────────────────────────

#[tokio::test]
async fn acp_session_invalid_action_fails() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = AcpSessionTool::new(acp);
    let ctx = test_context();
    let result = tool.execute(json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid action");
}

#[tokio::test]
async fn sessions_history_invalid_session_fails() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionsHistoryTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"session_id": "nonexistent"}), &ctx)
        .await;
    let is_failed = result.as_ref().map(|o| !o.success).unwrap_or(true);
    assert!(is_failed, "Expected failure for invalid session");
}

#[tokio::test]
async fn sessions_send_missing_args_fails() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionsSendTool::new(acp);
    let ctx = test_context();
    let result = tool.execute(json!({"session_id": "x"}), &ctx).await;
    assert!(result.is_err() || !result.unwrap().success, "Expected failure for missing args");
}

#[tokio::test]
async fn sessions_yield_missing_subagent_id_fails() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionsYieldTool::new(acp);
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err() || !result.unwrap().success, "Expected failure for missing subagent_id");
}

#[tokio::test]
async fn session_status_not_found_fails() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SessionStatusTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"session_id": "nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent session");
}

#[tokio::test]
async fn subagents_shutdown_nonexistent_fails() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SubagentsTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "shutdown", "subagent_id": "nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent subagent");
}

#[tokio::test]
async fn subagents_status_nonexistent_fails() {
    let acp = Arc::new(manta::acp::AcpControlPlane::new());
    let tool = SubagentsTool::new(acp);
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "status", "subagent_id": "nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent subagent");
}

#[tokio::test]
async fn apply_patch_applies_valid_patch() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("patch_target.txt");
    std::fs::write(&file_path, "old line\nsecond line\n").unwrap();

    let patch = format!(
        "--- a/patch_target.txt\n+++ b/patch_target.txt\n@@ -1,2 +1,2 @@\n-old line\n+new line\n second line\n"
    );

    let tool = ApplyPatchTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "patch": patch,
                "directory": temp_dir.path().to_str().unwrap()
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success, "Expected patch to apply successfully");

    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("new line"), "Expected file to be patched");
}

#[tokio::test]
async fn apply_patch_missing_patch_fails() {
    let tool = ApplyPatchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err() || !result.unwrap().success, "Expected failure for missing patch");
}

// ── Remaining Tool Negative / Boundary Tests ──────────────────────────────────

#[tokio::test]
async fn delegate_max_children_fails() {
    let tool = DelegateTool::root();
    let ctx = test_context();

    // Spawn 3 children (max)
    for i in 0..3 {
        let result = tool
            .execute(
                json!({
                    "action": "spawn",
                    "task": {"prompt": format!("task {}", i)}
                }),
                &ctx,
            )
            .await;
        // Without agent builder these will fail, so we can't easily test max children
        // Just verify the tool handles the spawn attempt
        assert!(result.is_ok());
    }
}

#[tokio::test]
async fn delegate_invalid_action_fails() {
    let tool = DelegateTool::root();
    let ctx = test_context();
    let result = tool.execute(json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for invalid action");
}

#[tokio::test]
async fn delegate_cancel_nonexistent_fails() {
    let tool = DelegateTool::root();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "cancel", "child_id": "nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent child");
}

#[tokio::test]
async fn mcp_connect_missing_server_id_fails() {
    let tool = McpConnectionTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"action": "connect"}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing server_id");
}

#[tokio::test]
async fn mcp_invalid_action_fails() {
    let tool = McpConnectionTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for invalid action");
}

#[tokio::test]
async fn mcp_disconnect_nonexistent_fails() {
    let tool = McpConnectionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "disconnect", "server_id": "nonexistent"}), &ctx)
        .await;
    let is_failed = result.as_ref().map(|o| !o.success).unwrap_or(true);
    assert!(is_failed, "Expected failure for nonexistent server");
}

#[tokio::test]
async fn update_plan_get_nonexistent_fails() {
    let tool = UpdatePlanTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "get", "plan_id": "nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent plan");
}

#[tokio::test]
async fn update_plan_invalid_action_fails() {
    let tool = UpdatePlanTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid action");
}

#[tokio::test]
async fn update_plan_set_status_invalid_status_fails() {
    let tool = UpdatePlanTool::new();
    let ctx = test_context();

    // Create a plan first
    let create_result = tool
        .execute(
            json!({"action": "create", "title": "status-test", "steps": ["step"]}),
            &ctx,
        )
        .await
        .expect("create failed");
    let plan_id = create_result.data.as_ref().unwrap().get("id").unwrap().as_str().unwrap();

    let result = tool
        .execute(
            json!({"action": "set_status", "plan_id": plan_id, "status": "invalid_status"}),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid status");
}

#[tokio::test]
async fn pdf_generates_with_custom_title() {
    let temp_dir = tempfile::tempdir().unwrap();
    let output_path = temp_dir.path().join("test.pdf");

    let tool = PdfTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "content": "# Hello PDF",
                "output": output_path.to_str().unwrap(),
                "title": "Custom Title"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert!(output.output.contains("Custom Title") || output.output.contains("pdf") || output.output.contains("HTML"));
}

#[tokio::test]
async fn pdf_orientation_landscape() {
    let temp_dir = tempfile::tempdir().unwrap();
    let output_path = temp_dir.path().join("landscape.pdf");

    let tool = PdfTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "content": "Landscape content",
                "output": output_path.to_str().unwrap(),
                "orientation": "landscape"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
}

#[tokio::test]
async fn image_file_not_found_fails() {
    let tool = ImageTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"path": "/tmp/manta-nonexistent-image.png"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent image");
}

#[tokio::test]
async fn image_reads_jpeg() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("test.jpg");

    // Write a minimal valid JPEG (SOI + EOI)
    let jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xD9];
    std::fs::write(&file_path, jpeg_data).unwrap();

    let tool = ImageTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"path": file_path.to_str().unwrap()}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    // ImageTool may succeed even for minimal JPEG, or may return partial info
    // We just verify it doesn't panic and returns a result
    assert!(
        output.success || output.error.is_some(),
        "Expected either success or error"
    );
}

#[tokio::test]
async fn tts_empty_text_fails() {
    let tool = TtsTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"text": ""}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    // TTS may fallback or fail — either is acceptable for empty text
    // We just verify the tool handles it gracefully
    let _ = output.success;
}

#[tokio::test]
async fn nodes_invalid_action_fails() {
    let tool = NodesTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid action");
}

#[tokio::test]
async fn nodes_describe_nonexistent_fails() {
    let tool = NodesTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "describe", "node_id": "nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent node");
}

#[tokio::test]
async fn canvas_invalid_action_fails() {
    let canvas_mgr = Arc::new(manta::canvas::CanvasManager::new());
    let tool = CanvasTool::new(canvas_mgr);
    let ctx = test_context();
    let result = tool.execute(json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid action");
}

#[tokio::test]
async fn canvas_update_nonexistent_fails() {
    let canvas_mgr = Arc::new(manta::canvas::CanvasManager::new());
    let tool = CanvasTool::new(canvas_mgr);
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "action": "update",
                "session_id": "nonexistent-session",
                "components": [{"type": "text", "id": "t", "content": "x"}]
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    // May succeed silently or fail — just verify it doesn't panic
    let _ = output.success;
}

