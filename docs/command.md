# Syscity Slash Commands

`/` commands are available in Syscity's chat UI, TUI, and any channel that supports slash-command parsing (Telegram, Discord, Slack, WebChat, etc.).

The command system has two layers:

- **Gateway command catalog** (`src/gateway/commands.rs`) — canonical command definitions exposed via the WebSocket RPC method `commands.list` and executed via `commands.execute`.
- **TUI slash commands** (`src/tui/commands.rs`) — client-side parser that handles a small set of local commands and forwards everything else to the gateway.

- **Local** commands run client-side without a backend round-trip.
- **Remote** commands are sent to the gateway via WebSocket RPC.
- **Admin** commands require `SCOPE_ADMIN`.

---

## Legend

| Badge | Meaning |
|---|---|
| `local` | Executed client-side |
| `admin` | Requires admin scope |
| `essential` | Always shown in completions |
| `standard` | Shown by default |
| `power` | Hidden unless explicitly requested |

### Implementation Status

| Icon | Meaning |
|---|---|
| ✅ | **Implemented** — calls real backend APIs |
| ⏳ | **Partial** — basic implementation, subcommands may be missing |

---

## Session Commands

| Command | Args | Description | Local | Admin | Status |
|---|---|---|---|---|---|
| `/new` | `[model]` | Start a new session | yes | no | ✅ |
| `/reset` | `[soft\|hard]` | Reset the current session | no | no | ✅ |
| `/stop` | — | Abort the current run | no | no | ✅ |
| `/compact` | `[instructions]` | Flush transcript to disk and compact context | no | no | ✅ |
| `/export-session` | `[path]` | Export session transcript as HTML | no | no | ✅ |
| `/clear` | — | Clear chat history (client-side) | yes | no | ✅ |
| `/session` | `idle\|max-age <duration\|off>` | Manage session timeout settings | no | no | ✅ |

### Examples

```text
/new gpt-4
/reset hard
/stop
/compact summarize the last 50 messages
/export-session
/session idle 30m
/session max-age off
```

---

## Model / Directive Commands

| Command | Args | Description | Local | Admin | Status |
|---|---|---|---|---|---|
| `/model` | `[name\|#\|status]` | Show or switch the active model | no | no | ✅ |
| `/think` | `<level>` | Set thinking level (`off`, `minimal`, `low`, `medium`, `high`) | no | no | ✅ |
| `/verbose` | `on\|off\|full` | Toggle verbose output | no | no | ✅ |
| `/trace` | `on\|off` | Toggle plugin trace | no | no | ✅ |
| `/fast` | `[on\|off\|status]` | Show or set fast mode | no | no | ✅ |
| `/reasoning` | `[on\|off\|stream]` | Set reasoning visibility | no | no | ✅ |
| `/queue` | `<mode>` | Set queue behavior (`steer`, `interrupt`, `followup`) | no | no | ✅ |

### Examples

```text
/model status
/model claude-sonnet-4-6
/think high
/verbose full
/fast on
/reasoning stream
/queue steer
```

---

## Status / Query Commands

| Command | Args | Description | Local | Admin | Status |
|---|---|---|---|---|---|
| `/help` | `[page] [--tier=essential|standard|power]` | Show paginated help summary | no | no | ✅ |
| `/commands` | — | Alias for `/help` | no | no | ✅ |
| `/status` | — | Show gateway runtime status | no | no | ✅ |
| `/tools` | `[compact\|verbose]` | Show available tools | no | no | ✅ |
| `/whoami` | — | Show your sender ID and scopes | no | no | ✅ |
| `/usage` | `[off\|tokens\|full\|cost]` | Show usage statistics | no | no | ⏳ |
| `/context` | `[list\|detail\|json]` | Show context assembly info | no | no | ✅ |

### Examples

```text
/help
/help 2 --tier=power
/status
/tools verbose
/whoami
/usage tokens
/context detail
```

---

## Subagents / ACP Commands

