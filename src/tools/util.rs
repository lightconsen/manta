//! Shared helpers: stream consumption and JSON schema construction.

use std::time::Duration;

use serde_json::Value;
use tokio_stream::StreamExt;

use super::{ToolExecutionChunk, ToolExecutionResult};

/// Consume a tool execution stream, accumulating chunks into a
/// [`ToolExecutionResult`] while invoking `on_chunk` for each chunk.
pub(super) async fn consume_stream<S, F, Fut>(
    mut stream: S,
    on_chunk: &mut F,
) -> ToolExecutionResult
where
    S: tokio_stream::Stream<Item = ToolExecutionChunk> + Unpin,
    F: FnMut(ToolExecutionChunk) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let mut output = String::new();
    let mut error_output = String::new();
    let mut data: Option<Value> = None;

    while let Some(chunk) = stream.next().await {
        match chunk {
            ToolExecutionChunk::Output(text) => {
                output.push_str(&text);
                on_chunk(ToolExecutionChunk::Output(text)).await;
            }
            ToolExecutionChunk::Error(text) => {
                error_output.push_str(&text);
                on_chunk(ToolExecutionChunk::Error(text)).await;
            }
            ToolExecutionChunk::Data(value) => {
                let value_clone = value.clone();
                data = Some(value);
                on_chunk(ToolExecutionChunk::Data(value_clone)).await;
            }
            ToolExecutionChunk::Done => {
                on_chunk(ToolExecutionChunk::Done).await;
            }
        }
    }

    let success = error_output.is_empty();
    let final_output = if error_output.is_empty() {
        output
    } else if output.is_empty() {
        error_output.clone()
    } else {
        format!("{}\nErrors:\n{}", output, error_output)
    };

    ToolExecutionResult {
        success,
        output: final_output,
        error: if success { None } else { Some(error_output) },
        data,
        execution_time: Duration::default(),
    }
}

/// Helper function to create a JSON schema for a tool
pub fn create_schema(
    description: impl Into<String>,
    properties: Value,
    required: Vec<impl Into<String>>,
) -> Value {
    let required: Vec<String> = required.into_iter().map(Into::into).collect();

    serde_json::json!({
        "type": "object",
        "description": description.into(),
        "properties": properties,
        "required": required,
    })
}
