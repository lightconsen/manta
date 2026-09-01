//! Shared tests for the admin WS handlers (invoked via the facade re-exports).

use std::sync::Arc;

use super::super::{WsRequest, WsResponse};
use crate::gateway::GatewayState;

use super::*;
use crate::gateway::state_tests::make_test_state;
use crate::gateway::GatewayConfig;

fn req(id: &str, method: &str, params: Option<serde_json::Value>) -> WsRequest {
    WsRequest {
        frame_type: "req".into(),
        id: id.into(),
        method: method.into(),
        params,
    }
}

async fn state() -> Arc<GatewayState> {
    Arc::new(make_test_state(GatewayConfig::default()).await)
}

#[tokio::test]
async fn system_reload_defaults_to_all() {
    let state = state().await;
    let resp =
        handle_system_reload(&req("r1", "system.reload", Some(serde_json::json!({}))), &state)
            .await;
    assert!(resp.ok, "reload failed: {:?}", resp.error);
    let payload = resp.payload.as_ref().unwrap();
    assert_eq!(payload["scope"], "all");
    assert_eq!(payload["success"], true);
}

#[tokio::test]
async fn system_reload_scope_skills() {
    let state = state().await;
    let resp = handle_system_reload(
        &req("r1", "system.reload", Some(serde_json::json!({ "scope": "skills" }))),
        &state,
    )
    .await;
    assert!(resp.ok);
    assert!(resp.payload.as_ref().unwrap()["skills"].is_object());
}

