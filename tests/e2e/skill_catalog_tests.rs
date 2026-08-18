use super::*;

/// Mock provider that loads the weather skill body via the `skill` tool,
/// then answers.
fn skill_loader_mock_provider() -> MockProvider {
    MockProvider::new().with_callback(|messages| {
        // Cache-check prompt is a single user message asking about caching.
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }
        if messages.iter().any(|m| m.role == Role::Tool) {
            return ProviderMessage::assistant("Done.");
        }
        ProviderMessage::assistant("Let me load the weather skill.").with_tool_calls(vec![
            ToolCall {
                id: "call_skill_1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "skill".to_string(),
                    arguments: r#"{"name":"weather"}"#.to_string(),
                },
                index: None,
                result: None,
            },
        ])
    })
}

/// The system prompt must carry only the skills catalog (name + description),
/// never skill bodies; the model loads a body on demand via the `skill` tool.
#[tokio::test]
#[serial]
async fn test_skills_catalog_stable_and_body_on_demand() {
    let port = 41230;
    let mock = skill_loader_mock_provider();
    start_test_gateway_with_mock(port, mock.clone()).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;
    client.send_chat(&sid, "weather in Shanghai?").await;

    client
        .wait_for_event("chat.final", 30)
        .await
        .expect("Expected chat.final event");

    let history = mock.history();
    assert!(history.len() >= 2, "expected at least two model requests");

    // System prompts belonging to the chat context (the title-generation
    // call has its own minimal system prompt and is excluded).
    let chat_prompts: Vec<&str> = history
        .iter()
        .flat_map(|req| req.messages.iter())
        .filter(|m| m.role == Role::System && m.content.contains("# Syscity AI Assistant"))
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        chat_prompts.len() >= 2,
        "expected initial + continuation chat prompts, got {}",
        chat_prompts.len()
    );

    for sp in &chat_prompts {
        assert!(sp.contains("## Available Skills"), "catalog header missing");
        assert!(sp.contains("**weather**"), "catalog row missing");
        assert!(sp.contains("Get weather information"), "catalog description missing");
        assert!(
            !sp.contains("# Weather Skill"),
            "skill body must not be inlined into the prompt"
        );
    }

    // The catalog makes the prompt prefix byte-identical across requests —
    // this is what keeps provider prompt caches effective.
    assert!(
        chat_prompts.windows(2).all(|w| w[0] == w[1]),
        "chat system prompts must be identical across requests"
    );

    // The tool message in the continuation request carries the full body.
    let tool_contents: Vec<&str> = history
        .iter()
        .flat_map(|req| req.messages.iter())
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        tool_contents.iter().any(|c| c.contains("# Weather Skill")),
        "skill tool must return the full body; got: {:?}",
        tool_contents
    );
}
