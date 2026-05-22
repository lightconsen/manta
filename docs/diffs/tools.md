# Manta Tools vs OpenClaw Tools — Detailed Comparison

> Last updated: 2026-05-15

## Overview

Both Manta and OpenClaw provide a tool system that allows LLM agents to interact with the outside world. OpenClaw's tool system is built in TypeScript around `ToolCatalog` with jiti-based plugin loading, while Manta's is implemented in Rust under `src/tools/` with a trait-based registry, WASM-based plugins, and extensive safety mechanisms.

**Current alignment: ~92%**

---

## Core Tool Architecture

| Feature | OpenClaw | Manta (`src/tools/mod.rs`) | Status |
|---------|----------|---------------------------|--------|
| **Tool Trait** | TypeScript interface | `Tool` trait with `async_trait` | Aligned |
| **Tool Name** | `name: string` | `fn name(&self) -> &str` | Aligned |
| **Description** | `description: string` | `fn description(&self) -> &str` | Aligned |
| **Parameters Schema** | JSON Schema object | `fn parameters_schema(&self) -> Value` | Aligned |
| **Execute** | `execute(args, context)` | `async fn execute(args, context)` | Aligned |
| **Availability Check** | Optional | `fn is_available(&self, context) -> bool` | Manta extra |
| **Timeout** | Context-based | `fn timeout(&self, context) -> Duration` | Aligned |
| **Function Definition** | Converted at registration | `fn to_function_definition(&self)` | Aligned |

### Key Differences

- **Trait vs Interface**: Manta uses Rust's `async_trait` for dynamic dispatch (`Box<dyn Tool>`), while OpenClaw uses TypeScript interfaces.
- **Availability Check**: Manta's `is_available()` allows tools to self-disable based on context (e.g., `CronTool` disables when scheduler not initialized).

---

## Tool Registry

| Feature | OpenClaw (`ToolCatalog`) | Manta (`ToolRegistry`) | Status |
|---------|--------------------------|------------------------|--------|
| **Static Registration** | `register(tool)` | `register(BoxedTool)` | Aligned |
| **Dynamic Registration** | Plugin SDK (jiti) | `register_dynamic(Arc<dyn Tool>)` | Aligned concept |
| **Deregistration** | By name | `remove()` + `deregister_prefix()` | Manta extra |
| **Blocked Prefixes** | Not explicit | `blocked_prefixes: RwLock<HashSet>` | Manta extra |
| **Tool Lookup** | `get(name)` | `get(name)` + `has(name)` + `list()` | Aligned |
| **Definitions** | `getDefinitions()` | `get_definitions()` + `get_available()` | Manta enhanced |
| **Circuit Breaker** | ❌ Not implemented | `failure_counts` with threshold=3 | Manta extra |
| **Tool Caching** | ❌ Not implemented | `cache` with TTL support | Manta extra |
| **Privilege Gating** | Basic policy | `privileged_tools` + `SkillTrust` | Manta enhanced |
| **Blocked Prefix Cleanup** | N/A | `deregister_prefix()` for MCP cleanup | Manta extra |

### Key Differences

- **Circuit Breaker**: Manta tracks consecutive failures per tool; after 3 failures the tool is marked "degraded" and excluded from `get_available()`. This prevents repeatedly calling broken tools.
- **Blocked Prefixes**: Manta's `deregister_prefix("mcp__server1__")` allows bulk cleanup of dynamically registered MCP tools when a server disconnects, without needing `&mut self`.
- **Privilege Gating**: Manta's `SkillTrust` enum (`Community` = 0, `Trusted` = 1) gates access to privileged tools (shell, file_write, delegate) at the registry level.

---

## Tool Execution Flow

