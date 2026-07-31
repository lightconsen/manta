# Adding MCP Servers

How to connect an MCP (Model Context Protocol) server to Syscity and start using its tools.

## Prerequisites

- The daemon is running (`syscity start`). The web UI is at `http://127.0.0.1:18080` by default.
- For **local stdio servers** (the `npx`-style ones), Node.js must be installed on the same machine.
- The server's tools become available to agents immediately; you don't need to restart anything.

## Supported Transports

| Transport        | Use case                                        |
|------------------|-------------------------------------------------|
| `stdio`          | Spawn a local subprocess (default). `npx -y @modelcontextprotocol/server-filesystem` and similar. |
| `sse`            | Connect to a remote HTTP server via Server-Sent Events (older MCP spec). |
| `streamable_http`| POST requests with SSE response bodies (current MCP spec, used by remote/OAuth servers). |

## Four ways to add a server

Choose whichever fits. All of them persist to the same place (`[mcp.servers.*]` in `~/.syscity/config.toml`) and register the server's tools.

### 1. Enable a preset (Settings UI — easiest)

Open **Settings → MCP Servers**. The panel lists curated presets (GitHub, Gmail, Slack, Linear, Notion, …) plus remote OAuth presets. Click **Enable** next to one:

- **Stdio presets** (e.g. GitHub) connect immediately.
- **Remote OAuth presets** (e.g. "GitHub (Remote)") open a browser window — authorize, and the server connects automatically. If the browser doesn't open, a modal appears with the authorization link; click it, authorize, and it completes on its own.

Presets are defined in `src/mcp/presets.toml` and are freely editable — add your own or remove ones you don't need. Changes apply on the next page refresh.

### 2. Add a custom server (Settings UI — "Add MCP Server" form)

In **Settings → MCP Servers**, scroll to **Add Server** and fill in:

| Field              | Notes                                                        |
|--------------------|--------------------------------------------------------------|
| Server ID          | Unique key used in config and tool names (e.g. `filesystem`). |
| Transport          | `stdio`, `sse`, or `streamable_http`.                        |
| Command / Args     | Only for `stdio`. e.g. `npx` + `-y @modelcontextprotocol/server-filesystem`. |
| URL                | Only for `sse` / `streamable_http`. The server endpoint.     |
| Auth type          | `none` or `oauth2`. Choose `oauth2` for remote MCP servers that require authorization. |
| Client ID          | OAuth client ID supplied by the remote server (required for `oauth2`). |
| Scopes             | Space-separated permission scopes, if any.                   |
| Auth URL / Token URL | Usually optional — endpoints are auto-discovered from `/.well-known/oauth-authorization-server`. Only fill in if the server doesn't advertise them. |
| Auto-connect       | Connect automatically at startup and after adding.           |

Click **Add Server**. For OAuth servers this triggers the authorization flow in your browser (same as presets); otherwise it connects right away.

### 3. Edit `~/.syscity/config.toml` directly

Add a `[mcp.servers.<id>]` block, then restart the daemon (or let hot-reload pick it up):

```toml
# Local stdio server
[mcp.servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed"]
auto_connect = true

# Remote SSE server
[mcp.servers.mysse]
transport = "sse"
url = "https://example.com/sse"

# Remote OAuth server (streamable_http)
[mcp.servers.myremote]
transport = "streamable_http"
url = "https://mcp.example.com/"
auth_type = "oauth2"
client_id = "my-client-id"
scopes = ""
```

All fields are optional except those required by the transport: `command`/`args` for `stdio`, `url` for `sse`/`streamable_http`. Environment variables support `$VAR` references that are resolved at connect time:

```toml
[mcp.servers.myserver]
transport = "stdio"
command = "python"
args = ["server.py"]
env = { API_KEY = "$MY_API_KEY" }
```

Useful settings:

- `timeout_secs` — per-request timeout (default `30`).
- `max_tools` — cap on registered tools, `0` = unlimited (default).
- `health_check_interval_secs` — health-check cadence (default `30`; `0` disables).
- `auto_reconnect` — reconnect after a failed health check (default `true`).
- `max_reconnect_attempts` — cap on reconnect attempts (default `5`).

### 4. CLI

`syscity mcp connect <server_id>` connects a server and saves it to config. Requires the daemon to be running.

```bash
# Local stdio (--args is repeatable, one arg per flag)
syscity mcp connect filesystem \
  --command npx \
  --args "-y" \
  --args "@modelcontextprotocol/server-filesystem" \
  --args "/path/to/allowed"

# Remote OAuth
syscity mcp connect myremote \
  --url https://mcp.example.com/ \
  --transport streamable_http \
  --auth_type oauth2 \
  --client_id my-client-id
```

Other useful subcommands:

```bash
syscity mcp list                 # connected servers
syscity mcp tools <server_id>    # tools a server exposes
syscity mcp call <server_id> <tool> --args '{"key": "value"}'
syscity mcp resources <server_id>
syscity mcp disconnect <server_id>
```

## How OAuth authorization works

For remote servers with `auth_type = "oauth2"`, Syscity implements **OAuth 2.1 + PKCE**:

1. When you add/connect the server, Syscity discovers the OAuth endpoints via `/.well-known/oauth-authorization-server` and opens a local callback server.
2. A browser window opens on the server's authorization page (or the UI shows a link to click).
3. After you authorize, the browser redirects to the local callback, Syscity exchanges the code for an access + refresh token, and the server connects.

Tokens are stored at `~/.syscity/mcp_tokens/<server_id>.json` and cached in memory. Expiring tokens are refreshed automatically. **Removing a server deletes its stored token**, so a re-added server always starts a fresh flow.

## Verifying it worked

- The server appears under **Settings → MCP Servers** as connected.
- Its tools are available to agents, named `mcp__<server_id>__<tool>` (e.g. `mcp__github__create_issue`).
- `syscity mcp tools <server_id>` lists them; `syscity mcp call <server_id> <tool> --args '...'` calls one directly.

## Removing a server

- **Settings UI:** click **Remove** on the server's row.
- **CLI:** `syscity mcp disconnect <server_id>`.
- **config.toml:** delete the `[mcp.servers.<id>]` block and restart.

Removal deregisters the server's tools, drops its OAuth token, and deletes the persisted config.

## Testing OAuth locally

Run the included mock OAuth MCP server, then enable the "Test OAuth" preset:

```bash
python scripts/test_oauth_mcp.py
```

It serves a Streamable-HTTP MCP server with a full OAuth flow on `http://localhost:9999`, so you can exercise add → authorize → connect → tool call without any external service.
