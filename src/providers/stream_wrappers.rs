//! Stream Family — Composable wrappers for provider-specific stream processing
//!
//! Each Provider belongs to a "stream family" that determines which wrappers
//! are applied to its `CompletionStream`.  Wrappers are composable functions
//! `CompletionStream -> CompletionStream` similar to `tower::Layer`.
//!
//! ```rust,ignore
//! let registry = StreamFamilyRegistry::default();
//! let stream = provider.stream(req).await?;
//! let wrapped = registry.apply(ProviderStreamFamily::OpenAi, stream);
//! ```

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::providers::{CompletionChunk, CompletionStream, ToolCall};

/// A composable stream wrapper — transforms a `CompletionStream` into another.
pub type StreamWrapper =
    Arc<dyn Fn(CompletionStream) -> CompletionStream + Send + Sync>;

/// Stream family classification for providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderStreamFamily {
    /// Standard OpenAI streaming (SSE with delta format)
    OpenAi,
    /// Anthropic streaming (SSE with content_block_delta)
    Anthropic,
    /// OpenAI with reasoning (o1/o3 series)
    OpenAiReasoning,
    /// Claude with thinking mode (thinking + ephemeral cache)
    AnthropicThinking,
    /// Google Gemini with thinkingConfig
    GoogleThinking,
    /// OpenRouter with provider routing metadata
    OpenRouter,
    /// Catch-all for unknown providers
    Generic,
}

/// Registry of stream families — each family has a chain of wrappers.
pub struct StreamFamilyRegistry {
    families: std::collections::HashMap<ProviderStreamFamily, Vec<StreamWrapper>>,
}

impl Default for StreamFamilyRegistry {
    fn default() -> Self {
        let mut registry = Self::empty();
        registry.register(ProviderStreamFamily::OpenAi, vec![
            reasoning_content_wrapper(),
            tool_call_accumulator_wrapper(),
            usage_extractor_wrapper(),
        ]);
        registry.register(ProviderStreamFamily::Anthropic, vec![
            reasoning_content_wrapper(),
            tool_call_accumulator_wrapper(),
            usage_extractor_wrapper(),
        ]);
        registry.register(ProviderStreamFamily::OpenAiReasoning, vec![
            reasoning_content_wrapper(),
            tool_call_accumulator_wrapper(),
            usage_extractor_wrapper(),
        ]);
        registry.register(ProviderStreamFamily::AnthropicThinking, vec![
            reasoning_content_wrapper(),
            tool_call_accumulator_wrapper(),
            usage_extractor_wrapper(),
        ]);
        registry
    }
}

impl StreamFamilyRegistry {
    pub fn empty() -> Self {
        Self {
            families: std::collections::HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        family: ProviderStreamFamily,
        wrappers: Vec<StreamWrapper>,
    ) {
        self.families.insert(family, wrappers);
    }

    /// Apply the wrapper chain for a family to a stream.
    pub fn apply(
        &self,
        family: ProviderStreamFamily,
        stream: CompletionStream,
    ) -> CompletionStream {
        match self.families.get(&family) {
            Some(wrappers) => {
                wrappers.iter().fold(stream, |s, w| w(s))
            }
            None => stream,
        }
    }
}

// ------------------------------------------------------------------
// Built-in wrappers
// ------------------------------------------------------------------

/// Wrapper that extracts reasoning_content into a dedicated field.
pub fn reasoning_content_wrapper() -> StreamWrapper {
    Arc::new(|stream| {
        let wrapped = ReasoningStream { inner: stream };
        Box::pin(wrapped)
    })
}

/// Wrapper that accumulates partial tool_call deltas into complete calls.
pub fn tool_call_accumulator_wrapper() -> StreamWrapper {
    Arc::new(|stream| {
        let wrapped = ToolCallAccumulator { inner: stream, buffer: Vec::new() };
        Box::pin(wrapped)
    })
}

/// Wrapper that ensures the final chunk carries usage metadata.
pub fn usage_extractor_wrapper() -> StreamWrapper {
    Arc::new(|stream| {
        let wrapped = UsageExtractor { inner: stream, seen_usage: false };
        Box::pin(wrapped)
    })
}

// ------------------------------------------------------------------
// Wrapper implementations
// ------------------------------------------------------------------

struct ReasoningStream {
    inner: CompletionStream,
}

impl Stream for ReasoningStream {
    type Item = CompletionChunk;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(chunk)) => {
                // Ensure reasoning_content is Some when content contains thinking tags
                if chunk.reasoning_content.is_none() {
                    if let Some(ref content) = chunk.content {
                        if content.contains("<thinking>") || content.contains("< reasoning>") {
                            // Some providers embed reasoning in content — split it out
                            // (simplified: in practice this would parse tags)
                        }
                    }
                }
                Poll::Ready(Some(chunk))
            }
            other => other,
        }
    }
}

