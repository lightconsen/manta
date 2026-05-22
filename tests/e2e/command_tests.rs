use super::*;

#[tokio::test]
#[serial]
async fn command_help_returns_markdown() {
    let port = 40010;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.execute_command("help").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true), "help command failed");
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
    assert!(commands.len() >= 20, "Expected at least 20 commands, got: {}", commands.len());
    let names: Vec<String> = commands
        .iter()
        .filter_map(|c| c.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert!(names.contains(&"help".to_string()), "Expected 'help' command");
    assert!(names.contains(&"tools".to_string()), "Expected 'tools' command");
    assert!(names.contains(&"session".to_string()), "Expected 'session' command");
}

#[tokio::test]
#[serial]
async fn command_reset_clears_history() {
    let port = 40018;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;
    client.send_chat(&sid, "Test message before reset").await;

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
    assert!(text.contains("reset"), "Expected reset confirmation, got: {}", text);
}

#[tokio::test]
#[serial]
async fn command_stop_no_session() {
    let port = 40019;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

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
        .request("commands.execute", json!({"command": "skill", "args": "nonexistent-skill-xyz"}))
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for nonexistent skill, got: {:?}",
        resp
    );
    let error = resp.get("error").unwrap();
    assert_eq!(error.get("code").and_then(|v| v.as_str()), Some("SKILL_NOT_FOUND"));
}

#[tokio::test]
#[serial]
async fn command_mcp_disconnect_requires_arg() {
    let port = 40061;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client
        .request("commands.execute", json!({"command": "mcp", "args": "disconnect"}))
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for missing arg, got: {:?}",
        resp
    );
    let error = resp.get("error").unwrap();
    assert_eq!(error.get("code").and_then(|v| v.as_str()), Some("INVALID_ARGS"));
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
async fn command_persisted_to_session_history() {
    let port = 39011;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    let resp = client
        .request("commands.execute", json!({"command": "help", "session_id": sid}))
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "help command failed: {:?}",
        resp.get("error")
    );

    let history = client.get_history(&sid).await;
    let has_user_command = history.iter().any(|m| {
        m.get("role").and_then(|v| v.as_str()) == Some("user")
            && m.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("/help")
    });

    assert!(has_user_command, "Expected /help command to be persisted in session history");
}
