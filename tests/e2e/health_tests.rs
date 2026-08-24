use super::*;

#[tokio::test]
#[serial]
async fn health_returns_healthy() {
    let port = free_port();
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("health", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "health failed: {:?}",
        resp.get("error")
    );
    let payload = resp_payload(&resp).unwrap();
    assert_eq!(payload.get("status").and_then(|v| v.as_str()), Some("healthy"));
}

#[tokio::test]
#[serial]
async fn system_presence_returns_online() {
    let port = free_port();
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("system.presence", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "system.presence failed: {:?}",
        resp.get("error")
    );
    let payload = resp_payload(&resp).unwrap();
    assert_eq!(payload.get("online").and_then(|v| v.as_bool()), Some(true));
}

#[tokio::test]
#[serial]
async fn ping_returns_pong() {
    let port = free_port();
    start_test_gateway(port, false).await;
    let mut client = FrontendSimulator::connect(port).await;

    let resp = client.request("ping", json!(null)).await;
    assert!(
        resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        "ping failed: {:?}",
        resp.get("error")
    );
}
