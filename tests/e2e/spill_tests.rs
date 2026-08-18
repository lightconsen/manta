use super::*;

/// Mock provider that asks the shell for a 40KB output, then answers.
fn big_output_mock_provider() -> MockProvider {
    MockProvider::new().with_callback(|messages| {
        // Cache-check prompt is a single user message asking about caching.
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }
        if messages.iter().any(|m| m.role == Role::Tool) {
            return ProviderMessage::assistant("Done.");
        }
        // `echo` is a shell builtin, so this works even with a cleared env.
        let command = format!("echo {}", "x".repeat(40_000));
        ProviderMessage::assistant("Running a noisy command.").with_tool_calls(vec![ToolCall {
            id: "call_big_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "shell".to_string(),
                arguments: serde_json::json!({ "command": command }).to_string(),
            },
            index: None,
            result: None,
        }])
    })
}

/// An oversized tool output must reach the model as a bounded head/tail
/// preview pointing at a spill file inside the agent workspace — never the
/// raw multi-KB blob — while the full output is preserved on disk.
#[tokio::test]
#[serial]
async fn test_oversized_tool_output_spills_to_workspace_file() {
    let port = 41220;
    let ws = std::env::temp_dir().join(format!("syscity_spill_e2e_{}", port));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).expect("create temp workspace");

    let mut config = test_config(port, false);
    config.model_provider = "mock".to_string();
    config.model = "mock-model".to_string();
    config.default_agent.workspace_dir = Some(ws.clone());

    let gateway = Gateway::new(config, None)
        .await
        .expect("Failed to create test gateway");
    let router = gateway.model_router();
    let mock = big_output_mock_provider();
    register_mock_provider_with_model(&router, mock.clone(), "mock-model").await;

    start_gateway_and_wait(port, gateway).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;
    client.send_chat(&sid, "run the noisy command").await;

    client
        .wait_for_event("chat.final", 30)
        .await
        .expect("Expected chat.final event");

    // The model's second request must carry the preview, not the raw blob.
    let history = mock.history();
    let tool_contents: Vec<&str> = history
        .iter()
        .flat_map(|req| req.messages.iter())
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(tool_contents.len(), 1, "exactly one tool result expected");
    let preview = tool_contents[0];
    assert!(
        preview.contains(".syscity/spill/"),
        "preview should point at the spill file: {:.200}",
        preview
    );
    assert!(preview.contains("file_read"), "retrieval hint present");
    assert!(preview.len() < 34_000, "preview must be bounded, got {} bytes", preview.len());

    // The full output is preserved inside the workspace spill dir.
    let spill_dir = ws.join(".syscity").join("spill");
    let files: Vec<_> = std::fs::read_dir(&spill_dir)
        .expect("spill dir exists")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(files.len(), 1);
    let on_disk = std::fs::read_to_string(files[0].path()).unwrap();
    assert_eq!(on_disk.len(), 40_001, "40KB of x's plus echo's newline");

    let _ = std::fs::remove_dir_all(&ws);
}
