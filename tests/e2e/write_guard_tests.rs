use super::*;

/// Mock provider that first tries to edit without reading (blocked by the
/// write guard), then follows the feedback: read, edit, done.
fn edit_without_read_mock() -> MockProvider {
    MockProvider::new().with_callback(|messages| {
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }
        let last_tool = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Tool)
            .map(|m| m.content.clone());
        match last_tool.as_deref() {
            // First step: blind edit — the guard must reject this.
            None => ProviderMessage::assistant("Editing directly.").with_tool_calls(vec![
                ToolCall {
                    id: "call_blind".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "file_edit".to_string(),
                        arguments: r#"{"path":"/tmp/syscity-wg-e2e.txt","old_string":"old","new_string":"new"}"#
                            .to_string(),
                    },
                    index: None,
                    result: None,
                },
            ]),
            // Blocked: follow the feedback and read first.
            Some(c) if c.contains("has not been read") => {
                ProviderMessage::assistant("Reading first.").with_tool_calls(vec![ToolCall {
                    id: "call_read".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "file_read".to_string(),
                        arguments: r#"{"path":"/tmp/syscity-wg-e2e.txt"}"#.to_string(),
                    },
                    index: None,
                    result: None,
                }])
            }
            // Read done: edit now works.
            Some(c) if c.contains("old text") => {
                ProviderMessage::assistant("Now editing.").with_tool_calls(vec![ToolCall {
                    id: "call_edit".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "file_edit".to_string(),
                        arguments: r#"{"path":"/tmp/syscity-wg-e2e.txt","old_string":"old text","new_string":"new text"}"#
                            .to_string(),
                    },
                    index: None,
                    result: None,
                }])
            }
            Some(_) => ProviderMessage::assistant("Done."),
        }
    })
}

/// The write guard turns a blind edit into corrective feedback; the model
/// recovers by reading first, and the edit then succeeds.
#[tokio::test]
#[serial]
async fn test_write_guard_blocks_blind_edit_and_model_recovers() {
    std::fs::write("/tmp/syscity-wg-e2e.txt", "old text").expect("seed edit target");

    let port = 41250;
    let mock = edit_without_read_mock();
    start_test_gateway_with_mock(port, mock.clone()).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;
    client
        .send_chat(&sid, "Update /tmp/syscity-wg-e2e.txt for me.")
        .await;

    client
        .wait_for_event("chat.final", 30)
        .await
        .expect("Expected chat.final event");

    // The model saw the guard's corrective feedback, then read, then edited.
    let history = mock.history();
    let tool_contents: Vec<&str> = history
        .iter()
        .flat_map(|req| req.messages.iter())
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        tool_contents
            .iter()
            .any(|c| c.contains("has not been read")),
        "guard feedback must reach the model; tool messages: {:?}",
        tool_contents
    );
    assert!(
        tool_contents.iter().any(|c| c.contains("replacement")),
        "edit must succeed after the read"
    );
    assert_eq!(std::fs::read_to_string("/tmp/syscity-wg-e2e.txt").unwrap(), "new text");

    let _ = std::fs::remove_file("/tmp/syscity-wg-e2e.txt");
}
