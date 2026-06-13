# Tools Module

Capabilities the AI assistant can use to interact with the world.

## Design

- **`Tool` trait** — `name()`, `description()`, `parameters_schema()`, `execute(args, context)`
- **`ToolRegistry`** — Registration, lookup, execution with caching, circuit breaker, and trust-level filtering
- **`ToolContext`** — Execution context: user_id, conversation_id, working_directory, allowed_paths, allowed_commands, sandbox flags, workspace_root, skill_trust, optional `user_context` and `tool_policy` for RBAC
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
- **Fine-grained RBAC** — Role-based tool access via `Role`, `UserContext`, and `ToolPolicy` with deny/allow lists, required role, max risk level, and category filtering. Evaluated in `ToolRegistry::is_excluded()`. See `src/tools/rbac.rs`.

### MCP Integration

`mcp.rs` supports Model Context Protocol servers:
- Auto-discover tools from MCP servers
- Dynamic tool registration via `register_dynamic()`
- Prefix-based cleanup when servers disconnect (`deregister_prefix()`)

## Missing / TODO

- **✅ Implemented**: Full sandbox on non-Unix — `#[cfg(not(unix))]` `apply_resource_limits()` is explicitly a no-op. Unix uses `setrlimit`. See `src/tools/sandbox.rs:265-269`.
- **✅ Implemented**: Tool execution audit log — `RuntimeAuditLog` with in-memory ring buffer and `PersistentAuditLog` with SQLite backend. `AuditEventType::ToolInvocation` and `ToolDeny` are recorded. See `src/security/runtime_audit.rs` and `src/security/persistent_audit.rs`.
- **✅ Implemented**: Command registry — `CommandDef` with key, name, description, args, category, tier, aliases, scope, `local`, `requires_admin`. See `src/gateway/commands.rs`.
- **✅ Implemented**: Help system — `handle_help()` builds dynamic help with category grouping, pagination (8 per page), tier filtering via `--tier essential|standard|power`. See `src/gateway/commands.rs:444-499`.
- **✅ Implemented**: Exec approvals — `ApprovalQueue` with human-in-the-loop, `ask`/`host`/`security` approval levels, `ShellSafetyTier` analysis, `SafeBinList` pre-approved binary paths, `shell_safety_policy()` policy hook. See `src/tools/approval.rs` and `src/tools/shell_safety.rs`.
- **✅ Implemented**: Tool SDK — `ToolSdk` with `ToolPack`, `ToolCapabilities`, `ToolMetadata`, `CapabilityFilter`, `ToolSdkError`, registry connection via `with_registry()`, `find_by_capability()` queries, `sync_from_tool_registry()` bi-directional sync. See `src/tools/sdk.rs`.
- **✅ Implemented**: Command detection — `CommandDetector` with three-layer detection: control commands (`/start`, `/help`, `/pair`, etc.), `/`-prefixed command messages, and inline token detection. See `src/tools/command_detector.rs`.
- **✅ Implemented**: Command authorization — `AuthContext` with AllowFrom resolution, `OwnerState` machine (`Unverified`/`Verified`/`Delegated`/`Revoked`), sender candidate matching, and provider inference. Enforced in `handle_commands_execute()`. See `src/channels/command_gate.rs`, `src/channels/owner.rs`, `src/gateway/command_provider.rs`, `src/gateway/commands.rs`.
- **✅ Implemented**: Command gating — access groups with `AccessGroup`, multi-authorizer OR/AND logic via `Authorizer` enum and `AuthorizerMode`, dual authorizer support via `check()`/`check_dual()`. See `src/channels/command_gate.rs`.
- **❌ Missing**: Tool gating — 30+ option gating (plugin_tool_allowlist, model_has_vision, sender_is_owner, model/provider gating, sandbox policy). Currently only binary privileged/unprivileged split via `SkillTrust` + circuit breaker.
- **❌ Missing**: Tool result streaming for long-running operations. `ToolExecutionResult` is a single buffered struct; SDK `streaming` field is placeholder metadata only.
- **✅ Implemented**: Fine-grained RBAC — `Role` enum (`Guest`/`User`/`PowerUser`/`Admin`/`Owner`), `UserContext`, `ToolPolicy` with deny/allow lists, required role, max risk level, and category filtering. Integrated into `ToolRegistry::is_excluded()` and layered on top of `SkillTrust` for backward compatibility. See `src/tools/rbac.rs`, `src/tools/mod.rs`.
