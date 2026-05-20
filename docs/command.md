# Manta Slash Commands

OpenClaw-style `/` commands available in Manta's web chat UI.

Commands are exposed via `commands.list` and executed via `commands.execute` over the WebSocket RPC protocol.

- **Local** commands run client-side without a backend round-trip.
- **Remote** commands are sent to the gateway via RPC.
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
| 📝 | **Stub** — reads/writes `runtime_settings`, but downstream does not yet consume the value |
| ⏳ | **Placeholder** — returns a message, no actual logic yet |

---

## Session Commands

| Command | Args | Description | Local | Admin | Status |
|---|---|---|---|---|---|
| `/new` | `[model]` | Start a new session | yes | no | ✅ |
| `/reset` | `[soft\|hard]` | Reset the current session | no | no | ✅ |
| `/stop` | — | Abort the current run | no | no | ✅ |
| `/compact` | `[instructions]` | Flush transcript to disk and compact context | no | no | ⏳ |
| `/export-session` | `[path]` | Export session transcript as HTML | no | no | ✅ |
| `/clear` | — | Clear chat history (client-side) | yes | no | ✅ |
| `/session` | `idle\|max-age <duration\|off>` | Manage session timeout settings | no | no | 📝 |

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
| `/think` | `<level>` | Set thinking level (`off`, `minimal`, `low`, `medium`, `high`) | no | no | 📝 |
| `/verbose` | `on\|off\|full` | Toggle verbose output | no | no | 📝 |
| `/trace` | `on\|off` | Toggle plugin trace | no | no | 📝 |
| `/fast` | `[on\|off\|status]` | Show or set fast mode | no | no | 📝 |
| `/reasoning` | `[on\|off\|stream]` | Set reasoning visibility | no | no | 📝 |
| `/queue` | `<mode>` | Set queue behavior (e.g. `steer`, `interrupt`, `followup`) | no | no | 📝 |

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
| `/help` | — | Show help summary | no | no | ✅ |
| `/commands` | — | Show full command catalog | no | no | ✅ |
| `/status` | — | Show gateway runtime status | no | no | ✅ |
| `/tools` | `[compact\|verbose]` | Show available tools | no | no | ✅ |
| `/whoami` | — | Show your sender ID and scopes | no | no | ✅ |
| `/usage` | `[off\|tokens\|full\|cost]` | Show usage statistics | no | no | 📝 |
| `/context` | `[list\|detail\|json]` | Show context assembly info | no | no | 📝 |

### Examples

```text
/help
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
| `/subagents` | `list\|kill\|log\|info\|send\|steer\|spawn` | Manage sub-agents | no | no | ✅ |
| `/acp` | `spawn\|cancel\|steer\|close\|sessions\|status\|...` | Manage ACP sessions | no | no | ✅ |
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
| `/skill` | `<name> [input]` | Run a skill by name | no | no | ✅ |
| `/allowlist` | `[list\|add\|remove] ...` | Manage command gate user levels | no | no | ✅ |
| `/approve` | `<id> <decision>` | Resolve an approval prompt | no | no | ✅ |
| `/btw` | `<question>` | Side question without changing context | no | no | ⏳ |

### Examples

```text
/skill summarize
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
| `/plugins` | `list\|install\|enable\|disable` | Inspect or toggle plugins | no | **yes** | ✅ |
| `/mcp` | `show\|get\|set\|unset` | Manage MCP server connections | no | **yes** | ✅ |
| `/debug` | `show\|set\|unset\|reset` | Runtime debug overrides | no | **yes** | ✅ |
| `/restart` | — | Restart the gateway | no | **yes** | ⏳ |
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

## Command Tiers

| Tier | Description |
|---|---|
| **essential** | Core commands always shown in the palette |
| **standard** | Common commands shown by default |
| **power** | Advanced/admin commands hidden unless filtering explicitly |

---

## RPC Methods

The command system is backed by two WebSocket RPC methods:

- **`commands.list`** — returns the full command catalog with metadata
- **`commands.execute`** — dispatches a command and returns the result

Both require `SCOPE_READ` (list) and `SCOPE_WRITE` (execute) respectively. Admin commands additionally require `SCOPE_ADMIN`.

---

## Unimplemented Commands — Difficulty Ranking

The following commands are stubs (📝) or placeholders (⏳). Ordered from easiest to hardest to fully implement.

| # | Command | Difficulty | What's Needed |
|---|---|---|---|
| 1 | `/reasoning` | **Easy** | Filter `reasoning_content` from responses before sending to client based on `runtime_settings["reasoning.visibility"]`. Frontend already handles display. |
| 2 | `/usage` | **Easy** | Hook into existing token counting in `agent/mod.rs` (already calculates `completion_tokens`), aggregate per-session, and write to `runtime_settings["usage.tokens"]` / `"usage.calls"`. |
| 3 | `/restart` | **Easy–Medium** | Send a signal to the main process or exit with a special code that the launcher wrapper restarts. Could also use `tokio::process::Command` to re-spawn self. |
| 4 | `/think` | **Medium** | Thread `runtime_settings["think.level"]` through to agent configuration. The agent already supports `thinking` options; map the setting to the provider's thinking parameter (e.g. Anthropic's `thinking.type` and `thinking.budget_tokens`). |
| 5 | `/trace` | **Medium** | Add conditional trace logging in `PluginManager` or plugin runtime that checks `runtime_settings["trace.enabled"]` before emitting trace events. |
| 6 | `/verbose` | **Medium** | Modify agent output formatting or tool result rendering based on `runtime_settings["verbose.mode"]`. May need to pass a flag through `AgentHandle` or `RunOptions`. |
| 7 | `/session` | **Medium** | Hook `runtime_settings["session.idle"]` / `"session.max-age"]` into `SessionManager::cleanup_timed_out()`. Requires a background task that periodically checks and terminates stale sessions. |
| 8 | `/btw` | **Medium** | Create a one-shot agent query path that bypasses session state. Can reuse `ModelRouter` directly without going through `chat.send` / session transcript. |
| 9 | `/fast` | **Hard** | Integrate with `ModelRouter` to dynamically switch to a faster/cheaper model when `runtime_settings["fast.mode"]` is true. Requires cost-aware routing and fallback logic. |
| 10 | `/context` | **Hard** | Expose the agent's context builder internals (system prompt, history, tools, memory injections). Requires introspecting `Context` / `TurnManager` at runtime. |
| 11 | `/queue` | **Hard** | Modify ACP serial queue behavior (steer vs interrupt vs followup) based on `runtime_settings["queue.mode"]`. Requires architectural changes to `AcpControlPlane` queue dispatch. |
| 12 | `/compact` | **Hardest** | Implement real context compaction: summarize or truncate transcript, inject summary as system prompt, and manage token budget. The `compaction.rs` module exists but is not yet wired into the command handler.
