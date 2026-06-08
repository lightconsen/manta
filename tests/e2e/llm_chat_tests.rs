use super::*;

#[tokio::test]
#[serial]
async fn llm_chat_streaming_journey() {
    let port = 40040;
    if pick_test_provider().is_some() {
        start_test_gateway(port, true).await;
    } else {
        start_test_gateway_with_mock(port, llm_mock_provider_for_streaming()).await;
    }
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(&sid, "Say exactly 'pong-from-llm' and nothing else.")
        .await;

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

    let mut history = Vec::new();
    for _ in 0..20 {
        history = client.get_history(&sid).await;
        let has_assistant = history
            .iter()
            .any(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"));
        if has_assistant {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let has_user = history
        .iter()
        .any(|m| m.get("role").and_then(|v| v.as_str()) == Some("user"));
    let has_assistant = history
        .iter()
        .any(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"));
    assert!(has_user, "User message should be persisted");
    assert!(has_assistant, "Assistant response should be persisted");
}

#[tokio::test]
#[serial]
async fn llm_tool_invocation_journey() {
    let port = 40041;
    if pick_test_provider().is_some() {
        start_test_gateway(port, true).await;
    } else {
        start_test_gateway_with_mock(port, llm_mock_provider_for_tool("time")).await;
    }
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(&sid, "What is the current date and time? Reply with just the year.")
        .await;

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

    if !tool_calling.is_empty() {
        let first = &tool_calling[0];
        assert_eq!(first.get("session_id").and_then(|v| v.as_str()), Some(sid.as_str()));
        assert!(first.get("tool_name").is_some(), "tool.calling event should have tool_name");
    }

    if !tool_results.is_empty() {
        let first = &tool_results[0];
        assert_eq!(first.get("session_id").and_then(|v| v.as_str()), Some(sid.as_str()));
        assert!(first.get("result").is_some(), "tool.result event should have result");
    }

    assert!(chat_final.is_some(), "Expected chat.final event within 60s");
}

#[tokio::test]
#[serial]
async fn session_created_event_on_first_chat() {
    let port = 40042;
    if pick_test_provider().is_some() {
        start_test_gateway(port, true).await;
    } else {
        start_test_gateway_with_mock(port, llm_mock_provider_for_streaming()).await;
    }
    let mut client = FrontendSimulator::connect(port).await;

    client
        .request("chat.send", json!({"message": "Hi there"}))
        .await;

    let payload = client
        .wait_for_event("session.created", 5)
        .await
        .expect("Expected session.created event");

    assert!(payload.get("session_id").is_some(), "session.created should contain session_id");
}
