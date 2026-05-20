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

---

## Session Commands

| Command | Args | Description | Local | Admin |
|---|---|---|---|---|
| `/new` | `[model]` | Start a new session | yes | no |
| `/reset` | `[soft\|hard]` | Reset the current session | no | no |
| `/stop` | — | Abort the current run | no | no |
| `/compact` | `[instructions]` | Flush transcript to disk and compact context | no | no |
| `/export-session` | `[path]` | Export session transcript as HTML | no | no |
| `/clear` | — | Clear chat history (client-side) | yes | no |
| `/session` | `idle\|max-age <duration\|off>` | Manage session timeout settings | no | no |

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

| Command | Args | Description | Local | Admin |
|---|---|---|---|---|
| `/model` | `[name\|#\|status]` | Show or switch the active model | no | no |
| `/think` | `<level>` | Set thinking level (`off`, `minimal`, `low`, `medium`, `high`) | no | no |
| `/verbose` | `on\|off\|full` | Toggle verbose output | no | no |
| `/trace` | `on\|off` | Toggle plugin trace | no | no |
| `/fast` | `[on\|off\|status]` | Show or set fast mode | no | no |
| `/reasoning` | `[on\|off\|stream]` | Set reasoning visibility | no | no |
| `/queue` | `<mode>` | Set queue behavior (e.g. `steer`, `interrupt`, `followup`) | no | no |

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

| Command | Args | Description | Local | Admin |
|---|---|---|---|---|
| `/help` | — | Show help summary | no | no |
| `/commands` | — | Show full command catalog | no | no |
| `/status` | — | Show gateway runtime status | no | no |
| `/tools` | `[compact\|verbose]` | Show available tools | no | no |
| `/whoami` | — | Show your sender ID and scopes | no | no |
| `/usage` | `[off\|tokens\|full\|cost]` | Show usage statistics | no | no |
| `/context` | `[list\|detail\|json]` | Show context assembly info | no | no |

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

| Command | Args | Description | Local | Admin |
|---|---|---|---|---|
| `/subagents` | `list\|kill\|log\|info\|send\|steer\|spawn` | Manage sub-agents | no | no |
| `/acp` | `spawn\|cancel\|steer\|close\|sessions\|status\|...` | Manage ACP sessions | no | no |
| `/kill` | `<id\|#\|all>` | Abort sub-agent runs | no | no |
| `/steer` | `<id> <message>` | Send steering message to a sub-agent | no | no |
| `/tell` | `<id> <message>` | Alias for `/steer` | no | no |
| `/focus` | `<target>` | Bind thread to a session target (agent) | no | no |
| `/unfocus` | — | Remove thread binding | no | no |

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

| Command | Args | Description | Local | Admin |
|---|---|---|---|---|
| `/skill` | `<name> [input]` | Run a skill by name | no | no |
| `/allowlist` | `[list\|add\|remove] ...` | Manage command gate user levels | no | no |
| `/approve` | `<id> <decision>` | Resolve an approval prompt | no | no |
| `/btw` | `<question>` | Side question without changing context | no | no |

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

| Command | Args | Description | Local | Admin |
|---|---|---|---|---|
| `/config` | `show\|get\|set\|unset` | Read or write runtime config | no | **yes** |
| `/plugins` | `list\|install\|enable\|disable` | Inspect or toggle plugins | no | **yes** |
| `/mcp` | `show\|get\|set\|unset` | Manage MCP server connections | no | **yes** |
| `/debug` | `show\|set\|unset\|reset` | Runtime debug overrides | no | **yes** |
| `/restart` | — | Restart the gateway | no | **yes** |
| `/bash` | `<command>` | Run a host shell command | no | **yes** |

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