| Feature | OpenClaw | Manta (`ToolRegistry::execute`) | Status |
|---------|----------|--------------------------------|--------|
| **Policy Enforcement** | `tool-policy.ts` granular rules | `ToolHooks::run_policy()` | Aligned |
| **Allow/Deny** | ✅ | `ToolPolicyDecision::Allow` / `Deny` | Aligned |
| **Human Approval** | ❌ Not explicit | `ToolPolicyDecision::NeedsApproval` | Manta extra |
| **Before Hooks** | Event hooks | `hooks.run_before()` | Aligned |
| **After Hooks** | Event hooks | `hooks.run_after()` | Aligned |
| **Result Caching** | ❌ | `get_cached()` / `store_cached()` | Manta extra |
| **Static Tool Execution** | ✅ | `tool.execute(args, context)` | Aligned |
| **Dynamic Tool Execution** | Plugin runtime | `dynamic_tools` fallback | Aligned |
| **Execution Result** | `{ output, error }` | `ToolExecutionResult` with data + timing | Manta enhanced |

### Execution Pipeline (Manta)

```
1. Policy hooks → Allow / Deny / NeedsApproval
   └─ NeedsApproval → ApprovalQueue → human decides → resume
2. Before hooks (fire-and-forget)
3. Cache check → return cached result if hit
4. Execute tool (static → dynamic fallback)
   └─ On success: store in cache, reset circuit breaker
   └─ On failure: record failure, increment circuit breaker
5. After hooks (fire-and-forget)
6. Return result
```

### Key Differences

- **Human-in-the-loop**: Manta's `ApprovalQueue` allows high-risk tool calls to suspend execution pending human approval via REST API (`/api/v1/approvals/:id/approve`). OpenClaw has no equivalent.
- **Result Caching**: Manta caches successful tool results with configurable TTL, keyed by `(tool_name, args_hash)`.
- **Execution Timing**: Manta's `ToolExecutionResult` includes `execution_time: Duration` for observability.

---

## Tool Context

| Feature | OpenClaw | Manta (`ToolContext`) | Status |
|---------|----------|----------------------|--------|
| **User ID** | ✅ | `user_id: String` | Aligned |
| **Conversation ID** | ✅ | `conversation_id: String` | Aligned |
| **Working Directory** | ✅ | `working_directory: PathBuf` | Aligned |
| **Environment** | ✅ | `environment: HashMap<String, String>` | Aligned |
| **Timeout** | ✅ | `timeout: Duration` (default 30s) | Aligned |
| **Allowed Paths** | ❌ | `allowed_paths: Vec<PathBuf>` | Manta extra |
| **Allowed Commands** | ❌ | `allowed_commands: Vec<String>` | Manta extra |
| **Sandbox Flag** | ✅ | `sandboxed: bool` | Aligned |
| **Resource Limits** | Basic | `memory_limit`, `cpu_limit`, `fd_limit`, `process_limit` | Manta enhanced |
| **Skill Trust** | ❌ | `skill_trust: SkillTrust` | Manta extra |

### Key Differences

- **Resource Limits**: Manta's `ToolContext` carries OS-level resource limits (memory, CPU, FDs, process count) that can be applied via `apply_resource_limits()` using `rlimit`.
- **Skill Trust**: The minimum trust level across active skills constrains available tools. A community skill cannot escalate to privileged tools.

---

## Security & Sandboxing

| Feature | OpenClaw | Manta | Status |
|---------|----------|-------|--------|
| **Sandbox Modes** | ✅ Sandbox modes | `SandboxedTool` wrapper | Aligned |
| **Path Restrictions** | ✅ | `SandboxConfig::allowed_paths` / `blocked_paths` | Aligned |
| **Network Control** | ✅ | `SandboxConfig::allow_network_access` | Aligned |
| **Timeout Enforcement** | ✅ | `SandboxConfig::timeout` + `tokio::time::timeout` | Aligned |
| **Path Validation** | ✅ | `check_path_args()` scans for path-like fields | Manta enhanced |
| **Approval Queue** | ❌ | `ApprovalQueue` with REST API | Manta extra |
| **Risk Levels** | ❌ | `RiskLevel` (Low/Medium/High/Critical) | Manta extra |
| **Circuit Breaker** | ❌ | 3-strike degradation | Manta extra |
| **Tool Validation** | Basic | `ToolRegistrar` with 3 validators | Manta enhanced |
| **Command Gating** | ✅ `command-gating.ts` | `CommandGate` with `UserLevel` | Aligned |

