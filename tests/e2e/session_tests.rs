use super::*;

#[tokio::test]
#[serial]
async fn session_full_lifecycle() {
    let port = free_port();
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sessions = client.list_sessions().await;
    assert!(sessions.is_empty(), "Expected empty session list initially");

    let sid = client.create_session().await;
    assert!(!sid.is_empty(), "Expected non-empty session_id");

    let sessions = client.list_sessions().await;
    assert_eq!(sessions.len(), 1, "Expected 1 session after creation");
    assert_eq!(sessions[0].get("session_id").and_then(|v| v.as_str()), Some(sid.as_str()));

    client.subscribe(vec![sid.clone()]).await;
    client.send_chat(&sid, "Hello session").await;

    tokio::time::sleep(Duration::from_millis(300)).await;

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

    client.unsubscribe(vec![sid.clone()]).await;
    client.delete_session(&sid).await;

    let sessions = client.list_sessions().await;
    assert!(
        sessions
            .iter()
            .all(|s| { s.get("session_id").and_then(|v| v.as_str()) != Some(&sid) }),
        "Deleted session should not appear in list"
    );
}

#[tokio::test]
#[serial]
async fn session_subscribe_unsubscribe() {
    let port = free_port();
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

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

    client.subscribe(vec![sid1.clone(), sid2.clone()]).await;
    client.unsubscribe(vec![sid1.clone()]).await;

    let sessions = client.list_sessions().await;
    assert_eq!(sessions.len(), 2, "Expected 2 sessions, got: {:?}", sessions);
}

#[tokio::test]
#[serial]
async fn session_auto_named_after_first_message() {
    let port = free_port();
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    let resp = client
        .request(
            "chat.send",
            json!({"session_id": sid, "message": "Tell me about the weather today"}),
        )
        .await;
    assert!(resp.get("ok").and_then(|v| v.as_bool()) == Some(true));

    tokio::time::sleep(Duration::from_millis(300)).await;
    let sessions = client.list_sessions().await;

    let session = sessions
        .iter()
        .find(|s| s.get("session_id").and_then(|v| v.as_str()) == Some(&sid));

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
