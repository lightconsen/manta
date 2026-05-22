# Manta Built-in Tools Testing Coverage

> Generated: 2026-05-22

## Tool Registry

All tools are registered in `src/gateway/mod.rs` via `create_default_tool_registry()` (line 3920).

## Coverage Matrix

| # | Tool Name | Category | Function | `e2e_websocket.rs` | `integrations_live.rs` | Status |
|---|-----------|----------|----------|:------------------:|:----------------------:|:------:|
| 1 | `file_read` | File System | Read file content | Chat trigger | Direct call | **Tested** |
| 2 | `file_write` | File System | Write file content | Chat trigger | Direct call | **Tested** |
| 3 | `file_edit` | File System | Edit file content | Chat trigger | Direct call | **Tested** |
| 4 | `glob` | File System | File pattern matching | Chat trigger | Direct call | **Tested** |
| 5 | `grep` | File System | Text search | Chat trigger | Direct call | **Tested** |
| 6 | `shell` | Execution | Execute shell commands | Chat trigger | Direct call | **Tested** |
| 7 | `execute_code` | Execution | Execute code (Python, etc.) | Chat trigger | Direct call | **Tested** |
| 8 | `web_search` | Web | Network search | Chat trigger | Direct call | **Tested** |
| 9 | `web_fetch` | Web | Fetch web page content | Chat trigger | Direct call | **Tested** |
| 10 | `todo` | Management | Todo list management | Chat trigger | Direct call | **Tested** |
| 11 | `cron` | Management | Cron job management | Chat trigger | Direct call | **Tested** |
| 12 | `time` | Utility | Get current time | Chat trigger | Direct call | **Tested** |
| 13 | `browser` | Utility | Browser automation (feature-gated) | -- | -- | **Untested** |
| 14 | `acp_spawn` | ACP | Spawn subagent | Chat trigger | Direct call | **Tested** |
| 15 | `acp_session` | ACP | Manage ACP session | Chat trigger | Direct call | **Tested** |
| 16 | `sessions_list` | ACP | List sessions | Chat trigger | Direct call | **Tested** |
| 17 | `sessions_history` | ACP | Get session history | Chat trigger | Direct call | **Tested** |
| 18 | `sessions_send` | ACP | Send message to session | Chat trigger | Direct call | **Tested** |
| 19 | `sessions_yield` | ACP | Yield result to session | Chat trigger | Direct call | **Tested** |
| 20 | `session_status` | ACP | Get session status | Chat trigger | Direct call | **Tested** |
| 21 | `subagents` | ACP | Subagent management | Chat trigger | Direct call | **Tested** |
| 22 | `apply_patch` | ACP | Apply code patch | Chat trigger | Direct call | **Tested** |
| 23 | `memory` | Memory | Store memory entry | Chat trigger | Direct call | **Tested** |
| 24 | `memory_search` | Memory | Semantic search memory | Chat trigger | Direct call | **Tested** |
| 25 | `memory_get` | Memory | CRUD memory operations | Chat trigger | Direct call | **Tested** |
| 26 | `delegate` | Delegation | Task delegation | Chat trigger | Direct call | **Tested** |
| 27 | `mcp_connection` | MCP | MCP server connection management | Chat trigger | Direct call | **Tested** |
| 28 | `update_plan` | Planning | Update execution plan | Chat trigger | Direct call | **Tested** |
| 29 | `process` | System | Process management | Chat trigger | Direct call | **Tested** |
| 30 | `pdf` | Document | PDF generation | Chat trigger | Direct call | **Tested** |
| 31 | `image` | Media | Image processing | Chat trigger | Direct call | **Tested** |
| 32 | `image_generate` | Media | Image generation | -- | -- | **Untested** |
| 33 | `tts` | Media | Text-to-speech | Chat trigger | Direct call | **Tested** |
| 34 | `nodes` | Network | Tailscale node management | Chat trigger | Direct call | **Tested** |
| 35 | `canvas` | UI | Canvas / dynamic UI | Chat trigger | Direct call | **Tested** |

## Test File Summary

### `tests/e2e_websocket.rs` (57 tests)

Tests tools through the **WebSocket frontend simulation** path:

- **Chat-triggered tools** (indirect): The LLM decides to call a tool during chat processing. Tests verify `tool.calling` and `tool.result` events are emitted.
  - `llm_tool_invocation_journey` -- triggers `time` tool via "What is the current date and time?"
  - `tool_shell_invoked_via_chat` -- triggers `shell` tool via explicit prompt
  - `tool_file_invoked_via_chat` -- triggers `file_write` / `file_read` via explicit prompt
  - `tool_file_edit_invoked_via_chat` -- triggers `file_edit` tool via explicit prompt
  - `tool_todo_invoked_via_chat` -- triggers `todo` tool via explicit prompt
  - `tool_code_exec_invoked_via_chat` -- triggers `execute_code` tool via explicit prompt
  - `tool_web_fetch_invoked_via_chat` -- triggers `web_fetch` tool via explicit prompt
  - `tool_web_search_invoked_via_chat` -- triggers `web_search` tool via explicit prompt
  - `tool_memory_invoked_via_chat` -- triggers `memory` tool via explicit prompt
  - `tool_memory_search_invoked_via_chat` -- triggers `memory_search` tool via explicit prompt
  - `tool_memory_get_invoked_via_chat` -- triggers `memory_get` tool via explicit prompt
  - `tool_glob_invoked_via_chat` -- triggers `glob` tool via explicit prompt
  - `tool_grep_invoked_via_chat` -- triggers `grep` tool via explicit prompt
  - `tool_process_invoked_via_chat` -- triggers `process` tool via explicit prompt
  - `tool_nodes_invoked_via_chat` -- triggers `nodes` tool via explicit prompt
  - `tool_update_plan_invoked_via_chat` -- triggers `update_plan` tool via explicit prompt
  - `tool_canvas_invoked_via_chat` -- triggers `canvas` tool via explicit prompt
  - `tool_pdf_invoked_via_chat` -- triggers `pdf` tool via explicit prompt
  - `tool_image_invoked_via_chat` -- triggers `image` tool via explicit prompt
  - `tool_tts_invoked_via_chat` -- triggers `tts` tool via explicit prompt
  - `tool_cron_invoked_via_chat` -- triggers `cron` tool via explicit prompt
  - `tool_acp_spawn_invoked_via_chat` -- triggers `acp_spawn` tool via explicit prompt
  - `tool_acp_session_invoked_via_chat` -- triggers `acp_session` tool via explicit prompt
  - `tool_sessions_list_invoked_via_chat` -- triggers `sessions_list` tool via explicit prompt
  - `tool_sessions_history_invoked_via_chat` -- triggers `sessions_history` tool via explicit prompt
  - `tool_sessions_send_invoked_via_chat` -- triggers `sessions_send` tool via explicit prompt
  - `tool_sessions_yield_invoked_via_chat` -- triggers `sessions_yield` tool via explicit prompt
  - `tool_session_status_invoked_via_chat` -- triggers `session_status` tool via explicit prompt
  - `tool_subagents_invoked_via_chat` -- triggers `subagents` tool via explicit prompt
  - `tool_apply_patch_invoked_via_chat` -- triggers `apply_patch` tool via explicit prompt
  - `tool_delegate_invoked_via_chat` -- triggers `delegate` tool via explicit prompt
  - `tool_mcp_connection_invoked_via_chat` -- triggers `mcp_connection` tool via explicit prompt

- **Command-query tools**:
  - `command_tools_returns_catalog` -- verifies `/tools` slash command returns tool names including "shell", "file"
  - `command_skill_lists_skills` -- verifies `/skill` command lists installed skills
  - `command_skill_not_found` -- verifies error for nonexistent skill
  - `command_mcp_returns_server_info` -- verifies `/mcp` command returns MCP status
  - `command_mcp_disconnect_requires_arg` -- verifies error for missing MCP server name

### `tests/integrations_live.rs` (116 tests)

Tests tools via **direct `Tool::execute()` invocation** (no Gateway / WebSocket):

| Test | Tool Tested |
|------|------------|
| `shell_tool_executes_echo` | `shell` |
| `file_read_write_cycle` | `file_read`, `file_write` |
| `file_edit_tool_replaces_content` | `file_edit` |
| `glob_tool_lists_files` | `glob` |
| `grep_tool_finds_patterns` | `grep` |
| `time_tool_returns_timestamp` | `time` |
| `todo_tool_adds_and_lists` | `todo` |
| `web_fetch_tool_fetches_example_com` | `web_fetch` |
| `process_tool_lists_processes` | `process` |
| `nodes_tool_returns_definitions` | `nodes` |
| `code_exec_tool_runs_python` | `execute_code` |
| `memory_tool_creates_and_reads` | `memory` |
| `memory_search_tool_searches` | `memory_search` |
| `memory_get_tool_crud` | `memory_get` |
| `update_plan_tool_crud` | `update_plan` |
| `cron_tool_list_without_scheduler` | `cron` |
| `pdf_tool_generates_output` | `pdf` |
| `image_tool_reads_temp_file` | `image` |
| `delegate_tool_spawn_without_agent` | `delegate` |
| `mcp_connection_tool_lists_empty` | `mcp_connection` |
| `web_search_tool_duckduckgo` | `web_search` |
| `tts_tool_falls_back_without_key` | `tts` |
| `canvas_tool_presents` | `canvas` |
| `acp_spawn_tool_executes_without_agent_builder` | `acp_spawn` |
| `acp_session_tool_lists_sessions` | `acp_session` |
| `sessions_list_tool_lists_sessions` | `sessions_list` |
| `sessions_history_tool_returns_history` | `sessions_history` |
| `sessions_send_tool_fails_for_missing_subagent` | `sessions_send` |
| `sessions_yield_tool_fails_for_missing_subagent` | `sessions_yield` |
| `session_status_tool_requires_id` | `session_status` |
| `subagents_tool_lists_subagents` | `subagents` |
| `apply_patch_tool_validates_patch` | `apply_patch` |