### `SandboxedTool`

Manta wraps any `Tool` with `SandboxedTool::new(tool, config)`:
- Validates path arguments before execution (checks `path`, `file`, `directory`, `dir`, `source`, `destination`, `dst` fields)
- Enforces allowlist/blocklist on file paths
- Enforces hard timeout via `tokio::time::timeout`
- Returns `MantaError::SandboxViolation` on violations

### `ToolRegistrar` Validation

Manta validates tools at registration time:
- **NameValidator**: 2-64 chars, alphanumeric + `_` + `-`, no leading digit
- **SchemaValidator**: Must have `type: "object"` and `properties`
- **SecurityValidator**: Checks for dangerous patterns in schema

---

## Hooks System

| Feature | OpenClaw | Manta (`ToolHooks`) | Status |
|---------|----------|---------------------|--------|
| **Before Hooks** | EventEmitter | `BeforeHookFn` (async, per tool) | Aligned |
| **After Hooks** | EventEmitter | `AfterHookFn` (async, per tool) | Aligned |
| **Policy Hooks** | `tool-policy.ts` | `PolicyHookFn` returning `ToolPolicyDecision` | Aligned |
| **Allow/Deny** | ✅ | `ToolPolicyDecision::Allow` / `Deny` | Aligned |
| **NeedsApproval** | ❌ | `ToolPolicyDecision::NeedsApproval` | Manta extra |
| **Multiple Hooks** | Single listener | Chained execution (all must allow) | Manta enhanced |

### Key Differences

- **Policy Chain**: Manta runs all registered policy hooks; if any returns `Deny`, execution stops. If any returns `NeedsApproval`, execution suspends.
- **Approval Metadata**: Manta's `NeedsApproval` includes `approval_id`, `risk_level`, `requested_by`, and human-readable `message`.

---

## Built-in Tools Comparison

| Tool | OpenClaw | Manta | File | Lines |
|------|----------|-------|------|-------|
| **File Read** | ✅ | ✅ `file_read` | `src/tools/file.rs` | ~534 |
| **File Write** | ✅ | ✅ `file_write` | `src/tools/file.rs` | ~534 |
| **File Edit** | ✅ | ✅ `file_edit` | `src/tools/file.rs` | ~534 |
| **Glob** | ✅ | ✅ `glob` | `src/tools/file.rs` | ~534 |
| **Grep** | ✅ | ✅ `grep` | `src/tools/grep.rs` | ~411 |
| **Shell** | ✅ | ✅ `shell` | `src/tools/shell.rs` | ~311 |
| **Web Search** | ✅ | ✅ `web_search` | `src/tools/web.rs` | ~1003 |
| **Web Fetch** | ✅ | ✅ `web_fetch` | `src/tools/web.rs` | ~1003 |
| **Browser** | ✅ | ✅ `browser` (feature-gated) | `src/tools/browser.rs` | ~613 |
| **Code Execution** | ✅ | ✅ `execute_code` | `src/tools/code_exec.rs` | ~440 |
| **Time/Date** | ✅ | ✅ `time` | `src/tools/time.rs` | ~446 |
| **Cron/Schedule** | ✅ | ✅ `cron` | `src/tools/cron_tool.rs` | ~344 |
| **Memory Store** | ✅ | ✅ `memory` | `src/tools/memory.rs` | ~847 |
| **Memory Search** | ✅ | ✅ `memory_search` | `src/tools/memory.rs` | ~847 |
| **Memory CRUD** | ✅ | ✅ `memory_get` | `src/tools/memory.rs` | ~847 |
| **Todo/Tasks** | ❌ | ✅ `todo` | `src/tools/todo_tool.rs` | ~546 |
| **Subagent Spawn** | ✅ | ✅ `acp_spawn` | `src/tools/acp_tool.rs` | ~436 |
| **Subagent Session** | ✅ | ✅ `acp_session` | `src/tools/acp_tool.rs` | ~436 |
| **Delegate** | ✅ | ✅ `delegate` | `src/tools/delegate_tool.rs` | ~696 |
| **Team Communicate** | ❌ | ✅ `team_communicate` | `src/tools/team_communicate_tool.rs` | ~324 |
| **MCP Connect** | ✅ | ✅ `mcp_connection` | `src/tools/mcp.rs` | ~1252 |
| **Canvas Tools** | ✅ `pi-tools` | ✅ via `CanvasManager` | `src/canvas/` | — |