| Command | Args | Description | Local | Admin | Status |
|---|---|---|---|---|---|
| `/subagents` | `list\|kill\|log\|info\|send\|steer\|spawn` | Manage sub-agents | no | no | ⏳ |
| `/acp` | `spawn\|cancel\|steer\|close\|sessions\|status\|...` | Manage ACP sessions | no | no | ⏳ |
| `/kill` | `<id\|#\|all>` | Abort sub-agent runs | no | no | ✅ |
| `/steer` | `<id> <message>` | Send steering message to a sub-agent | no | no | ✅ |
| `/tell` | `<id> <message>` | Alias for `/steer` | no | no | ✅ |
| `/focus` | `<target>` | Bind thread to a session target (agent) | no | no | ✅ |
| `/unfocus` | — | Remove thread binding | no | no | ✅ |

### Examples

```text
/subagents list
/acp status
/acp cancel
/kill all
/steer subagent-1 stop and summarize
/focus my-agent
/unfocus
```

---

## Skills / Approval Commands

| Command | Args | Description | Local | Admin | Status |
|---|---|---|---|---|---|
| `/skill` | `<name> [input]` | List skills or show skill details | no | no | ✅ |
| `/allowlist` | `[list\|add\|remove] ...` | Manage command gate user levels | no | no | ✅ |
| `/approve` | `<id> <decision>` | Resolve an approval prompt | no | no | ✅ |
| `/btw` | `<question>` | Side question without changing context | no | no | ✅ |

### Examples

```text
/skill summarize
/skill summarize my notes
/allowlist list
/allowlist add alice admin
/approve abc123 approve
/btw what is the weather?
```

---

## Admin Commands (Owner-Only)

| Command | Args | Description | Local | Admin | Status |
|---|---|---|---|---|---|
| `/config` | `show\|get\|set\|unset` | Read or write runtime config | no | **yes** | ✅ |
| `/plugins` | `list\|enable\|disable` | Inspect or toggle plugins | no | **yes** | ✅ |
| `/mcp` | `show\|disconnect` | Manage MCP server connections | no | **yes** | ⏳ |
| `/debug` | `show\|set\|unset\|reset` | Runtime debug overrides | no | **yes** | ✅ |
| `/restart` | — | Restart the gateway | no | **yes** | ✅ |
| `/bash` | `<command>` | Run a host shell command | no | **yes** | ✅ |

### Examples

```text
/config show
/config get model.override
/config set model.override claude-opus-4-6
/plugins list
/plugins enable my-plugin
/mcp show
/mcp disconnect my-server
/debug show
/debug reset
/restart
/bash ls -la
```

---

## TUI-Only Local Commands

These commands are handled directly by `src/tui/commands.rs` and never leave the terminal:

| Command | Args | Description |
|---|---|---|
| `/new` | — | Create a new session and subscribe to it |
| `/clear` | — | Clear the message panel |
| `/status` | — | Query gateway presence (`system.presence`) |
| `/tools` | — | Show number of available commands (`commands.list`) |
| `/model` | `<id>` | Set the default model (`models.set_default`) |
| `/help` | — | Open help popup with command list |
| `/config` | — | Open config editor popup |
| `/sessions` | — | List and refresh sessions |
| `/quit` / `/exit` | — | Exit the TUI |

---

## Inline Shortcuts

The following commands may be embedded inside normal messages. The command is executed and the remaining text continues as a regular chat message.

- `/help`
- `/commands`
- `/status`
- `/whoami`

### Example

```text
Hey, can you help me with this? /whoami By the way, what's 2+2?
```

The `/whoami` command executes and returns the sender ID, then the rest of the message is sent to the model.

---

## Command Metadata

Each command is represented by a `CommandDef`:

```rust
pub struct CommandDef {
    pub key: String,
    pub name: String,
    pub description: String,
    pub args: Option<String>,
    pub category: CommandCategory,
    pub tier: CommandTier,
    pub local: bool,
    pub requires_admin: bool,
    pub aliases: Vec<String>,
    pub scope: CommandScope,
    pub provider_hint: Option<CommandProviderHint>,
}
```

