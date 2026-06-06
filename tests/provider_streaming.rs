//! Provider Streaming Tests
//!
//! These tests verify that OpenAI and Anthropic providers correctly parse
//! Server-Sent Events (SSE) streams. Tests run serially to avoid mock conflicts.

use futures::StreamExt;
use serial_test::serial;
use syscity::providers::{AnthropicProvider, CompletionRequest, Message, OpenAiProvider, Provider};
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

// ── OpenAI Streaming Tests ───────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn openai_provider_streams_text_chunks() {
    let mock_server = MockServer::start().await;

    // SSE format: each event is "data: {...}\n\n"
    let sse_body = concat!(
        "data: {\"id\":\"chatcmpl-stream\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-stream\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-stream\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&mock_server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("Say hello")],
        stream: true,
        ..Default::default()
    };

    let mut stream = provider.stream(request).await.expect("stream should start");

    // Collect all chunks
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk);
    }

    // Concatenate all text content
    let full_text: String = chunks.iter().filter_map(|c| c.content.as_deref()).collect();

    assert_eq!(full_text, "Hello world", "streamed text should concatenate");
    assert!(chunks.iter().any(|c| c.is_done), "should receive done chunk");
}

#[tokio::test]
#[serial]
async fn openai_provider_stream_propagates_errors() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .mount(&mock_server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("Hello")],
        stream: true,
        ..Default::default()
    };

    let result = provider.stream(request).await;
    assert!(result.is_err(), "should return error for 503");
}

// ── Anthropic Streaming Tests ────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn anthropic_provider_streams_text_chunks() {
    let mock_server = MockServer::start().await;

    // Anthropic SSE format uses event types
    let sse_body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01Test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-5-sonnet\",\"stop_reason\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"\
         text\":\"Greetings\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"\
         text\":\" human\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&mock_server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("Greet me")],
        stream: true,
        ..Default::default()
    };

    let mut stream = provider.stream(request).await.expect("stream should start");

    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk);
    }

    // Anthropic parser yields text chunks for content_block_delta events
    let text_chunks: Vec<_> = chunks
        .iter()
        .filter(|c| c.content.is_some())
        .map(|c| c.content.as_deref().unwrap())
        .collect();

    let full_text: String = text_chunks.join("");
    assert_eq!(full_text, "Greetings human", "streamed text should concatenate");
}

#[tokio::test]
#[serial]
async fn anthropic_provider_stream_done_event() {
    let mock_server = MockServer::start().await;

    let sse_body = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"\
         text\":\"Done\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&mock_server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("Test")],
        stream: true,
        ..Default::default()
    };

    let mut stream = provider.stream(request).await.expect("stream should start");

    let mut got_content = false;
    let mut got_done = false;

    while let Some(chunk) = stream.next().await {
        if chunk.content.as_deref() == Some("Done") {
            got_content = true;
        }
        if chunk.is_done {
            got_done = true;
        }
    }

    assert!(got_content, "should receive content chunk");
    assert!(got_done, "should receive done signal");
}

#[tokio::test]
#[serial]
async fn anthropic_provider_stream_propagates_errors() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(529).set_body_string("Overloaded"))
        .mount(&mock_server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", &mock_server.uri()).unwrap();

    let request = CompletionRequest {
        messages: vec![Message::user("Hello")],
        stream: true,
        ..Default::default()
    };

    let result = provider.stream(request).await;
    assert!(result.is_err(), "should return error for 529");
}
