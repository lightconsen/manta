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

- **✅ Implemented**: Full sandbox on non-Unix — `#[cfg(not(unix))]` `apply_resource_limits()` is explicitly a no-op. Unix uses `setrlimit`. See `src/tools/sandbox.rs:265-269`.
- **✅ Implemented**: Tool execution audit log — `RuntimeAuditLog` with in-memory ring buffer and `PersistentAuditLog` with SQLite backend. `AuditEventType::ToolInvocation` and `ToolDeny` are recorded. See `src/security/runtime_audit.rs` and `src/security/persistent_audit.rs`.
- **📝 Partial**: Command registry — `CommandDef` exists with key, name, description, args, category, tier (`src/gateway/commands.rs:1-1678`). Missing: aliases, scope. Has `local` and `requires_admin` instead.
- **📝 Partial**: Help system — `handle_help()` builds dynamic help with category grouping. Missing: pagination (8 per page), feature flags. See `src/gateway/commands.rs:399-433`.
- **📝 Partial**: Exec approvals — `ApprovalQueue` with human-in-the-loop exists, including risk levels and oneshot resolution. Missing: `ask`/`host`/`security` levels, shell command safety analysis, safe bin policy. See `src/tools/approval.rs`.
- **📝 Partial**: Tool SDK — `ToolSdk` with `ToolPack`, `ToolCapabilities`, `ToolMetadata` exists for dynamic pack registration. Not a full external SDK. See `src/tools/sdk.rs`.
- **❌ Missing**: Command detection — three-layer detection (control command / command message / inline token).
- **❌ Missing**: Command authorization — provider inference, AllowFrom resolution, owner state machine, sender candidate matching.
- **❌ Missing**: Command gating — access groups, multi-authorizer OR logic, dual authorizer support, `resolveControlCommandGate()`.
- **❌ Missing**: Tool gating — 30+ option gating (plugin_tool_allowlist, model_has_vision, sender_is_owner, model/provider gating, sandbox policy).
- **❌ Missing**: Tool result streaming for long-running operations.
- **❌ Missing**: Fine-grained RBAC beyond Community/Trusted trust levels.