### Key Differences

- **Todo Tool**: Manta has a full todo/task management tool (`todo_tool.rs`) that OpenClaw lacks. Tasks are persisted to `~/.manta/todos/{conversation_id}.json`.
- **Team Communicate**: Manta has a `team_communicate` tool for inter-agent messaging within teams, respecting mesh/star/chain/broadcast patterns. OpenClaw has no equivalent.
- **Browser**: Both have browser automation, but Manta's is feature-gated (`--features browser`) and uses `chromiumoxide`.
- **Code Execution**: Manta's `CodeExecutionTool` validates Python code against forbidden imports (`os.system`, `subprocess`, `socket`, `ctypes`) before execution.

---

## Plugin / Dynamic Tools

| Feature | OpenClaw | Manta | Status |
|---------|----------|-------|--------|
| **Plugin Runtime** | jiti (ESM hot reload) | WASM via `wasmtime` (feature-gated) | Different approach |
| **Tool Registration** | `plugins.registerTool()` | `PluginToolWrapper` adapts to `Tool` trait | Aligned concept |
| **Hot Reload** | ✅ jiti | ✅ `PluginManager::initialize()` | Aligned |
| **Sandboxing** | Node.js vm | WASM sandbox | Manta safer |
| **Language** | TypeScript/JavaScript | Any WASM-compilable | Manta more flexible |
| **Tool SDK** | Full SDK with hooks | `ToolSdk` + `ToolPack` + `ToolMetadata` | Aligned |
| **Dynamic Discovery** | ✅ | ✅ MCP auto-discovery | Aligned |

### Key Differences

- **Runtime**: OpenClaw uses jiti for TypeScript plugin loading (faster dev cycle, less sandboxed). Manta uses WASM via `wasmtime` (stronger sandbox, any language that compiles to WASM).
- **MCP Integration**: Both support MCP (Model Context Protocol). Manta's `McpConnectionTool` supports `stdio`, `sse`, and `streamable_http` transports, auto-discovers tools, and registers them dynamically.

---

## Tool SDK & Packs

| Feature | OpenClaw | Manta (`src/tools/sdk.rs`) | Status |
|---------|----------|---------------------------|--------|
| **Tool Packs** | ✅ Domain-grouped | `ToolPack` { name, version, tools } | Aligned |
| **Capabilities** | ✅ | `ToolCapabilities` { requires_approval, sandboxed, streaming, risk_level, categories } | Aligned |
| **Metadata** | ✅ | `ToolMetadata` { name, description, capabilities, schema } | Aligned |
| **Sync from Registry** | ✅ | `ToolSdk::sync_from_tool_registry()` | Aligned |

---

## Manta-Exclusive Tool Features (Not in OpenClaw)

