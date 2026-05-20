//! Provider Contract Tests
//!
//! These tests verify that Provider response types maintain stable JSON
//! serialization contracts. Any change that breaks serialization/deserialization
//! of Message, ToolCall, CompletionResponse, etc. signals a breaking change for
//! LLM integrations.

use manta::providers::*;
use serde_json::json;

// ── Message Serialization Contract ───────────────────────────────────────────

#[test]
fn message_serializes_to_expected_shape() {
    let msg = Message::user("Hello, world!");
    let json = serde_json::to_value(&msg).unwrap();

    assert!(json.get("role").is_some(), "missing 'role' field");
    assert!(json.get("content").is_some(), "missing 'content' field");
    // name is skipped when None
    assert!(!json.get("name").is_some(), "name should be absent when None");
    assert!(!json.get("tool_calls").is_some(), "tool_calls should be absent when None");
    assert!(!json.get("tool_call_id").is_some(), "tool_call_id should be absent when None");

    assert_eq!(json["role"], "user");
    assert_eq!(json["content"], "Hello, world!");
}

#[test]
fn message_roundtrips_all_roles() {
    let cases = vec![
        Message::system("You are a helpful assistant."),
        Message::user("What is Rust?"),
        Message::assistant("Rust is a systems programming language."),
        Message::tool("42", "call_123"),
    ];

    for original in cases {
        let json = serde_json::to_string(&original).unwrap();
        let roundtripped: Message = serde_json::from_str(&json)
            .expect(&format!("Message with role {:?} must roundtrip", original.role));
        assert_eq!(original.role, roundtripped.role);
        assert_eq!(original.content, roundtripped.content);
    }
}

#[test]
fn message_with_name_serializes_correctly() {
    let msg = Message::user_named("alice", "hello");
    let json = serde_json::to_value(&msg).unwrap();

    assert_eq!(json["name"], "alice");
    assert_eq!(json["role"], "user");
}

#[test]
fn message_with_metadata_serializes_to_flat_map() {
    let msg = Message::user("test")
        .with_metadata("key1", "val1")
        .with_metadata("key2", "val2");

    let json = serde_json::to_value(&msg).unwrap();
    let meta = json["metadata"]
        .as_object()
        .expect("metadata must be an object");
    assert_eq!(meta.get("key1").unwrap(), "val1");
    assert_eq!(meta.get("key2").unwrap(), "val2");
}

#[test]
fn message_tool_calls_serializes_to_expected_shape() {
    let tool_call = ToolCall {
        id: "call_abc".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "shell".to_string(),
            arguments: "{\"command\":\"ls\"}".to_string(),
        },
        index: None,
        result: None,
    };
    let msg = Message::assistant("I'll run that for you.").with_tool_calls(vec![tool_call]);

    let json = serde_json::to_value(&msg).unwrap();
    let calls = json["tool_calls"]
        .as_array()
        .expect("tool_calls must be array");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["id"], "call_abc");
    assert_eq!(calls[0]["call_type"], "function");
    assert_eq!(calls[0]["function"]["name"], "shell");
}

// ── Role Serialization Contract ──────────────────────────────────────────────

#[test]
fn role_serializes_to_lowercase() {
    let cases = vec![
        (Role::System, "system"),
        (Role::User, "user"),
        (Role::Assistant, "assistant"),
        (Role::Tool, "tool"),
    ];

    for (role, expected) in cases {
        let json = serde_json::to_value(&role).unwrap();
        assert_eq!(json.as_str().unwrap(), expected, "Role::{:?} serialization mismatch", role);
    }
}

#[test]
fn role_roundtrips_all_variants() {
    for original in [Role::System, Role::User, Role::Assistant, Role::Tool] {
        let json = serde_json::to_string(&original).unwrap();
        let roundtripped: Role =
            serde_json::from_str(&json).expect(&format!("Role::{:?} must roundtrip", original));
        assert_eq!(original, roundtripped);
    }
}

// ── ToolCall Serialization Contract ──────────────────────────────────────────

#[test]
fn tool_call_serializes_to_expected_shape() {
    let call = ToolCall {
        id: "call_1".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "file_read".to_string(),
            arguments: "{\"path\":\"/etc/passwd\"}".to_string(),
        },
        index: None,
        result: None,
    };

    let json = serde_json::to_value(&call).unwrap();
    assert!(json.get("id").is_some(), "missing 'id'");
    assert!(json.get("call_type").is_some(), "missing 'call_type'");
    assert!(json.get("function").is_some(), "missing 'function'");
    assert_eq!(json["function"]["name"], "file_read");
    assert_eq!(json["function"]["arguments"], "{\"path\":\"/etc/passwd\"}");
}

#[test]
fn tool_call_roundtrips_through_json() {
    let original = ToolCall {
        id: "call_999".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "web_search".to_string(),
            arguments: "{\"query\":\"rust async\"}".to_string(),
        },
        index: None,
        result: None,
    };

    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: ToolCall = serde_json::from_str(&json).expect("ToolCall must roundtrip");
    assert_eq!(original.id, roundtripped.id);
    assert_eq!(original.function.name, roundtripped.function.name);
    assert_eq!(original.function.arguments, roundtripped.function.arguments);
}

// ── ToolResult Serialization Contract ────────────────────────────────────────

#[test]
fn tool_result_success_contract() {
    let result = ToolResult::success("call_1", "file contents here");
    let json = serde_json::to_value(&result).unwrap();

    assert_eq!(json["tool_call_id"], "call_1");
    assert_eq!(json["content"], "file contents here");
    assert_eq!(json["is_error"], false);
    assert_eq!(json["role"], "tool");
}

