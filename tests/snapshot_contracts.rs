//! Snapshot Contract Tests
//!
//! These tests use the `insta` crate to capture JSON snapshots of key data
//! structures. Any change that alters serialization output will produce a
//! snapshot diff, making breaking changes immediately visible.

use insta::assert_json_snapshot;
use manta::providers::{Message, Role, ToolCall, ToolResult, Usage};

// ── Message Snapshots ────────────────────────────────────────────────────────

#[test]
fn snapshot_user_message() {
    let msg = Message::user("Hello, world!");
    assert_json_snapshot!(msg, @r###"
    {
      "role": "user",
      "content": "Hello, world!"
    }
    "###);
}

#[test]
fn snapshot_system_message() {
    let msg = Message::system("You are a helpful assistant.");
    assert_json_snapshot!(msg, @r###"
    {
      "role": "system",
      "content": "You are a helpful assistant."
    }
    "###);
}

#[test]
fn snapshot_assistant_message_with_tool_calls() {
    let msg = Message::assistant("I'll run that for you.").with_tool_calls(vec![
        ToolCall {
            id: "call_abc".to_string(),
            call_type: "function".to_string(),
            function: manta::providers::FunctionCall {
                name: "shell".to_string(),
                arguments: "{\"command\":\"ls\"}".to_string(),
            },
        },
    ]);
    assert_json_snapshot!(msg, @r###"
    {
      "role": "assistant",
      "content": "I'll run that for you.",
      "tool_calls": [
        {
          "id": "call_abc",
          "call_type": "function",
          "function": {
            "name": "shell",
            "arguments": "{\"command\":\"ls\"}"
          }
        }
      ]
    }
    "###);
}

// ── ToolResult Snapshots ─────────────────────────────────────────────────────

#[test]
fn snapshot_tool_result_success() {
    let result = ToolResult::success("call_1", "file contents here");
    assert_json_snapshot!(result, @r###"
    {
      "tool_call_id": "call_1",
      "role": "tool",
      "content": "file contents here",
      "is_error": false
    }
    "###);
}

#[test]
fn snapshot_tool_result_error() {
    let result = ToolResult::error("call_2", "Permission denied");
    assert_json_snapshot!(result, @r###"
    {
      "tool_call_id": "call_2",
      "role": "tool",
      "content": "Permission denied",
      "is_error": true
    }
    "###);
}

// ── Usage Snapshots ──────────────────────────────────────────────────────────

#[test]
fn snapshot_usage() {
    let usage = Usage {
        prompt_tokens: 150,
        completion_tokens: 42,
        total_tokens: 192,
    };
    assert_json_snapshot!(usage, @r###"
    {
      "prompt_tokens": 150,
      "completion_tokens": 42,
      "total_tokens": 192
    }
    "###);
}

// ── Role Snapshots ───────────────────────────────────────────────────────────

#[test]
fn snapshot_role_variants() {
    let roles = vec![Role::System, Role::User, Role::Assistant, Role::Tool];
    assert_json_snapshot!(roles, @r###"
    [
      "system",
      "user",
      "assistant",
      "tool"
    ]
    "###);
}
