# Tools Module

Capabilities the AI assistant can use to interact with the world.

## Design

- **`Tool` trait** — `name()`, `description()`, `parameters_schema()`, `execute(args, context)`
- **`ToolRegistry`** — Registration, lookup, execution with caching, circuit breaker, and trust-level filtering
- **`ToolContext`** — Execution context: user_id, conversation_id, working_directory, allowed_paths, allowed_commands, sandbox flags, workspace_root, skill_trust
- **`ToolRegistrar`** — Validation-aware registration with name, schema, and security validators

### Built-in Tools

| Category | Tools |
|----------|-------|
| File | `read_file`, `write_file`, `edit_file`, `glob` |
| Shell | `shell` (with allowlist) |
| Web | `web_fetch`, `web_search` |
| Code | `code_exec` (sandboxed) |
| Memory | `memory_search`, `memory_get` |
| Session | `sessions_list`, `sessions_history`, `sessions_send`, `sessions_status` |
| Agent | `delegate`, `agents_list` |
| Browser | `browser` |
| Image | `image_generate` |
| PDF | `pdf` |
| Process | `process` |
| Grep | `grep` |
| Patch | `apply_patch` |
| Time | `time` |
| Todo | `todo` |
| Cron | `cron` |
| TTS | `tts` |
| Team | `team_communicate` |
| Gateway | `gateway` |
| ACP | `acp_session`, `acp_spawn` |
| MCP | `mcp_connection` (auto-discovers MCP server tools) |
| Canvas | `canvas` |

### Security Features

- **Path traversal detection** — `../`, `~`, null bytes blocked in SecurityValidator
- **Command injection detection** — `;`, `|`, `$`, `` ` ``, `$(` blocked
- **Sandbox mode** — Resource limits via `setrlimit` (Unix): memory, CPU, FDs, processes
- **Workspace boundary** — `workspace_only` mode restricts file ops to `workspace_root`
- **Approval queue** — Human-in-the-loop for high-risk tools
- **Circuit breaker** — Tools disabled after 3 consecutive failures
- **Privilege filtering** — Privileged tools hidden when `skill_trust == Community`

### MCP Integration

`mcp.rs` supports Model Context Protocol servers:
- Auto-discover tools from MCP servers
- Dynamic tool registration via `register_dynamic()`
- Prefix-based cleanup when servers disconnect (`deregister_prefix()`)

## Missing / TODO

- **Missing**: Command registry — `ChatCommandDefinition` with key, aliases, description, args, category, tier (essential/standard/power), scope.
- **Missing**: Command detection — three-layer detection (control command / command message / inline token), deliberate false-positive bias.
- **Missing**: Command authorization — provider inference, AllowFrom resolution, owner state machine, sender candidate matching, gateway client scope.
- **Missing**: Command gating — access groups, multi-authorizer OR logic, dual authorizer support, `resolveControlCommandGate()`.
- **Missing**: Help system — dynamic help message building with feature flags, category grouping, pagination (8 per page).
- **Missing**: Tool gating — 30+ option gating (plugin_tool_allowlist, model_has_vision, sender_is_owner, model/provider gating, sandbox policy).
- **Missing**: Exec approvals — `ask = "never" | "dangerous" | "always"`, `host = "auto" | "node" | "gateway"`, `security = "sandbox" | "normal" | "relaxed"`, shell command safety analysis (rm, curl, etc.), safe bin policy, host env sanitization.
- **Missing**: Tool SDK for external WASM plugin tools.
- **Missing**: Full sandbox isolation on non-Unix platforms (Windows sandbox is a no-op).
- **Missing**: Tool result streaming for long-running operations.
- **Missing**: Tool execution audit log.
- **Missing**: Fine-grained RBAC beyond Community/Trusted trust levels.