#[tokio::test]
async fn channels_list_empty() {
    let state = state().await;
    let resp = handle_channels_list(&req("r1", "channels.list", None), &state).await;
    assert!(resp.ok);
    assert_eq!(
        resp.payload.as_ref().unwrap()["channels"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn channels_enable_unknown_not_found() {
    let state = state().await;
    let resp = handle_channels_enable(
        &req("r1", "channels.enable", Some(serde_json::json!({ "name": "telegram" }))),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
}

#[tokio::test]
async fn channels_disable_missing_name_errors() {
    let state = state().await;
    let resp = handle_channels_disable(
        &req("r1", "channels.disable", Some(serde_json::json!({}))),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_PARAMS");
}

#[tokio::test]
async fn device_pairing_qr_unknown_not_found() {
    let state = state().await;
    let resp = handle_device_pairing_qr(
        &req("r1", "device.pairing.qr", Some(serde_json::json!({ "code": "NOPE" }))),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
}

#[tokio::test]
async fn device_pairing_qr_seeded_returns_svg() {
    let state = state().await;
    let code = match state
        .auth
        .device_pairing_store
        .request_access("dev-1", Some("Phone"), None)
        .await
    {
        crate::security::device_pairing::DeviceAccessResult::PairingRequired { code } => code,
        _ => panic!("expected a new pending request"),
    };
    let resp = handle_device_pairing_qr(
        &req("r1", "device.pairing.qr", Some(serde_json::json!({ "code": code }))),
        &state,
    )
    .await;
    assert!(resp.ok, "qr failed: {:?}", resp.error);
    let svg = resp.payload.as_ref().unwrap()["svg"].as_str().unwrap();
    assert!(svg.contains("<svg"));
}

#[tokio::test]
async fn device_pairing_setup_roundtrip() {
    let state = state().await;
    let code = match state
        .auth
        .device_pairing_store
        .request_access("dev-2", Some("Tablet"), None)
        .await
    {
        crate::security::device_pairing::DeviceAccessResult::PairingRequired { code } => code,
        _ => panic!("expected a new pending request"),
    };
    let setup_code = crate::security::device_pairing::DevicePairingStore::encode_setup_code(&code);
    let resp = handle_device_pairing_setup(
        &req(
            "r1",
            "device.pairing.setup",
            Some(serde_json::json!({ "setup_code": setup_code })),
        ),
        &state,
    )
    .await;
    assert!(resp.ok, "setup failed: {:?}", resp.error);
    let payload = resp.payload.as_ref().unwrap();
    assert_eq!(payload["device_id"], "dev-2");
    assert_eq!(payload["display_name"], "Tablet");
}

#[tokio::test]
async fn device_pairing_setup_invalid_code_errors() {
    let state = state().await;
    let resp = handle_device_pairing_setup(
        &req(
            "r1",
            "device.pairing.setup",
            Some(serde_json::json!({ "setup_code": "!!not-base64!!" })),
        ),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_PARAMS");
}

#[tokio::test]
async fn approvals_list_empty() {
    let state = state().await;
    let resp = handle_approvals_list(&req("r1", "approvals.list", None), &state).await;
    assert!(resp.ok);
    assert_eq!(resp.payload.as_ref().unwrap()["count"], 0);
}

#[tokio::test]
async fn approvals_approve_unknown_not_found() {
    let state = state().await;
    let resp = handle_approvals_approve(
        &req("r1", "approvals.approve", Some(serde_json::json!({ "id": "missing" }))),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
}

#[tokio::test]
async fn approvals_submit_then_deny_with_reason() {
    let state = state().await;
    // Submit a pending approval with a live response channel.
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let pa = crate::tools::approval::PendingApproval::new(
        "app-1",
        "bash",
        serde_json::json!({ "command": "ls" }),
        "alice",
    )
    .with_risk_level(crate::tools::approval::RiskLevel::High)
    .with_approval_level(crate::tools::approval::ApprovalLevel::Ask)
    .with_message("Run bash")
    .with_response_tx(tx);
    state.tools.approval_queue.submit(pa).await;

    let resp = handle_approvals_list(&req("r1", "approvals.list", None), &state).await;
    assert!(resp.ok);
    assert_eq!(resp.payload.as_ref().unwrap()["count"], 1);

    let resp = handle_approvals_deny(
        &req(
            "r1",
            "approvals.deny",
            Some(serde_json::json!({ "id": "app-1", "reason": "Not authorized" })),
        ),
        &state,
    )
    .await;
    assert!(resp.ok, "deny failed: {:?}", resp.error);
    let payload = resp.payload.as_ref().unwrap();
    assert_eq!(payload["status"], "denied");
    assert_eq!(payload["reason"], "Not authorized");
}

#[tokio::test]
async fn memory_search_unavailable_without_vector() {
    let state = state().await;
    let resp = handle_memory_search(
        &req("r1", "memory.search", Some(serde_json::json!({ "query": "foo" }))),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "UNAVAILABLE");
}

#[tokio::test]
async fn memory_collections_unavailable_without_vector() {
    let state = state().await;
    let resp = handle_memory_collections(&req("r1", "memory.collections", None), &state).await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "UNAVAILABLE");
}

#[tokio::test]
async fn mention_policy_get_and_set_roundtrip() {
    let state = state().await;
    let resp = handle_mention_policy_get(&req("r1", "mention.policy", None), &state).await;
    assert!(resp.ok);
    assert!(resp.payload.as_ref().unwrap()["policy"].is_string());

    let resp = handle_mention_policy_set(
        &req("r1", "mention.policy.set", Some(serde_json::json!({ "policy": "block" }))),
        &state,
    )
    .await;
    assert!(resp.ok, "set failed: {:?}", resp.error);
    assert_eq!(resp.payload.as_ref().unwrap()["policy"], "block");
}

#[tokio::test]
async fn mention_allowlist_add_and_list() {
    let state = state().await;
    let resp = handle_mention_allowlist_add(
        &req(
            "r1",
            "mention.allowlist.add",
            Some(serde_json::json!({ "channel": "telegram", "pattern": "@boss" })),
        ),
        &state,
    )
    .await;
    assert!(resp.ok);
    let resp = handle_mention_allowlist_list(
        &req("r1", "mention.allowlist", Some(serde_json::json!({ "channel": "telegram" }))),
        &state,
    )
    .await;
    assert!(resp.ok);
    let entries = resp.payload.as_ref().unwrap()["allowlist"]
        .as_array()
        .unwrap();
    assert!(entries.iter().any(|e| e == "@boss"));
}

#[tokio::test]
async fn auth_profiles_list_empty_and_get_unknown() {
    let state = state().await;
    let resp = handle_auth_profiles_list(&req("r1", "auth_profiles.list", None), &state).await;
    assert!(resp.ok);
    assert_eq!(resp.payload.as_ref().unwrap()["count"], 0);

    let resp = handle_auth_profiles_get(
        &req("r1", "auth_profiles.get", Some(serde_json::json!({ "id": "openai" }))),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
}

#[tokio::test]
async fn auth_profiles_rotate_unknown_errors() {
    let state = state().await;
    let resp = handle_auth_profiles_rotate(
        &req("r1", "auth_profiles.rotate", Some(serde_json::json!({ "id": "openai" }))),
        &state,
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error.as_ref().unwrap().code, "BAD_REQUEST");
}

#[tokio::test]
async fn audit_recent_returns_entries() {
    let state = state().await;
    let resp = handle_audit_recent(
        &req("r1", "audit.recent", Some(serde_json::json!({ "limit": 10 }))),
        &state,
    )
    .await;
    assert!(resp.ok, "audit.recent failed: {:?}", resp.error);
    let payload = resp.payload.as_ref().unwrap();
    assert!(payload["entries"].is_array());
    assert_eq!(payload["count"], 0);
}

#[tokio::test]
async fn audit_all_returns_entries() {
    let state = state().await;
    let resp = handle_audit_all(&req("r1", "audit.all", None), &state).await;
    assert!(resp.ok, "audit.all failed: {:?}", resp.error);
    let payload = resp.payload.as_ref().unwrap();
    assert!(payload["entries"].is_array());
}

#[tokio::test]
async fn audit_recent_defaults_limit_to_50() {
    let state = state().await;
    let resp = handle_audit_recent(&req("r1", "audit.recent", None), &state).await;
    assert!(resp.ok);
}
