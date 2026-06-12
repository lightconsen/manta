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

## Missing / TODO

- **✅ Implemented**: Inbound debounce policy — `InboundDebouncer` with configurable `debounce_ms`, LRU eviction, bypass prefixes, per-key buffering. See `src/inbound/debounce.rs:1-302`.
- **✅ Implemented**: Advanced mention gating — `MentionState` with `DirectMessage`/`Mentioned`/`NotMentioned` exists (`src/channels/mod.rs:146-169`). `ImplicitMentionDetector` with `is_reply_to_bot()` and `is_quoting_bot()` for implicit mentions (reply to bot, quoted bot). See `src/security/mention_gate.rs`.
- **✅ Implemented**: Transport stall watchdog — `ChannelHealthMonitor` tracks heartbeats and `Stale`/`Degraded`/`Unhealthy` status (`src/channels/health.rs:1-291`, `src/channels/lifecycle.rs:1-567`). `monitor_channel_with_timeout()` adds transport-level timeout detection for socket hangs and stalls inside individual channel health checks. See `src/channels/health.rs:287-337`.
- **✅ Implemented**: Channel plugin architecture (WASM) — `PluginChannel` and `PluginChannelRegistry` exist in `src/channels/plugin_host.rs`, `ChannelRegistry` supports `plugins: Option<PluginChannelRegistry>`. `ExtendedChannelRegistry` provides name-based plugin lookup with manifest storage. Plugin channels are discovered and started in Gateway lifecycle via `init_plugin_channels()`. See `src/gateway/mod.rs:3126-3177`.
- **✅ Implemented**: Config adapter with per-account enable/disable — CLI `syscity channel enable/disable` exists (`src/cli/channel.rs`), channel configs have `enabled: bool`. REST API endpoints: `GET /api/v1/channels`, `POST /api/v1/channels/{name}/enable`, `POST /api/v1/channels/{name}/disable` in `src/gateway/handlers/admin.rs`.
- **✅ Implemented**: Sender identity validation — `SenderIdentity` type with E164 phone validation
  (`+\d{3,}` up to 15 digits), username rules (alphanumeric + `_-\.`, 2-32 chars), multi-field
  identity (user_id, username, phone, email, display_name), `IdentityValidator` with configurable
  rules per platform (`AllowedCharSet` for Telegram/Discord/etc), platform-specific builders
  (`telegram_identity()`, `discord_identity()`, `slack_identity()`).
  See `src/channels/identity.rs:1-491`.
- **✅ Implemented**: Conversation resolution with multi-source fallback — `ConversationResolver`
  tries providers in order: `CommandProvider` (explicit `@agent`), `FocusedBindingProvider`
  (existing binding), `InboundProvider` (channel default), `ArtifactBindingProvider` (placeholder),
  `PluginBindingProvider` (placeholder), `FallbackProvider` (global default).
  `ResolutionSource` enum with priority ordering. See `src/channels/resolver.rs:1-444`.
- **✅ Implemented**: Thread binding policy — `ThreadBindingPolicy` with idle timeout (default 24h),
  max age (default 7d), placement hint (`Current`/`Child`), spawn target (`Subagent`/`Acp`), max
  children limit. `TrackedThreadBinding` with activity tracking. `ThreadBindingManager` with
  register/get/record/reap lifecycle and hierarchy tracking. Presets: `strict_policy()`,
  `branching_policy()`, `acp_policy()`. See `src/channels/thread_binding.rs:1-387`.
- **✅ Implemented**: Reply prefix template system — `ReplyPrefixTemplate` with `{{placeholder}}`
  syntax (`model_name`, `model_provider`, `timestamp`, `date`, `time`, `session_id`, `channel`,
  `user_id`, `cost`, custom fields), `ReplyPrefixEngine` with channel filters and async rendering.
  Presets: `model_tag_template()`, `minimal_model_template()`, `timestamp_model_template()`,
  `cost_aware_template()`. See `src/channels/reply_prefix.rs:1-310`.
- **✅ Implemented**: Channel-ACP binding integration — `ChannelAcpBridge` bridges channel
  conversations to ACP sessions with bidirectional `channel_conversation_id ↔ acp_session_id`
  mapping. Forwards messages via `AcpCommand::GetStatus/Pause/Resume/Cancel`. `parse_acp_command()`
  parses `/spawn`, `/acp run`, `/acp pause/resume/cancel/status`. See `src/channels/acp_bridge.rs:1-470`.
- **✅ Implemented**: Command gating for channel messages — `CommandGate` with named `AccessGroup`
  user sets, `Authorizer` variants (GroupMember, PairedUser, Admin, Allowlisted, Public, DenyAll,
  Custom), configurable OR/AND logic via `AuthorizerMode`, per-channel `CommandGateConfig` with
  command filters, dual authorizer support via `check_dual()`. `parse_command()` utility for `/`
  and `!` prefix parsing. See `src/channels/command_gate.rs:1-500`.
- **✅ Implemented**: Account snapshot system — `AccountSnapshot` with diagnostic display tones
  (Default/Muted/Success/Warn/Error), metrics tracking, `AccountSnapshotStore` with per-channel/
  per-account storage, worst-tone aggregation, diagnostic summary. Helper builders:
  `healthy_snapshot()`, `error_snapshot()`, `warning_snapshot()`, `muted_snapshot()`.
  See `src/channels/snapshot.rs:1-350`.
- **✅ Implemented**: Channel allowlists — every channel implementation supports `DmPolicy::Allowlist` with per-channel allowlist config: Discord, Telegram, Slack, Signal, QQ, Lark, iMessage, WhatsApp, WebChat. Multi-source matching via `MatchSource` enum in `src/security/allowlist.rs`.
- **✅ Implemented**: Session envelope context — `SessionEnvelopeContext` with `store_path` +
  `previous_timestamp` for interval calculation, session age, idle/expiry checks, messages-per-hour
  rate. `SessionEnvelopeManager` with per-conversation tracking, idle/expired eviction, active count.
  See `src/channels/envelope.rs:1-330`.
