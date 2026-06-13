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

