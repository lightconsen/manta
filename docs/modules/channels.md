# Channels Module

Communication interfaces through which users interact with Manta.

## Design

- **`Channel` trait** — Unified interface: `start()`, `stop()`, `send()`, `send_typing()`, `edit_message()`, `delete_message()`, `health_check()`.
- **Feature-gated implementations** — Each channel is behind a Cargo feature:
  - `telegram` — `teloxide`-based bot
  - `discord` — `serenity`-based bot
  - `slack` — Socket Mode bot
  - `whatsapp`, `qq`, `feishu`, `signal`, `imessage`, `webchat`
- **`ChannelRegistry`** — Holds all enabled channels and manages start/stop.
- **Formatter subsystem** — `MessageFormatter` trait with Markdown, HTML, Slack mrkdwn, and Discord embed formatters.
- **Input provenance** — `InputProvenance` tracks message origin (external user, inter-session, internal system) for trust decisions.
- **Mention gating** — `MentionState` controls whether a group message is processed (DM, mentioned, not mentioned).

## Key Types

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> ChannelCapabilities;
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn send(&self, message: OutgoingMessage) -> Result<Id>;
    // ...
}
```

## Missing / TODO

- **✅ Implemented**: Inbound debounce policy — `InboundDebouncer` with configurable `debounce_ms`, LRU eviction, bypass prefixes, per-key buffering. See `src/inbound/debounce.rs:1-302`.
- **📝 Partial**: Advanced mention gating — `MentionState` with `DirectMessage`/`Mentioned`/`NotMentioned` exists (`src/channels/mod.rs:146-169`). Missing: implicit mentions (reply to bot, quoted bot, thread participant).
- **📝 Partial**: Transport stall watchdog — `ChannelHealthMonitor` tracks heartbeats and `Stale`/`Degraded`/`Unhealthy` status (`src/channels/health.rs:1-338`, `src/channels/lifecycle.rs:1-567`). Missing: transport-specific stall detection (e.g., socket read timeout) inside individual channel implementations.
- **❌ Missing**: Channel plugin architecture (WASM-based extensible channels).
- **❌ Missing**: Config adapter with per-account enable/disable/inspect/delete.
- **❌ Missing**: Sender identity validation (E164 format `+\d{3,}`, username rules, multi-field identity).
- **❌ Missing**: Conversation resolution with multi-source fallback (command-provider, focused-binding, inbound-provider, inbound-bundled-artifact, inbound-bundled-plugin, inbound-fallback).
- **❌ Missing**: Thread binding policy with idle timeout (default 24h) and max age, placement hint (current/child), spawn support (subagent/acp).
- **❌ Missing**: Reply prefix template system (dynamic model info in responses).
- **❌ Missing**: Channel-ACP binding integration.
- **❌ Missing**: Command gating for channel messages — access groups, multi-authorizer OR logic, dual authorizer support.
- **❌ Missing**: Account snapshot system — channel account state snapshots with diagnostic display tones (default/muted/success/warn/error).
- **❌ Missing**: Channel allowlists with multi-source matching (account, channel, group dimensions).
- **❌ Missing**: Session envelope context (storePath + previousTimestamp for interval calculation).
