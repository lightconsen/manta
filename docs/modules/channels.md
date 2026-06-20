# Channels Module

Communication interfaces through which users interact with Syscity.

## Design

- **`Channel` trait** — Unified interface: `start()`, `stop()`, `send()`, `send_typing()`, `edit_message()`, `delete_message()`, `health_check()`.
- **Feature-gated implementations** — Each channel is behind a Cargo feature:
  - `telegram` — `teloxide`-based bot
  - `discord` — `serenity`-based bot
  - `slack` — Socket Mode bot
  - `whatsapp`, `qq`, `feishu`, `signal`, `imessage`, `webchat`
- **`ChannelRegistry`** — Holds all enabled channels and manages start/stop.
- **Extended registry** — `ExtendedChannelRegistry` supports both native and WASM plugin channels.
- **Formatter subsystem** — `MessageFormatter` trait with Markdown, HTML, Slack mrkdwn, and Discord embed formatters.
- **Input provenance** — `InputProvenance` tracks message origin (external user, inter-session, internal system) for trust decisions.
- **Mention gating** — `MentionState` controls whether a group message is processed (DM, mentioned, not mentioned).
- **Channel management** — `ChannelHealthMonitor`, `LifecycleManager`, `MetricsManager`, `ChannelStateStore` for health, lifecycle, metrics, and state persistence.
- **Command gating** — `CommandGate` with `Authorizer` and `AccessGroup` for command access control.
- **Thread binding** — `ThreadBindingManager` with policies for conversation placement decisions.
- **Identity validation** — `IdentityValidator` and `SenderIdentity` for sender verification.
- **Reply prefix** — `ReplyPrefixEngine` with templates for prepending model info to replies.
  *Responsibility boundary:* The template engine lives in `channels::reply_prefix` but is **applied** by the outbound pipeline (`DefaultOutboundPipeline`), not by channels themselves. `ReplyDispatcher` and `MessageFormatter` do not add prefixes — they receive already-prefixed content. See `docs/modules/outbound.md`.
- **Account snapshots** — `AccountSnapshotStore` for channel health snapshots.
- **ACP bridge** — `ChannelAcpBridge` for routing ACP commands through channels.
- **Session envelopes** — `SessionEnvelopeManager` for tracking session context.
- **Conversation resolver** — `ConversationResolver` with multiple resolution providers.

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

```rust
pub enum InputProvenance {
    ExternalUser { channel: String, is_direct: bool },
    InterSession { source_session: String },
    InternalSystem { source: String },
}
```

```rust
pub enum MentionState {
    DirectMessage,
    Mentioned,
    NotMentioned,
}
```

## Implemented Features

- Unified `Channel` trait with async lifecycle management
- Feature-gated channel implementations (Telegram, Discord, Slack, WhatsApp, QQ, Feishu/Lark, Signal, iMessage, WebChat)
- Native and WASM plugin channel support via `ExtendedChannelRegistry`
- Message validation and sanitization (length, control characters, null bytes)
- Input provenance tracking for trust decisions
- Mention gating for group channels
- Rich formatted content (Markdown, HTML, Slack mrkdwn, Discord embeds)
- Channel health monitoring and lifecycle management
- Metrics tracking with latency windows
- Command gating with authorizer modes
- Thread binding with placement policies
- Identity validation for sender verification
- Reply prefix templates with model metadata
- Account snapshot store for health reporting
- ACP bridge for inter-agent communication via channels
- Session envelope tracking
- Conversation resolution with multiple providers

