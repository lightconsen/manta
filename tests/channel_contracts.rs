//! Channel Contract Tests
//!
//! These tests verify that channel message types maintain stable serialization
//! contracts. Any change that breaks JSON serialization/deserialization will
//! fail these tests, signaling a potential breaking change for external
//! integrations (webhooks, channel APIs, etc.).

use syscity::channels::*;

// ── IncomingMessage Serialization Contract ───────────────────────────────────

#[test]
fn incoming_message_serializes_to_expected_shape() {
    let msg = IncomingMessage::new("user_123", "conv_456", "Hello, world!");

    let json = serde_json::to_value(&msg).expect("IncomingMessage must serialize to JSON");

    // Contract: must have these fields
    assert!(json.get("id").is_some(), "missing 'id' field");
    assert!(json.get("user_id").is_some(), "missing 'user_id' field");
    assert!(json.get("conversation_id").is_some(), "missing 'conversation_id' field");
    assert!(json.get("content").is_some(), "missing 'content' field");
    assert!(json.get("attachments").is_some(), "missing 'attachments' field");
    assert!(json.get("metadata").is_some(), "missing 'metadata' field");
    assert!(json.get("provenance").is_some(), "missing 'provenance' field");
    assert!(json.get("mention").is_some(), "missing 'mention' field");

    // Contract: content must be the exact string
    assert_eq!(json["content"], "Hello, world!");
}

#[test]
fn incoming_message_roundtrips_through_json() {
    let original = IncomingMessage::new("alice", "room_1", "test message")
        .with_provenance(InputProvenance::ExternalUser {
            channel: "telegram".to_string(),
            is_direct: true,
        })
        .with_mention(MentionState::Mentioned);

    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: IncomingMessage =
        serde_json::from_str(&json).expect("IncomingMessage must roundtrip through JSON");

    assert_eq!(original.user_id.0, roundtripped.user_id.0);
    assert_eq!(original.conversation_id.0, roundtripped.conversation_id.0);
    assert_eq!(original.content, roundtripped.content);
    assert_eq!(original.mention, roundtripped.mention);
}

#[test]
fn input_provenance_external_user_serializes_correctly() {
    let provenance = InputProvenance::ExternalUser {
        channel: "discord".to_string(),
        is_direct: false,
    };

    let json = serde_json::to_value(&provenance).unwrap();

    // Contract: external variant uses object with 'channel' and 'is_direct'
    assert_eq!(json["ExternalUser"]["channel"], "discord");
    assert_eq!(json["ExternalUser"]["is_direct"], false);
}

#[test]
fn input_provenance_default_is_external_unknown() {
    let default = InputProvenance::default();

    match default {
        InputProvenance::ExternalUser { channel, is_direct } => {
            assert_eq!(channel, "unknown");
            assert!(!is_direct);
        }
        other => panic!("default provenance should be ExternalUser, got {:?}", other),
    }
}

#[test]
fn mention_state_serializes_to_snake_case() {
    let states = vec![
        (MentionState::DirectMessage, "direct_message"),
        (MentionState::Mentioned, "mentioned"),
        (MentionState::NotMentioned, "not_mentioned"),
    ];

    for (state, expected) in states {
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json.as_str().unwrap(), expected);
    }
}

// ── OutgoingMessage Serialization Contract ───────────────────────────────────

#[test]
fn outgoing_message_serializes_to_expected_shape() {
    let msg = OutgoingMessage::new(ConversationId::new("conv_789"), "Response text");

    let json = serde_json::to_value(&msg).expect("OutgoingMessage must serialize to JSON");

    assert!(json.get("conversation_id").is_some(), "missing 'conversation_id' field");
    assert!(json.get("content").is_some(), "missing 'content' field");
    assert!(json.get("formatted_content").is_some(), "missing 'formatted_content' field");
    assert!(json.get("attachments").is_some(), "missing 'attachments' field");
    assert!(json.get("reply_to").is_some(), "missing 'reply_to' field");
    assert!(json.get("options").is_some(), "missing 'options' field");
    assert!(json.get("usage").is_some(), "missing 'usage' field");
}

// NOTE: OutgoingMessage does not implement Deserialize intentionally,
// as it is constructed programmatically, not received from external sources.

// ── Attachment Serialization Contract ────────────────────────────────────────

#[test]
fn attachment_serializes_to_expected_shape() {
    let attachment =
        Attachment::new("doc.pdf", "application/pdf").with_data(vec![0x25, 0x50, 0x44, 0x46]);

    let json = serde_json::to_value(&attachment).unwrap();

    assert!(json.get("id").is_some(), "missing 'id' field");
    assert!(json.get("filename").is_some(), "missing 'filename' field");
    assert!(json.get("content_type").is_some(), "missing 'content_type' field");
    assert!(json.get("size").is_some(), "missing 'size' field");
    assert!(json.get("data").is_some(), "missing 'data' field");
    assert!(json.get("url").is_some(), "missing 'url' field");

    assert_eq!(json["filename"], "doc.pdf");
    assert_eq!(json["content_type"], "application/pdf");
    assert_eq!(json["size"], 4);
}

