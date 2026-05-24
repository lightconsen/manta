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

## Done / Implemented

- **Done**: Auth profile store with SQLite persistence (`AuthProfileStore`) — key state metadata only (failure counts, cooldown, status); raw keys remain in config.
- **Done**: API key rotation with `AuthProfile` / `AuthProfileManager` — automatic failover between multiple keys per provider.
- **Done**: OAuth 2.0 + PKCE initial authorization flow (`pkce.rs`, `oauth_flow.rs`, `oauth_callback.rs`) + CLI `manta provider auth` command.
- **Done**: Differentiated cooldown — `FailureClass::default_backoff_secs()` used per failure type (RateLimit=60s, AuthTemporary=5s, etc.), never below config minimum.
- **Done**: Usage tracking (`ProviderUsageTracker`) — per-provider token consumption, cost estimation, time windows (today/this_hour/this_month), budget enforcement.
- **Done**: Model catalog (`ModelCatalog`) — dynamic registry with `ModelCatalogEntry`, suppression list, `/v1/models` and `/api/v1/models` endpoints.
- **Done**: Doctor diagnostic system (`manta doctor`) — provider health, auth status, circuit state, deprecation warnings, migration hints, plugin extension point.
- **Done**: Stream family wrappers (`ProviderStreamFamily`) — OpenAI Responses, Anthropic Thinking, Google Thinking, Moonshot Thinking, Minimax Fast Mode.

## Missing / TODO

- **Missing**: Credential state machine (valid / expiring / expired / invalid_expires) with explicit reason codes.
- **Missing**: Token auto-refresh for OAuth credentials (`refresh_if_needed` exists but may not be fully wired).
- **Missing**: Model suppression — catalog has suppression list field but runtime disabling of known-broken models may not be fully wired.
- **Missing**: Plugin-extensible provider — allow WASM plugins to register new providers with custom stream families and usage fetchers.
- **Missing**: Smart routing — automatically select model based on request features (vision, tools, reasoning), fallback chains, load balancing.
- **Missing**: Google Gemini, Moonshot, Minimax provider implementations (stream families exist but no providers).
- **Missing**: Local model provider (llama.cpp, ollama).
- **Missing**: Provider usage fetch from remote APIs — `UsageQuota` type exists but no live fetching from provider dashboards (Claude, Gemini, Codex, etc.).