## Untested Tools (2 total)

The following tools have **no E2E or integration test coverage**:

| Tool | Reason / Blocker |
|------|-----------------|
| `browser` | Requires `browser` feature flag + Chrome/Chromium (not enabled in default build) |
| `image_generate` | Requires image generation API key (no fallback path) |

## ACP Tools

The following ACP tools are verified via **direct `execute()` invocation** in `integrations_live.rs`. They are also tested via chat-triggered E2E tests in `e2e_websocket.rs`:

| Tool | Integration Test | E2E Chat Test |
|------|-----------------|---------------|
| `acp_spawn` | `acp_spawn_tool_executes_without_agent_builder` | `tool_acp_spawn_invoked_via_chat` |
| `acp_session` | `acp_session_tool_lists_sessions` | `tool_acp_session_invoked_via_chat` |
| `sessions_list` | `sessions_list_tool_lists_sessions` | `tool_sessions_list_invoked_via_chat` |
| `sessions_history` | `sessions_history_tool_returns_history` | `tool_sessions_history_invoked_via_chat` |
| `sessions_send` | `sessions_send_tool_fails_for_missing_subagent` | `tool_sessions_send_invoked_via_chat` |
| `sessions_yield` | `sessions_yield_tool_fails_for_missing_subagent` | `tool_sessions_yield_invoked_via_chat` |
| `session_status` | `session_status_tool_requires_id` | `tool_session_status_invoked_via_chat` |
| `subagents` | `subagents_tool_lists_subagents` | `tool_subagents_invoked_via_chat` |
| `apply_patch` | `apply_patch_tool_validates_patch` | `tool_apply_patch_invoked_via_chat` |

## Conditionally Tested Tools

The following tools are covered in both E2E and integration tests, but may not exercise full functionality due to missing dependencies:

| Tool | `integrations_live.rs` | `e2e_websocket.rs` | Limitation |
|------|------------------------|--------------------|------------|
| `web_search` | `web_search_tool_duckduckgo` | `tool_web_search_invoked_via_chat` | Requires network; may timeout |
| `cron` | `cron_tool_list_without_scheduler` | `tool_cron_invoked_via_chat` | Without scheduler: error; with scheduler: full test |
| `memory_search` | `memory_search_tool_searches` | `tool_memory_search_invoked_via_chat` | Uses SQLite store without vector embeddings |
| `delegate` | `delegate_tool_spawn_without_agent` | `tool_delegate_invoked_via_chat` | Without agent config: error; with config: full test |
| `mcp_connection` | `mcp_connection_tool_lists_empty` | `tool_mcp_connection_invoked_via_chat` | Without MCP servers: empty list |
| `pdf` | `pdf_tool_generates_output` | `tool_pdf_invoked_via_chat` | May fail without Chrome/headless browser |
| `image` | `image_tool_reads_temp_file` | `tool_image_invoked_via_chat` | Minimal PNG header test |
| `tts` | `tts_tool_falls_back_without_key` | `tool_tts_invoked_via_chat` | Without API key: fallback behavior |
| `canvas` | `canvas_tool_presents` | `tool_canvas_invoked_via_chat` | Tests basic present action |

## Privileged Tools

The following tools require `SkillTrust::Trusted` to execute:

```rust
shell, execute_code, file_write, file_edit, delegate,
acp_spawn, acp_session, memory,
sessions_send, sessions_yield, subagents, apply_patch,
message, process, image_generate
```

Community-trust skills are restricted from using these tools.

## How to Add Missing Tests

### Option 1: Direct Tool Execution (like `integrations_live.rs`)

```rust
#[tokio::test]
async fn web_fetch_tool_fetches_example_com() {
    let tool = WebFetchTool::new();
    let result = tool.execute(
        json!({"url": "https://example.com"}),
        ToolContext::default(),
    ).await;
    assert!(result.is_ok());
}
```

### Option 2: Chat-Triggered (like `e2e_websocket.rs`)

```rust
async fn tool_xyz_invoked_via_chat() {
    let mut client = FrontendSimulator::connect(port).await;
    client.send_chat(&sid, "Please use the xyz tool...").await;
    let payload = client.wait_for_event("tool.calling", 60).await;
    assert_eq!(payload["tool_name"], "xyz");
}
```

### Option 3: MCP HTTP Endpoint

For tools exposed via MCP, test through the HTTP JSON-RPC interface:

```http
POST /mcp/v1/tools/call
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "time",
    "arguments": {}
  }
}
```
