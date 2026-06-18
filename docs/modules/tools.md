# Tools Module

Capabilities the AI assistant can use to interact with the world.

## Design

- **`Tool` trait** — `name()`, `description()`, `parameters_schema()`, `execute(args, context)`
- **`ToolRegistry`** — Registration, lookup, execution with caching, circuit breaker, and trust-level filtering
- **`ToolContext`** — Execution context: user_id, conversation_id, working_directory, allowed_paths, allowed_commands, sandbox flags, workspace_root, skill_trust, optional `user_context` and `tool_policy` for RBAC
- **`ToolRegistrar`** — Validation-aware registration with name, schema, and security validators
- **`ApprovalQueue`** — Human-in-the-loop approval system with risk levels
- **`RBAC`** — Role-based access control with `Role`, `UserContext`, `ToolPolicy`, `SandboxPolicy`

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
| Gateway | `gateway` |
| ACP | `acp_session`, `acp_spawn` |
| MCP | `mcp_connection` (auto-discovers MCP server tools) |
| Canvas | `canvas` |
| SDK | `sdk` |
| Nodes | `nodes` |
| STT | `stt` |
| Command Detection | `command_detector` |

### Security Features

- **Path traversal detection** — `../`, `~`, null bytes blocked in SecurityValidator
- **Command injection detection** — `;`, `|`, `$`, `` ` ``, `$(` blocked
- **Sandbox mode** — Resource limits via `setrlimit` (Unix): memory, CPU, FDs, processes
- **Workspace boundary** — `workspace_only` mode restricts file ops to `workspace_root`
- **Approval queue** — Human-in-the-loop for high-risk tools with `RiskLevel` classification
- **Circuit breaker** — Tools disabled after 3 consecutive failures
- **Privilege filtering** — Privileged tools hidden when `skill_trust == Community`
- **Fine-grained RBAC** — Role-based tool access via `Role`, `UserContext`, and `ToolPolicy` with deny/allow lists, required role, max risk level, and category filtering. Evaluated in `ToolRegistry::is_excluded()`. See `src/tools/rbac.rs`.
- **Content filtering** — Secret scanning and PII detection in tool outputs via `ContentFilter`

### MCP Integration

`mcp.rs` supports Model Context Protocol servers:
- Auto-discover tools from MCP servers
- Dynamic tool registration via `register_dynamic()`
- Prefix-based cleanup when servers disconnect (`deregister_prefix()`)

## Key Types

```rust
pub struct ToolContext {
    pub user_id: String,
    pub conversation_id: String,
    pub working_directory: PathBuf,
    pub environment: HashMap<String, String>,
    pub timeout: Duration,
    pub allowed_paths: Vec<PathBuf>,
    pub allowed_commands: Vec<String>,
    pub sandboxed: bool,
    pub memory_limit: Option<usize>,
    pub cpu_limit: Option<u64>,
    pub fd_limit: Option<u64>,
    pub process_limit: Option<u64>,
    pub skill_trust: SkillTrust,
    pub workspace_root: PathBuf,
    pub workspace_only: bool,
    pub user_context: Option<UserContext>,
    pub tool_policy: Option<ToolPolicy>,
    pub model_name: Option<String>,
    pub provider_name: Option<String>,
    pub sender_id: Option<String>,
    pub sender_is_owner: bool,
    pub plugin_allowlist: Option<Vec<String>>,
    pub model_capabilities: ModelCapabilities,
    pub sandbox_policy: Option<SandboxPolicy>,
}
```

```rust
pub enum SkillTrust {
    Community = 0,
    Trusted = 1,
}
```

```rust
pub enum RiskLevel {
    None,
    Low,
    Medium,
    High,
    Critical,
}
```

## Implemented Features

- Unified `Tool` trait with async execution
- Tool registry with registration, lookup, and execution
- Execution context with sandboxing and resource limits
- Path traversal and command injection detection
- Workspace boundary enforcement
- Approval queue with risk-level-based filtering
- Circuit breaker for failing tools
- Skill trust-based privilege filtering
- Fine-grained RBAC with role, policy, and category filtering
- MCP client integration with auto-discovery
- Dynamic tool registration and deregistration
- Content filtering with secret scanning and PII detection
- Command detection layer for parsing structured commands from messages
- Streaming tool execution with chunk-based output
- Model and provider-based tool gating
- Sender-based tool access control

