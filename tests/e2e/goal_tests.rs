use super::*;

/// Helper: execute a slash command with arguments via `commands.execute`.
///
/// `FrontendSimulator::execute_command` only passes the `command` field,
/// but goal and other argument-driven commands expect `params.args`.
async fn exec_goal(client: &mut FrontendSimulator, args: &str) -> serde_json::Value {
    client
        .request("commands.execute", json!({"command": "goal", "args": args}))
        .await
}

/// Build a MockProvider that handles both goal parsing and sub-agent execution.
///
/// - Cache checks ("NOCACHE") → "NOCACHE"
/// - Goal parsing (system prompt contains "goal analyzer") → JSON plan with
///   `model_override: "mock-model"` so the runner resolves correctly
/// - Sub-agent execution → text response ("Task completed.")
fn goal_mock_provider() -> MockProvider {
    MockProvider::new().with_callback(|messages| {
        // Cache-check prompt is a single user message asking about caching.
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }

        // Goal parsing call: system prompt contains "goal analyzer".
        let is_goal_parse = messages.iter().any(|m| m.content.contains("goal analyzer"));
        if is_goal_parse {
            return ProviderMessage::assistant(
                r#"{"description":"test goal","conditions":[{"type":"exit_code","command":"true","expected":0}],"max_rounds":1,"model_override":"mock-model"}"#,
            );
        }

        // Sub-agent execution: return text so the agent finishes immediately.
        ProviderMessage::assistant("Task completed.")
    })
}

/// Start a test gateway for goal tests with proper mock router setup.
async fn start_goal_test_gateway(port: u16) {
    let mut config = test_config(port, false);
    config.model_provider = "mock".to_string();
    config.model = "mock-model".to_string();

    let gateway = Gateway::new(config, None)
        .await
        .expect("Failed to create test gateway");

    let router = gateway.model_router();
    router
        .add_provider_instance("mock", Arc::new(goal_mock_provider()))
        .await
        .expect("Failed to register mock provider");

    // Register both the model alias and the "default" alias so that
    // complete_auto → "default_model" → "default" → mock provider.
    router
        .set_alias(ModelAlias {
            name: "mock-model".to_string(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            temperature: None,
            max_tokens: None,
        })
        .await;
    router
        .set_alias(ModelAlias {
            name: "default".to_string(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            temperature: None,
            max_tokens: None,
        })
        .await;
    router
        .switch_default_model("default")
        .await
        .expect("Failed to set default model");

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

#[tokio::test]
#[serial]
async fn test_goal_full_lifecycle() {
    let port = 41106;
    start_goal_test_gateway(port).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    let resp = exec_goal(&mut client, "write tests --max-rounds 1").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));

    // Wait for goal.started event.
    let started = client
        .wait_for_event("goal.progress", 15)
        .await
        .expect("Expected goal.started event");
    let inner = started.get("event").and_then(|v| v.as_object());
    assert!(inner.is_some(), "Expected nested goal.started event");
    assert_eq!(
        inner.and_then(|o| o.get("event")).and_then(|v| v.as_str()),
        Some("goal.started")
    );

    // Wait for goal.retry event (first round feedback).
    let retry = client
        .wait_for_event("goal.progress", 15)
        .await
        .expect("Expected goal.retry event");
    let retry_inner = retry.get("event").and_then(|v| v.as_object());
    assert!(retry_inner.is_some(), "Expected nested goal.retry event");
    assert_eq!(
        retry_inner
            .and_then(|o| o.get("event"))
            .and_then(|v| v.as_str()),
        Some("goal.retry")
    );

    // Wait for goal.check event.
    let check = client
        .wait_for_event("goal.progress", 15)
        .await
        .expect("Expected goal.check event");
    let check_inner = check.get("event").and_then(|v| v.as_object());
    assert!(check_inner.is_some(), "Expected nested goal.check event");
    assert_eq!(
        check_inner
            .and_then(|o| o.get("event"))
            .and_then(|v| v.as_str()),
        Some("goal.check")
    );

    // Wait for terminal event (goal.done or goal.aborted).
    let terminal = client
        .wait_for_event("goal.progress", 15)
        .await
        .expect("Expected terminal goal event");
    let term_inner = terminal.get("event").and_then(|v| v.as_object());
    assert!(term_inner.is_some(), "Expected nested terminal event");
    let term_type = term_inner
        .and_then(|o| o.get("event"))
        .and_then(|v| v.as_str());
    assert!(
        term_type == Some("goal.done") || term_type == Some("goal.aborted"),
        "Expected goal.done or goal.aborted, got: {:?}",
        term_type
    );
}

#[tokio::test]
#[serial]
async fn test_goal_command_starts_and_completes() {
    let port = 41100;
    start_goal_test_gateway(port).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    // Send /goal command.
    let resp = exec_goal(&mut client, "write tests --max-rounds 1").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));

    // Expect goal.started event.
    let started = client
        .wait_for_event("goal.progress", 15)
        .await
        .expect("Expected goal.progress event");
    let inner = started.get("event").and_then(|v| v.as_object());
    assert!(inner.is_some(), "Expected nested goal.started event");
    let event_type = inner.and_then(|o| o.get("event")).and_then(|v| v.as_str());
    assert_eq!(event_type, Some("goal.started"));
}