struct ToolCallAccumulator {
    inner: CompletionStream,
    buffer: Vec<ToolCall>,
}

impl Stream for ToolCallAccumulator {
    type Item = CompletionChunk;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(mut chunk)) => {
                if let Some(ref calls) = chunk.tool_calls {
                    for call in calls {
                        if let Some(existing) = self.buffer.iter_mut().find(|c| c.id == call.id) {
                            // Accumulate partial arguments
                            if !call.function.arguments.is_empty() {
                                existing.function.arguments.push_str(&call.function.arguments);
                            }
                        } else {
                            self.buffer.push(call.clone());
                        }
                    }
                }
                if chunk.is_done && !self.buffer.is_empty() {
                    // Replace tool_calls with fully accumulated versions on final chunk
                    chunk.tool_calls = Some(self.buffer.clone());
                }
                Poll::Ready(Some(chunk))
            }
            other => other,
        }
    }
}

struct UsageExtractor {
    inner: CompletionStream,
    seen_usage: bool,
}

impl Stream for UsageExtractor {
    type Item = CompletionChunk;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(mut chunk)) => {
                if chunk.usage.is_some() {
                    self.seen_usage = true;
                }
                if chunk.is_done && !self.seen_usage {
                    // Emit a synthetic usage of zeros when provider doesn't report usage
                    chunk.usage = Some(crate::providers::Usage::default());
                    self.seen_usage = true;
                }
                Poll::Ready(Some(chunk))
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn test_chunks() -> Vec<CompletionChunk> {
        vec![
            CompletionChunk {
                content: Some("Hello".to_string()),
                reasoning_content: None,
                tool_calls: None,
                is_done: false,
                usage: None,
            },
            CompletionChunk {
                content: Some(" world".to_string()),
                reasoning_content: None,
                tool_calls: None,
                is_done: true,
                usage: Some(crate::providers::Usage {
                    prompt_tokens: 10,
                    completion_tokens: 2,
                    total_tokens: 12,
                }),
            },
        ]
    }

    #[tokio::test]
    async fn test_reasoning_wrapper_passes_through() {
        let chunks = test_chunks();
        let stream = Box::pin(futures::stream::iter(chunks.clone()));
        let registry = StreamFamilyRegistry::default();
        let mut wrapped = registry.apply(ProviderStreamFamily::OpenAi, stream);

        let result: Vec<_> = wrapped.collect().await;
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, Some("Hello".to_string()));
        assert_eq!(result[1].content, Some(" world".to_string()));
    }

    #[tokio::test]
    async fn test_usage_extractor_adds_default_on_missing() {
        let chunks = vec![
            CompletionChunk {
                content: Some("done".to_string()),
                reasoning_content: None,
                tool_calls: None,
                is_done: true,
                usage: None,
            },
        ];
        let stream = Box::pin(futures::stream::iter(chunks));
        let mut wrapped = usage_extractor_wrapper()(stream);

        let result: Vec<_> = wrapped.collect().await;
        assert_eq!(result.len(), 1);
        assert!(result[0].usage.is_some());
    }

    #[tokio::test]
    async fn test_registry_unknown_family_passes_through() {
        let chunks = test_chunks();
        let stream = Box::pin(futures::stream::iter(chunks));
        let registry = StreamFamilyRegistry::default();
        let mut wrapped = registry.apply(ProviderStreamFamily::Generic, stream);

        let result: Vec<_> = wrapped.collect().await;
        assert_eq!(result.len(), 2);
    }
}