#[test]
fn tool_result_error_contract() {
    let result = ToolResult::error("call_2", "Permission denied");
    let json = serde_json::to_value(&result).unwrap();

    assert_eq!(json["tool_call_id"], "call_2");
    assert_eq!(json["is_error"], true);
    assert_eq!(json["role"], "tool");
}

#[test]
fn tool_result_roundtrips_through_json() {
    let original = ToolResult::success("call_3", "output data");
    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: ToolResult = serde_json::from_str(&json).unwrap();

    assert_eq!(original.tool_call_id, roundtripped.tool_call_id);
    assert_eq!(original.content, roundtripped.content);
    assert_eq!(original.is_error, roundtripped.is_error);
    assert_eq!(original.role, roundtripped.role);
}

// ── Usage Serialization Contract ─────────────────────────────────────────────

#[test]
fn usage_serializes_to_expected_shape() {
    let usage = Usage {
        prompt_tokens: 150,
        completion_tokens: 42,
        total_tokens: 192,
    };

    let json = serde_json::to_value(&usage).unwrap();
    assert_eq!(json["prompt_tokens"], 150);
    assert_eq!(json["completion_tokens"], 42);
    assert_eq!(json["total_tokens"], 192);
}

#[test]
fn usage_roundtrips_through_json() {
    let original = Usage {
        prompt_tokens: 1000,
        completion_tokens: 500,
        total_tokens: 1500,
    };

    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: Usage = serde_json::from_str(&json).unwrap();
    assert_eq!(original.prompt_tokens, roundtripped.prompt_tokens);
    assert_eq!(original.completion_tokens, roundtripped.completion_tokens);
    assert_eq!(original.total_tokens, roundtripped.total_tokens);
}

// ── CompletionResponse Field Contract ────────────────────────────────────────

#[test]
fn completion_response_field_contract() {
    let response = CompletionResponse {
        message: Message::assistant("Hello!"),
        usage: Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 2,
            total_tokens: 12,
        }),
        model: "claude-sonnet-4-6".to_string(),
        finish_reason: Some("stop".to_string()),
    };

    // Verify field-level invariants (not JSON serialization, since
    // CompletionResponse intentionally does not derive Serialize).
    assert_eq!(response.message.content, "Hello!");
    assert_eq!(response.model, "claude-sonnet-4-6");
    assert_eq!(response.finish_reason, Some("stop".to_string()));
    let usage = response.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 2);
    assert_eq!(usage.total_tokens, 12);
}

// ── ToolDefinition / FunctionDefinition Contract ─────────────────────────────

#[test]
fn function_definition_serializes_to_expected_shape() {
    let def = FunctionDefinition {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "input": {"type": "string"}
            },
            "required": ["input"]
        }),
    };

    let json = serde_json::to_value(&def).unwrap();
    assert_eq!(json["name"], "test_tool");
    assert_eq!(json["description"], "A test tool");
    assert_eq!(json["parameters"]["type"], "object");
}

#[test]
fn tool_definition_wraps_function_correctly() {
    let def = ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "shell".to_string(),
            description: "Run shell commands".to_string(),
            parameters: json!({"type": "object"}),
        },
    };

    let json = serde_json::to_value(&def).unwrap();
    assert_eq!(json["type"], "function");
    assert_eq!(json["function"]["name"], "shell");
}

// ── CompletionRequest Default Contract ───────────────────────────────────────

#[test]
fn completion_request_default_contract() {
    let req = CompletionRequest::default();
    assert!(req.messages.is_empty());
    assert!(req.tools.is_none());
    assert_eq!(req.temperature, Some(0.7));
    assert_eq!(req.max_tokens, Some(2048));
    assert!(!req.stream);
    assert!(req.model.is_none());
    assert!(req.stop.is_none());
}

// ── Cross-type Integration Contract ──────────────────────────────────────────

#[test]
fn full_provider_flow_serialization_contract() {
    // Simulate a realistic LLM interaction flow — test serializable parts only.
    // CompletionRequest/CompletionResponse intentionally do not derive Serialize
    // (they are constructed programmatically, not sent over the wire directly).

    // Messages must serialize
    let messages = vec![
        Message::system("You are helpful."),
        Message::user("What is 2+2?"),
    ];
    let msgs_json = serde_json::to_string(&messages).unwrap();
    let msgs_value: serde_json::Value = serde_json::from_str(&msgs_json).unwrap();
    assert!(msgs_value.is_array());
    assert_eq!(msgs_value.as_array().unwrap().len(), 2);

    // Tool definitions must serialize
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "calculator".to_string(),
            description: "Calculate math".to_string(),
            parameters: json!({"type": "object"}),
        },
    }];
    let tools_json = serde_json::to_string(&tools).unwrap();
    let tools_value: serde_json::Value = serde_json::from_str(&tools_json).unwrap();
    assert_eq!(tools_value.as_array().unwrap().len(), 1);
    assert_eq!(tools_value[0]["type"], "function");

    // Response message + usage must serialize
    let resp_message = Message::assistant("2 + 2 = 4");
    let resp_usage = Usage {
        prompt_tokens: 15,
        completion_tokens: 7,
        total_tokens: 22,
    };
    let msg_json = serde_json::to_string(&resp_message).unwrap();
    let msg_value: serde_json::Value = serde_json::from_str(&msg_json).unwrap();
    assert_eq!(msg_value["content"], "2 + 2 = 4");

    let usage_json = serde_json::to_string(&resp_usage).unwrap();
    let usage_value: serde_json::Value = serde_json::from_str(&usage_json).unwrap();
    assert_eq!(usage_value["total_tokens"], 22);
}