#[tokio::test]
#[serial]
async fn test_goal_command_creates_goal_id() {
    let port = 41101;
    start_goal_test_gateway(port).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    let resp = exec_goal(&mut client, "write tests --max-rounds 1").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));

    let payload = resp.get("payload").and_then(|v| v.as_object()).cloned();
    assert!(payload.is_some(), "Expected payload in response: {:?}", resp);
    let goal_id = payload
        .as_ref()
        .and_then(|p| p.get("goal_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    assert!(goal_id.is_some(), "Expected goal_id in response payload");
    let gid = goal_id.unwrap();
    assert!(gid.starts_with("goal_"), "goal_id should start with 'goal_'");
}

#[tokio::test]
#[serial]
async fn test_goal_list_shows_active_goals() {
    let port = 41102;
    start_goal_test_gateway(port).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    // Should show no goals initially.
    let list_resp = exec_goal(&mut client, "list").await;
    assert!(list_resp.get("ok").and_then(|v| v.as_bool()) == Some(true));

    // Start a goal.
    let resp = exec_goal(&mut client, "write tests --max-rounds 1").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));

    // List should show the active goal.
    let list_resp2 = exec_goal(&mut client, "list").await;
    assert!(list_resp2.get("ok").and_then(|v| v.as_bool()) == Some(true));
    let text = list_resp2
        .get("payload")
        .and_then(|p| p.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(text.contains("goal_"), "Expected goal ID in list response: {}", text);
}

#[tokio::test]
#[serial]
async fn test_goal_cancel_aborts_goal() {
    let port = 41103;
    start_goal_test_gateway(port).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    // Start a goal.
    let resp = exec_goal(&mut client, "write tests --max-rounds 5").await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));
    let goal_id = resp
        .get("payload")
        .and_then(|p| p.get("goal_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Cancel it by ID.
    let cancel_resp = exec_goal(&mut client, &format!("cancel {}", goal_id)).await;
    assert!(
        cancel_resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "Cancel failed: {:?}",
        cancel_resp
    );
    let text = cancel_resp
        .get("payload")
        .and_then(|p| p.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(text.contains(&goal_id), "Cancel response should mention goal ID");
}

#[tokio::test]
#[serial]
async fn test_goal_cancel_unknown_id_returns_error() {
    let port = 41104;
    start_goal_test_gateway(port).await;
    let mut client = FrontendSimulator::connect(port).await;

    let _sid = client.create_session().await;

    let cancel_resp = exec_goal(&mut client, "cancel nonexistent_goal_xyz").await;
    assert!(
        cancel_resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for unknown goal ID"
    );
}

#[tokio::test]
#[serial]
async fn test_goal_empty_description_returns_error() {
    let port = 41105;
    start_goal_test_gateway(port).await;
    let mut client = FrontendSimulator::connect(port).await;

    let _sid = client.create_session().await;

    let resp = exec_goal(&mut client, "  ").await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for empty goal description"
    );
}
