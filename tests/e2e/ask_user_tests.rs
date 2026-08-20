use super::*;

/// Build a MockProvider that drives one `ask_user` turn.
///
/// - Cache-check prompts ("NOCACHE") → "NOCACHE"
/// - First turn → tool call `ask_user` with a multiple-choice question.
/// - After the tool result (the human's answer) is back → final answer.
fn ask_user_mock_provider() -> MockProvider {
    MockProvider::new().with_callback(|messages| {
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }
        let has_tool_result = messages.iter().any(|m| m.role == Role::Tool);
        if has_tool_result {
            return ProviderMessage::assistant("Received your answer. Done!");
        }
        ProviderMessage::assistant("I need to ask you a question.").with_tool_calls(vec![
            ToolCall {
                id: "call_ask_1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "ask_user".to_string(),
                    arguments: r#"{"question":"Continue?","options":["yes","no"]}"#.to_string(),
                },
                index: None,
                result: None,
            },
        ])
    })
}

/// End-to-end: an `ask_user` call blocks the turn until the human answers
/// over the WS `ask.respond` method, then the agent resumes and finishes.
#[tokio::test]
#[serial]
async fn ask_user_blocks_turn_until_ws_respond_then_resumes() {
    let port = 41270;
    start_test_gateway_with_mock(port, ask_user_mock_provider()).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client.send_chat(&sid, "Please ask me a question.").await;

    // The agent pauses inside ask_user; the gateway broadcasts ask.required.
    let ask_payload = client
        .wait_for_event("ask.required", 30)
        .await
        .expect("Expected ask.required event");
    let ask_id = ask_payload["ask_id"].as_str().unwrap().to_string();
    assert_eq!(ask_payload["question"], "Continue?");
    assert_eq!(ask_payload["options"], json!(["yes", "no"]));
    assert_eq!(ask_payload["required"], true);
    assert_eq!(ask_payload["session_id"], sid);

    // The human answers through the WS method; the tool resumes.
    let resp = client
        .request("ask.respond", json!({ "ask_id": ask_id, "response": "yes" }))
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "ask.respond failed: {:?}",
        resp.get("error")
    );

    // The queue broadcasts resolution back to the same session.
    let resolved = client
        .wait_for_event("ask.resolved", 10)
        .await
        .expect("Expected ask.resolved event");
    assert_eq!(resolved["ask_id"], ask_id);
    assert_eq!(resolved["cancelled"], false);

    // The agent sees the answer and finishes the turn.
    let final_payload = client
        .wait_for_event("chat.final", 30)
        .await
        .expect("Expected chat.final event after answering");
    let response = final_payload["response"].as_str().unwrap();
    assert!(
        response.contains("Done"),
        "final response should reflect the answered turn, got: {response}"
    );
}