#[test]
fn attachment_roundtrips_through_json() {
    let original =
        Attachment::new("image.png", "image/png").with_data(vec![0x89, 0x50, 0x4E, 0x47]);

    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: Attachment =
        serde_json::from_str(&json).expect("Attachment must roundtrip through JSON");

    assert_eq!(original.filename, roundtripped.filename);
    assert_eq!(original.content_type, roundtripped.content_type);
    assert_eq!(original.size, roundtripped.size);
    assert_eq!(original.data, roundtripped.data);
}

// ── ChannelType Serialization Contract ───────────────────────────────────────

#[test]
fn channel_type_serializes_to_snake_case() {
    let cases = vec![
        (ChannelType::Whatsapp, "whatsapp"),
        (ChannelType::Telegram, "telegram"),
        (ChannelType::Feishu, "feishu"),
        (ChannelType::Qq, "qq"),
        (ChannelType::Discord, "discord"),
        (ChannelType::Slack, "slack"),
        (ChannelType::Websocket, "websocket"),
        (ChannelType::WebTerminal, "web_terminal"),
    ];

    for (ty, expected) in cases {
        let json = serde_json::to_value(&ty).unwrap();
        assert_eq!(
            json.as_str().unwrap(),
            expected,
            "ChannelType::{:?} should serialize to '{}'",
            ty,
            expected
        );
    }
}

#[test]
fn channel_type_roundtrips_all_variants() {
    let variants = vec![
        ChannelType::Whatsapp,
        ChannelType::Telegram,
        ChannelType::Feishu,
        ChannelType::Qq,
        ChannelType::Discord,
        ChannelType::Slack,
        ChannelType::Websocket,
        ChannelType::WebTerminal,
    ];

    for original in variants {
        let json = serde_json::to_string(&original).unwrap();
        let roundtripped: ChannelType = serde_json::from_str(&json)
            .expect(&format!("ChannelType::{:?} must roundtrip", original));
        assert_eq!(original, roundtripped);
    }
}

// ── MessageMetadata Serialization Contract ───────────────────────────────────

#[test]
fn message_metadata_includes_timestamp_and_extra() {
    let meta = MessageMetadata::new()
        .with_extra("bot_version", "1.0.0")
        .with_extra("source_ip", "127.0.0.1");

    let json = serde_json::to_value(&meta).unwrap();

    assert!(json.get("timestamp").is_some(), "missing 'timestamp' field");
    assert_eq!(json["bot_version"], "1.0.0");
    assert_eq!(json["source_ip"], "127.0.0.1");
}

// ── MessageOptions Serialization Contract ────────────────────────────────────

#[test]
fn message_options_default_contract() {
    let options = MessageOptions::default();

    let json = serde_json::to_value(&options).unwrap();

    assert_eq!(json["silent"], false, "default silent should be false");
    assert_eq!(json["show_typing"], false, "default show_typing should be false");
    assert!(json["custom"].is_object(), "custom should be an object");
}

// ── Cross-type Integration Contract ──────────────────────────────────────────

#[test]
fn full_message_flow_serialization_contract() {
    // Simulate a realistic message flow from webhook to agent to response
    let incoming = IncomingMessage::new("user_42", "conv_99", "/help")
        .with_provenance(InputProvenance::ExternalUser {
            channel: "telegram".to_string(),
            is_direct: true,
        })
        .with_attachment(
            Attachment::new("screenshot.png", "image/png").with_data(vec![0x89, 0x50, 0x4E, 0x47]),
        );

    let incoming_json = serde_json::to_string(&incoming).unwrap();

    // Simulate processing and creating outgoing
    let outgoing =
        OutgoingMessage::new(incoming.conversation_id.clone(), "Here is the help you requested...");

    let outgoing_json = serde_json::to_string(&outgoing).unwrap();

    // Incoming must be valid JSON and deserializable
    let _: IncomingMessage = serde_json::from_str(&incoming_json).unwrap();

    // Both must produce valid JSON
    let incoming_value: serde_json::Value = serde_json::from_str(&incoming_json).unwrap();
    let outgoing_value: serde_json::Value = serde_json::from_str(&outgoing_json).unwrap();

    // Conversation ID must be preserved across the flow
    assert_eq!(incoming_value["conversation_id"], outgoing_value["conversation_id"]);
}
