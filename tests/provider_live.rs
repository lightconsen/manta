//! Provider Live Integration Tests
//!
//! These tests use Wiremock to simulate LLM API endpoints, verifying that
//! the OpenAI and Anthropic providers correctly serialize requests and
//! deserialize responses. Tests run serially to avoid mock server conflicts.

use serde_json::json;
use serial_test::serial;
use syscity::providers::{
    AnthropicProvider, CompletionRequest, Message, OpenAiProvider, Provider, Role, ToolDefinition,
};
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

// ── OpenAI Provider Tests ────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn openai_provider_completes_successfully() {
    let mock_server = MockServer::start().await;

    let response_body = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o-mini",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from mock OpenAI!"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 6,
            "total_tokens": 16
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-openai-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-openai-key", &mock_server.uri())
        .expect("create provider");

    let request = CompletionRequest {
        messages: vec![Message::user("Say hello")],
        ..Default::default()
    };

    let response = provider
        .complete(request)
        .await
        .expect("completion should succeed");

    assert_eq!(response.message.role, Role::Assistant);
    assert_eq!(response.message.content, "Hello from mock OpenAI!");
    assert_eq!(response.model, "gpt-4o-mini");
    assert_eq!(response.finish_reason, Some("stop".to_string()));

    let usage = response.usage.expect("usage should be present");
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 6);
    assert_eq!(usage.total_tokens, 16);
}

#[tokio::test]
#[serial]
async fn openai_provider_handles_tool_calls() {
    let mock_server = MockServer::start().await;

    let response_body = json!({
        "id": "chatcmpl-tool",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call_abc",
                            "type": "function",
                            "function": {
                                "name": "shell",
                                "arguments": "{\"command\":\"ls\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }
        ],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 15,
            "total_tokens": 35
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("List files")],
        tools: Some(vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: syscity::providers::FunctionDefinition {
                name: "shell".to_string(),
                description: "Run shell commands".to_string(),
                parameters: json!({"type": "object"}),
            },
        }]),
        ..Default::default()
    };

    let response = provider
        .complete(request)
        .await
        .expect("completion should succeed");

    let tool_calls = response
        .message
        .tool_calls
        .expect("tool_calls should be present");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_abc");
    assert_eq!(tool_calls[0].function.name, "shell");
    assert_eq!(tool_calls[0].function.arguments, "{\"command\":\"ls\"}");
    assert_eq!(response.finish_reason, Some("tool_calls".to_string()));
}

#[tokio::test]
#[serial]
async fn openai_provider_propagates_api_errors() {
    let mock_server = MockServer::start().await;

    let error_body = json!({
        "error": {
            "message": "Invalid API key",
            "type": "invalid_request_error",
            "code": "invalid_api_key"
        }
    });

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body))
        .mount(&mock_server)
        .await;

    let provider = OpenAiProvider::with_base_url("bad-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("Hello")],
        ..Default::default()
    };

    let result = provider.complete(request).await;
    assert!(result.is_err(), "should return error for 401");

    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("401"), "error should mention status code");
}

#[tokio::test]
#[serial]
async fn openai_provider_sends_correct_request_body() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-body",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "OK"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 1, "total_tokens": 6}
        })))
        .mount(&mock_server)
        .await;

    let provider = OpenAiProvider::with_base_url("key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::system("Be helpful"), Message::user("Hi")],
        model: Some("gpt-4o-mini".to_string()),
        temperature: Some(0.5),
        max_tokens: Some(100),
        ..Default::default()
    };

    let response = provider.complete(request).await.expect("should succeed");
    assert_eq!(response.message.content, "OK");
}

// ── Anthropic Provider Tests ─────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn anthropic_provider_completes_successfully() {
    let mock_server = MockServer::start().await;

    let response_body = json!({
        "id": "msg_01TestAnthropic",
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": "Hello from mock Claude!"}],
        "model": "claude-3-5-sonnet-20241022",
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 12, "output_tokens": 7}
    });

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-anthropic-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-anthropic-key", &mock_server.uri())
        .expect("create provider");

    let request = CompletionRequest {
        messages: vec![Message::user("Greet me")],
        ..Default::default()
    };

    let response = provider
        .complete(request)
        .await
        .expect("completion should succeed");

    assert_eq!(response.message.role, Role::Assistant);
    assert_eq!(response.message.content, "Hello from mock Claude!");
    assert_eq!(response.model, "claude-3-5-sonnet-20241022");
    assert_eq!(response.finish_reason, Some("end_turn".to_string()));

    let usage = response.usage.expect("usage should be present");
    assert_eq!(usage.prompt_tokens, 12);
    assert_eq!(usage.completion_tokens, 7);
    assert_eq!(usage.total_tokens, 19);
}

#[tokio::test]
#[serial]
async fn anthropic_provider_handles_system_prompt() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_sys",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Arr!"}],
            "model": "claude-3-5-sonnet-20241022",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 8, "output_tokens": 2}
        })))
        .mount(&mock_server)
        .await;

    let provider = AnthropicProvider::with_base_url("key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::system("You are a pirate"), Message::user("Speak")],
        ..Default::default()
    };

    let response = provider.complete(request).await.expect("should succeed");
    assert_eq!(response.message.content, "Arr!");
}

#[tokio::test]
#[serial]
async fn anthropic_provider_propagates_api_errors() {
    let mock_server = MockServer::start().await;

    let error_body = json!({
        "type": "error",
        "error": {
            "type": "authentication_error",
            "message": "Invalid API key"
        }
    });

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body))
        .mount(&mock_server)
        .await;

    let provider = AnthropicProvider::with_base_url("bad-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("Hello")],
        ..Default::default()
    };

    let result = provider.complete(request).await;
    assert!(result.is_err(), "should return error for 401");

    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("401"), "error should mention status code");
}

// ── Provider Trait Contract via Wiremock ─────────────────────────────────────

#[tokio::test]
#[serial]
async fn provider_trait_name_and_model() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "test",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "custom-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": ""}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&mock_server)
        .await;

    let provider = OpenAiProvider::with_base_url("key", &mock_server.uri())
        .unwrap()
        .with_model("custom-model");

    assert_eq!(provider.name(), "openai");
    assert_eq!(provider.default_model(), "custom-model");
    assert!(provider.supports_tools());
    assert_eq!(provider.max_context(), 4_096); // default for unknown models
}