| Feature | Module | Description |
|---------|--------|-------------|
| **Circuit Breaker** | `mod.rs` | 3-strike degradation per tool |
| **Tool Result Caching** | `mod.rs` | TTL-based cache keyed by `(name, args_hash)` |
| **Human Approval Queue** | `approval.rs` | Suspend execution pending REST API approval |
| **Risk Level Classification** | `approval.rs` | Low/Medium/High/Critical for approval decisions |
| **Skill Trust Gating** | `mod.rs` | `SkillTrust::Community` hides privileged tools |
| **Blocked Prefix Deregistration** | `mod.rs` | Bulk cleanup via `deregister_prefix()` |
| **Resource Limits** | `mod.rs` | OS-level memory/CPU/FD/process limits in `ToolContext` |
| **Command Gating** | `command_gate.rs` | `UserLevel` (Chat/User/Admin) for slash commands |
| **Todo Tool** | `todo_tool.rs` | Task management with priorities and subtasks |
| **Team Communicate** | `team_communicate_tool.rs` | Inter-agent messaging with pattern awareness |
| **Code Validation** | `code_exec.rs` | Forbidden import detection before Python execution |
| **SandboxedTool Wrapper** | `sandbox.rs` | Generic path/network/timeout sandbox for any tool |
| **Tool Validation** | `mod.rs` | Name/Schema/Security validators at registration |

---

## OpenClaw-Exclusive Tool Features (Not in Manta)

| Feature | Module | Gap |
|---------|--------|-----|
| **jiti Plugin Runtime** | `plugins/` | Manta uses WASM instead of jiti — different dev experience |
| **Granular Tool Policy** | `tool-policy.ts` | OpenClaw has richer rule-based policy engine |
| **Tool Security Audit** | `audit-tool-policy.ts` | Dedicated audit system for tool calls |
| **Channel Tools** | `channel-tools/` | Tools for sending messages through channels |
| **Canvas pi-tools** | `pi-tools/` | Richer canvas manipulation tools |

---

## File Mapping

| OpenClaw File | Manta File | Lines |
|---------------|------------|-------|
| `tools/` (~3,000 lines) | `src/tools/mod.rs` | ~1,590 |
| `tools/bash-tools/` | `src/tools/shell.rs` | ~311 |
| `tools/browser-tools/` | `src/tools/browser.rs` | ~613 |
| `tools/file-tools/` | `src/tools/file.rs` | ~534 |
| `tools/web-tools/` | `src/tools/web.rs` | ~1,003 |
| `tools/memory-tools/` | `src/tools/memory.rs` | ~847 |
| `tools/openclaw-tools/` | `src/tools/acp_tool.rs` + `delegate_tool.rs` | ~1,132 |
| `tool-policy.ts` | `src/tools/hooks.rs` + `approval.rs` | ~938 |
| `plugins/` (jiti runtime) | `src/plugins/` (WASM runtime) | ~400+ |
| `canvas/pi-tools/` | `src/canvas/` + `CanvasManager` | — |
| N/A | `src/tools/todo_tool.rs` | ~546 |
| N/A | `src/tools/team_communicate_tool.rs` | ~324 |
| N/A | `src/tools/sandbox.rs` | ~378 |
| N/A | `src/tools/command_gate.rs` | ~325 |
| N/A | `src/tools/sdk.rs` | ~127 |
| N/A | `src/tools/mcp.rs` | ~1,252 |

**Total**: OpenClaw ~5,000+ lines (TypeScript) vs Manta ~11,500+ lines (Rust) across tool-related files.

---

## Summary

Manta's tool system is **functionally equivalent** to OpenClaw's with several enhancements:

1. **Safety**: Circuit breaker, result caching, human approval queue, risk levels, skill trust gating
2. **Sandboxing**: Generic `SandboxedTool` wrapper with path/network/timeout enforcement
3. **Validation**: Name/schema/security validators at registration time
4. **Dynamic Registration**: Both static and dynamic tools with prefix-based bulk deregistration
5. **Unique Tools**: Todo management, team communication, code validation
6. **MCP Support**: Full Model Context Protocol client with auto-discovery

The remaining ~8% gap is primarily in:
- **jiti vs WASM plugin runtime** (different approaches, trade-offs in dev experience vs sandboxing)
- **Granular tool policy engine** (OpenClaw has richer rule-based policies)
- **Dedicated tool security audit** (OpenClaw's `audit-tool-policy.ts`)
- **Channel-specific tools** (OpenClaw has tools for sending messages through channels)
