//! Channel Inbound Tests
//!
//! Tests for channel access control (DM policy, pairing, allowlist)
//! across Slack, WhatsApp, QQ, and Lark/Feishu channels.
//!
//! These tests verify that inbound message handling correctly applies
//! security policies without requiring live network connections.

use manta::channels::lark::{LarkChannel, LarkConfig};
use manta::channels::qq::{QqChannel, QqConfig};
use manta::channels::slack::{SlackChannel, SlackConfig};
use manta::channels::whatsapp::{WhatsappChannel, WhatsappConfig};
use manta::security::pairing::{DmPolicy, PairingStore};
use std::sync::Arc;

// ── Slack Channel Access Control ─────────────────────────────────────────────

#[tokio::test]
async fn slack_open_policy_allows_any_user() {
    let channel = SlackChannel::new(SlackConfig::new("xoxb-test"));
    channel.set_dm_policy(DmPolicy::Open).await;

    let (allowed, reply) = channel.check_access("U123", Some("Alice")).await;

    assert!(allowed, "Open policy should allow any user");
    assert!(reply.is_none(), "Open policy should not return a reply");
}

#[tokio::test]
async fn slack_allowlist_blocks_unauthorized_user() {
    let channel = SlackChannel::new(SlackConfig::new("xoxb-test"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["U123".to_string()]).await;

    let (allowed, reply) = channel.check_access("U999", Some("Eve")).await;

    assert!(!allowed, "Allowlist should block unauthorized user");
    assert!(reply.is_some(), "Should return a rejection message");
}

#[tokio::test]
async fn slack_allowlist_allows_authorized_user() {
    let channel = SlackChannel::new(SlackConfig::new("xoxb-test"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["U123".to_string()]).await;

    let (allowed, reply) = channel.check_access("U123", Some("Alice")).await;

    assert!(allowed, "Allowlist should allow authorized user");
    assert!(reply.is_none(), "Allowed user should not get a rejection message");
}

#[tokio::test]
async fn slack_pairing_requires_store() {
    let channel = SlackChannel::new(SlackConfig::new("xoxb-test"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    // No pairing store set

    let (allowed, reply) = channel.check_access("U123", Some("Alice")).await;

    assert!(!allowed, "Pairing without store should deny access");
    assert!(reply.is_some(), "Should return an error message");
}

#[tokio::test]
async fn slack_pairing_new_user_gets_code() {
    let channel = SlackChannel::new(SlackConfig::new("xoxb-test"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(Arc::new(PairingStore::new())).await;

    let (allowed, reply) = channel.check_access("U123", Some("Alice")).await;

    assert!(!allowed, "New user should not be allowed immediately");
    let reply_text = reply.expect("Should return a pairing message");
    assert!(
        reply_text.contains("pairing code") || reply_text.contains("Access requested"),
        "Reply should contain pairing info: {}",
        reply_text
    );
}

#[tokio::test]
async fn slack_pairing_authorized_user_allowed() {
    let store = Arc::new(PairingStore::new());
    let channel = SlackChannel::new(SlackConfig::new("xoxb-test"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(store.clone()).await;

    // Request access first
    let result = store.request_access("slack", "U123", Some("Alice")).await;
    let code = match result.unwrap() {
        manta::security::pairing::RequestAccessResult::NewRequest { code } => code,
        other => panic!("Expected NewRequest, got {:?}", other),
    };

    // Approve the request
    store.approve("slack", &code, Some("admin")).await;

    // Now the user should be authorized
    let (allowed, reply) = channel.check_access("U123", Some("Alice")).await;
    assert!(allowed, "Approved user should be allowed");
    assert!(reply.is_none());
}

// ── WhatsApp Channel Access Control ──────────────────────────────────────────

#[tokio::test]
async fn whatsapp_open_policy_allows_any_number() {
    let channel = WhatsappChannel::new(WhatsappConfig::new("123456", "token"));
    channel.set_dm_policy(DmPolicy::Open).await;

    let (allowed, reply) = channel.check_access("+14155552671", Some("Bob")).await;

    assert!(allowed);
    assert!(reply.is_none());
}

#[tokio::test]
async fn whatsapp_allowlist_blocks_unauthorized_number() {
    let channel = WhatsappChannel::new(WhatsappConfig::new("123456", "token"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["+14155552671".to_string()]).await;

    let (allowed, reply) = channel.check_access("+9999999999", None).await;

    assert!(!allowed);
    assert!(reply.is_some());
}

#[tokio::test]
async fn whatsapp_allowlist_allows_authorized_number() {
    let channel = WhatsappChannel::new(WhatsappConfig::new("123456", "token"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["+14155552671".to_string()]).await;

    let (allowed, reply) = channel.check_access("+14155552671", None).await;

    assert!(allowed);
    assert!(reply.is_none());
}

#[tokio::test]
async fn whatsapp_allowlist_normalizes_plus_prefix() {
    let channel = WhatsappChannel::new(WhatsappConfig::new("123456", "token"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel
        .set_allow_from(vec!["14155552671".to_string()])
        .await;

    // Store has + prefix, allowlist doesn't — should still match
    let (allowed, _reply) = channel.check_access("+14155552671", None).await;

    assert!(
        allowed,
        "Should match number regardless of + prefix normalization"
    );
}

#[tokio::test]
async fn whatsapp_pairing_new_user_gets_code() {
    let channel = WhatsappChannel::new(WhatsappConfig::new("123456", "token"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(Arc::new(PairingStore::new())).await;

    let (allowed, reply) = channel.check_access("+14155552671", Some("Bob")).await;

    assert!(!allowed);
    let reply_text = reply.expect("Should return a pairing message");
    assert!(
        reply_text.contains("pairing") || reply_text.contains("pending"),
        "Reply should contain pairing info: {}",
        reply_text
    );
}

#[tokio::test]
async fn whatsapp_pairing_authorized_user_allowed() {
    let store = Arc::new(PairingStore::new());
    let channel = WhatsappChannel::new(WhatsappConfig::new("123456", "token"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(store.clone()).await;

    let result = store.request_access("whatsapp", "14155552671", Some("Bob")).await;
    let code = match result.unwrap() {
        manta::security::pairing::RequestAccessResult::NewRequest { code } => code,
        other => panic!("Expected NewRequest, got {:?}", other),
    };

    store.approve("whatsapp", &code, Some("admin")).await;

    let (allowed, reply) = channel.check_access("+14155552671", Some("Bob")).await;
    assert!(allowed);
    assert!(reply.is_none());
}

// ── QQ Channel Access Control ────────────────────────────────────────────────

#[tokio::test]
async fn qq_open_policy_allows_any_user() {
    let channel = QqChannel::new(QqConfig::new("app_1", "secret", "12345"));
    channel.set_dm_policy(DmPolicy::Open).await;

    let (allowed, reply) = channel.check_access("12345", Some("User")).await;

    assert!(allowed);
    assert!(reply.is_none());
}

#[tokio::test]
async fn qq_allowlist_blocks_unauthorized_user() {
    let channel = QqChannel::new(QqConfig::new("app_1", "secret", "12345"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["12345".to_string()]).await;

    let (allowed, reply) = channel.check_access("99999", Some("Eve")).await;

    assert!(!allowed);
    assert!(reply.is_some());
}

#[tokio::test]
async fn qq_allowlist_allows_authorized_user() {
    let channel = QqChannel::new(QqConfig::new("app_1", "secret", "12345"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["12345".to_string()]).await;

    let (allowed, reply) = channel.check_access("12345", Some("User")).await;

    assert!(allowed);
    assert!(reply.is_none());
}

#[tokio::test]
async fn qq_pairing_new_user_gets_code() {
    let channel = QqChannel::new(QqConfig::new("app_1", "secret", "12345"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(Arc::new(PairingStore::new())).await;

    let (allowed, reply) = channel.check_access("12345", Some("User")).await;

    assert!(!allowed);
    assert!(reply.is_some());
}

#[tokio::test]
async fn qq_pairing_authorized_user_allowed() {
    let store = Arc::new(PairingStore::new());
    let channel = QqChannel::new(QqConfig::new("app_1", "secret", "12345"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(store.clone()).await;

    let result = store.request_access("qq", "12345", Some("User")).await;
    let code = match result.unwrap() {
        manta::security::pairing::RequestAccessResult::NewRequest { code } => code,
        other => panic!("Expected NewRequest, got {:?}", other),
    };

    store.approve("qq", &code, Some("admin")).await;

    let (allowed, reply) = channel.check_access("12345", Some("User")).await;
    assert!(allowed);
    assert!(reply.is_none());
}

// ── Lark/Feishu Channel Access Control ───────────────────────────────────────

#[tokio::test]
async fn lark_open_policy_allows_any_user() {
    let channel = LarkChannel::new(LarkConfig::new("app_1", "secret"));
    channel.set_dm_policy(DmPolicy::Open).await;

    let (allowed, reply) = channel.check_access("user_1", Some("User")).await;

    assert!(allowed);
    assert!(reply.is_none());
}

#[tokio::test]
async fn lark_allowlist_blocks_unauthorized_user() {
    let channel = LarkChannel::new(LarkConfig::new("app_1", "secret"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["user_1".to_string()]).await;

    let (allowed, reply) = channel.check_access("user_2", Some("Eve")).await;

    assert!(!allowed);
    assert!(reply.is_some());
    let text = reply.unwrap();
    assert!(text.contains("无权") || text.contains("authorized"), "Reply should indicate unauthorized: {}", text);
}

#[tokio::test]
async fn lark_allowlist_allows_authorized_user() {
    let channel = LarkChannel::new(LarkConfig::new("app_1", "secret"));
    channel.set_dm_policy(DmPolicy::Allowlist).await;
    channel.set_allow_from(vec!["user_1".to_string()]).await;

    let (allowed, reply) = channel.check_access("user_1", Some("User")).await;

    assert!(allowed);
    assert!(reply.is_none());
}

#[tokio::test]
async fn lark_pairing_new_user_gets_code() {
    let channel = LarkChannel::new(LarkConfig::new("app_1", "secret"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(Arc::new(PairingStore::new())).await;

    let (allowed, reply) = channel.check_access("user_1", Some("User")).await;

    assert!(!allowed);
    assert!(reply.is_some());
    let text = reply.unwrap();
    assert!(
        text.contains("配对码") || text.contains("pairing") || text.contains("审批"),
        "Reply should contain pairing info: {}",
        text
    );
}

#[tokio::test]
async fn lark_pairing_authorized_user_allowed() {
    let store = Arc::new(PairingStore::new());
    let channel = LarkChannel::new(LarkConfig::new("app_1", "secret"));
    channel.set_dm_policy(DmPolicy::Pairing).await;
    channel.set_pairing_store(store.clone()).await;

    let result = store.request_access("lark", "user_1", Some("User")).await;
    let code = match result.unwrap() {
        manta::security::pairing::RequestAccessResult::NewRequest { code } => code,
        other => panic!("Expected NewRequest, got {:?}", other),
    };

    store.approve("lark", &code, Some("admin")).await;

    let (allowed, reply) = channel.check_access("user_1", Some("User")).await;
    assert!(allowed);
    assert!(reply.is_none());
}

// ── Cross-Channel Policy Consistency ─────────────────────────────────────────

#[tokio::test]
async fn all_channels_default_to_open_policy() {
    let slack = SlackChannel::new(SlackConfig::new("xoxb-test"));
    let whatsapp = WhatsappChannel::new(WhatsappConfig::new("123", "token"));
    let qq = QqChannel::new(QqConfig::new("app", "secret", "123"));
    let lark = LarkChannel::new(LarkConfig::new("app", "secret"));

    let (s, _) = slack.check_access("user", None).await;
    let (w, _) = whatsapp.check_access("+123", None).await;
    let (q, _) = qq.check_access("123", None).await;
    let (l, _) = lark.check_access("user", None).await;

    assert!(s, "Slack default should be Open");
    assert!(w, "WhatsApp default should be Open");
    assert!(q, "QQ default should be Open");
    assert!(l, "Lark default should be Open");
}

#[tokio::test]
async fn all_channels_support_pairing_flow() {
    let store = Arc::new(PairingStore::new());

    let slack = SlackChannel::new(SlackConfig::new("xoxb-test"));
    slack.set_dm_policy(DmPolicy::Pairing).await;
    slack.set_pairing_store(store.clone()).await;

    let whatsapp = WhatsappChannel::new(WhatsappConfig::new("123", "token"));
    whatsapp.set_dm_policy(DmPolicy::Pairing).await;
    whatsapp.set_pairing_store(store.clone()).await;

    let qq = QqChannel::new(QqConfig::new("app", "secret", "123"));
    qq.set_dm_policy(DmPolicy::Pairing).await;
    qq.set_pairing_store(store.clone()).await;

    let lark = LarkChannel::new(LarkConfig::new("app", "secret"));
    lark.set_dm_policy(DmPolicy::Pairing).await;
    lark.set_pairing_store(store.clone()).await;

    // New users should all be denied and get a code
    let (s, s_reply) = slack.check_access("U1", None).await;
    let (w, w_reply) = whatsapp.check_access("+1", None).await;
    let (q, q_reply) = qq.check_access("1", None).await;
    let (l, l_reply) = lark.check_access("u1", None).await;

    assert!(!s && s_reply.is_some(), "Slack pairing should deny new user");
    assert!(!w && w_reply.is_some(), "WhatsApp pairing should deny new user");
    assert!(!q && q_reply.is_some(), "QQ pairing should deny new user");
    assert!(!l && l_reply.is_some(), "Lark pairing should deny new user");
}
