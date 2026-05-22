use super::*;

#[tokio::test]
#[serial]
async fn agents_list_returns_array() {
    let port = 40020;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("agents.list", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "agents.list failed: {:?}",
        resp.get("error")
    );
    let agents = resp_payload(&resp)
        .unwrap()
        .get("agents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        agents.is_empty() || agents.iter().all(|a| a.is_string()),
        "Expected array of agent IDs, got: {:?}",
        agents
    );
}

#[tokio::test]
#[serial]
async fn agents_get_returns_not_found_for_unknown() {
    let port = 40021;
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client
        .request("agents.get", json!({"agent_id": "nonexistent-agent-12345"}))
        .await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(false),
        "Expected error for unknown agent, got: {:?}",
        resp
    );
    let error = resp.get("error").unwrap();
    assert_eq!(error.get("code").and_then(|v| v.as_str()), Some("AGENT_NOT_FOUND"));
}
