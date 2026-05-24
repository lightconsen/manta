# Providers Module

LLM provider abstractions for OpenAI, Anthropic, and fallback chains.

## Design

- **`Provider` trait** — Core abstraction:
  - `complete()` — non-streaming request
  - `stream()` — streaming response (`CompletionStream`)
  - `supports_tools()`, `max_context()`, `health_check()`
  - `stream_family()` — provider-specific stream wrapper selection
- **`ProviderRegistry`** — Holds named provider instances
- **`CompletionRequest`** — Unified request with messages, tools, temperature, model, vision/tools/reasoning flags, fallback models
- **`CompletionChunk`** — Streaming delta with content, reasoning_content, tool_calls, usage

### Implementations

| Provider | File | Notes |
|----------|------|-------|
| OpenAI | `openai.rs` | Chat Completions API + streaming + tool calling + retry logic |
| Anthropic | `anthropic.rs` | Messages API + streaming + thinking support |
| Fallback | `fallback.rs` | Chains multiple providers with failover logic |

### Stream Wrappers

`stream_wrappers.rs` defines `ProviderStreamFamily` for provider-specific payload adaptations:
- Generic (default)
- OpenAI Responses Defaults
- Anthropic Thinking
- Google Thinking
- Moonshot Thinking
- Minimax Fast Mode

## Key Types

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn default_model(&self) -> &str;
    fn supports_tools(&self) -> bool;
    fn max_context(&self) -> usize;
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
    async fn stream(&self, request: CompletionRequest) -> Result<CompletionStream>;
    async fn health_check(&self) -> Result<bool>;
    fn stream_family(&self) -> ProviderStreamFamily;
}
```

## Missing / TODO

- **Missing**: Auth profile store — only single API key per provider; no multi-profile, no OAuth, no token credential type.
- **Missing**: API key rotation — no automatic failover between multiple keys.
- **Missing**: OAuth 2.0 + PKCE flow for providers that require it.
- **Missing**: Usage tracking — no per-provider token consumption stats or quota windows.
- **Missing**: Model catalog — models are hardcoded; no dynamic discovery or `/v1/models` endpoint.
- **Missing**: Model suppression — no runtime disabling of known-broken models.
- **Missing**: Doctor diagnostic system for provider auth issues.
- **Missing**: Credential state machine (valid/expiring/expired/invalid).
- **Missing**: Profile cooldown mechanism after failures (rate limit, auth, billing).
- **Missing**: Google Gemini, Moonshot, Minimax provider implementations (stream families exist but no providers).
- **Missing**: Local model provider (llama.cpp, ollama).