| Field | Meaning |
|---|---|
| `key` | Canonical command key used for dispatch |
| `name` | Display name |
| `args` | Argument hint shown in help |
| `category` | `Session`, `Model`, `Status`, `Agents`, `Tools`, `Admin` |
| `tier` | `Essential`, `Standard`, `Power` |
| `local` | Runs client-side only |
| `requires_admin` | Requires `SCOPE_ADMIN` |
| `aliases` | Alternative names |
| `scope` | `Global`, `DirectMessage`, `Channel` |
| `provider_hint` | Optional provider/model override |

---

## Command Tiers

| Tier | Description |
|---|---|
| **essential** | Core commands always shown in the palette |
| **standard** | Common commands shown by default |
| **power** | Advanced/admin commands hidden unless filtering explicitly |

Tier is enforced by `CommandGate::user_level` in addition to the admin scope check.

---

## RPC Methods

The gateway command system is backed by two WebSocket RPC methods:

- **`commands.list`** — returns the full command catalog with metadata
- **`commands.execute`** — dispatches a command and returns the result

Both require `SCOPE_READ` (list) and `SCOPE_WRITE` (execute) respectively. Admin commands additionally require `SCOPE_ADMIN`.

---

## Provider/Model Hints

`CommandProviderResolver` (in `src/gateway/command_provider.rs`) maps commands to advisory provider/model hints:

| Condition | Hint | Reason |
|---|---|---|
| Explicit `provider_hint` on `CommandDef` | configured hint | highest priority |
| `UserLevel::Chat` | `fast` | low-trust users steered to cheap model |
| `CommandCategory::Admin` or `CommandTier::Power` | `power` | admin/power commands need capable model |
| Essential `Session`/`Status` commands | `fast` | cheap to answer |
| Everything else | default | no hint |

---

## Implementation Notes

| Command | Key Implementation Detail |
|---|---|
| `/new` | In the gateway catalog it is marked `local` (channel-only); the TUI handles `/new` locally by calling `sessions.create` + `sessions.subscribe`. |
| `/clear` | Client-side only; removes messages from the local buffer. |
| `/status` | Gateway handler returns active agent/session counts from `GatewayState`. |
| `/tools` | Calls `ToolRegistry::list()` and optionally shows each tool name. |
| `/whoami` | Returns the authenticated `user_id` and granted scopes from the WebSocket connection. |
| `/think` | Stores `think.level` in `runtime_settings`. The actual budget injection happens downstream before completion. |
| `/trace` | Calls `PluginManager::set_trace_enabled()` and stores `trace.enabled` in `runtime_settings`. |
| `/fast` | Swaps `config.model` to the resolved `fast` alias on `/fast on`, and restores the original model on `/fast off`. |
| `/queue` | Stores `queue.mode` in `runtime_settings`; `interrupt` mode can be consumed by `send_to_agent()` to cancel the current run. |
| `/context` | Resolves the active agent via `AgentRouter::resolve_by_session()`, then calls `Agent::context_info()` to introspect message count, tokens, and tool iterations. |
| `/compact` | Resolves the active agent, calls `Agent::compact_context()`, then flushes the transcript to disk via `TranscriptStore::export(HTML)`. |
| `/btw` | Bypasses session state by calling `ModelRouter::complete_auto()` directly for a one-shot Q&A. |
| `/restart` | Spawns a background task that sleeps 1 second then calls `std::process::exit(0)`. |
| `/bash` | Runs `sh -c <args>` and returns stdout/stderr/exit code. |
| `/approve` | Lists pending approvals or resolves one via `ApprovalQueue::resolve()`. |
| `/allowlist` | Reads/writes user levels via `CommandGate`. |

---

## Module Relationships

```
src/gateway/commands.rs        # canonical catalog and execute handler
src/gateway/command_provider.rs # provider/model hint resolution
src/tui/commands.rs            # TUI client-side slash command parser
src/channels/command_gate.rs   # channel-side command gating
src/tools/command_gate.rs      # user level store and enforcement
```
