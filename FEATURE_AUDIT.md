# Manta Feature Integration Audit

## Summary

| Feature | Status | Integration Level | Notes |
|---------|--------|-------------------|-------|
| **Core/Gateway** | ✅ WIRED | Full | Main control plane operational |
| **Vector Memory** | ✅ WIRED | Full | Initialized in Gateway, API endpoints exposed |
| **Local GGUF Embeddings** | ✅ WIRED | Full | Lazy initialization with HF Hub support |
| **Plugins** | ✅ WIRED | Full | Initialized in Gateway::start(), management API exposed |
| **Hot Reload** | ✅ WIRED | Partial | Initialized but file watcher not actively used for config changes |
| **ACP (Agent Control Plane)** | ✅ WIRED | Full | Tools registered, API endpoints exposed |
| **Cron Scheduler** | ✅ WIRED | Full | Background task started, CronTool registered |
| **Canvas/A2UI** | ✅ WIRED | Full | Routes registered, handlers implemented |
| **Security (Auth/Rate Limit)** | ⚠️ PARTIAL | Partial | Initialized but NOT applied as middleware to routes |
| **Storage Adapter** | ⚠️ PARTIAL | Partial | Initialized in state but not actively used by other components |
| **Model Router** | ✅ WIRED | Full | Providers configured, health checks, fallback chains |
| **Tools** | ⚠️ PARTIAL | Partial | Most tools registered, but some missing (MemoryTool, DelegateTool, McpConnectionTool) |
| **Channels** | ⚠️ PARTIAL | Partial | Webhook routes exist, but channel adapters not fully integrated |
| **Skills** | ⚠️ STANDALONE | Standalone | Only used in CLI, not integrated into Gateway/Agent |
| **Tailscale** | ✅ WIRED | Feature-gated | Feature flag enables Tailscale integration |
| **Web Terminal** | ✅ WIRED | Full | Spawned in background, serves web UI |

---

## Detailed Analysis

### ✅ FULLY WIRED

#### 1. Vector Memory + Local GGUF Embeddings
**Files:** `src/memory/vector.rs`, `src/memory/local_embeddings.rs`, `src/gateway/mod.rs:595-674`

- Initialized in `Gateway::new()` based on config
- Supports OpenAI and LocalGguf providers
- HuggingFace Hub auto-download implemented
- API endpoints: `/api/v1/memory/search`, `/api/v1/memory/add`, `/api/v1/memory/collections`
- Lazy loading pattern for GGUF models

**Status:** Production ready

---

#### 2. Plugins (WASM)
**Files:** `src/plugins/`, `src/gateway/mod.rs:726-737`, `src/gateway/mod.rs:2194-2233`

- `PluginManager` created in `Gateway::new()`
- Initialized in `Gateway::start()` if enabled
- Management API exposed: `/api/v1/plugins/*`
- Hot loading/unloading supported

**Status:** Fully integrated

---

#### 3. ACP (Agent Control Plane)
**Files:** `src/acp/`, `src/gateway/mod.rs:861-865`, `src/tools/acp_tool.rs`

- `AcpControlPlane` created in `Gateway::new()`
- Tools registered: `AcpSpawnTool`, `AcpSessionTool`
- API endpoints for subagent management
- Thread binding for conversation continuity

**Status:** Fully integrated

---

#### 4. Cron Scheduler
**Files:** `src/cron/`, `src/gateway/mod.rs:693-711`, `src/tools/cron_tool.rs`

- Background task started in `Gateway::new()`
- `CronTool` registered in tool registry
- Persistent job scheduling supported

**Status:** Fully integrated

---

#### 5. Canvas/A2UI
**Files:** `src/canvas/`, `src/gateway/mod.rs:825`, `src/gateway/mod.rs:838-840`, `src/gateway/mod.rs:1784-1899`

- `CanvasManager` created in `Gateway::new()`
- WebSocket endpoint: `/ws/canvas/:id`
- REST API endpoints: `/api/v1/canvas`, `/api/v1/canvas/:id`
- Full CRUD handlers implemented

**Status:** Fully integrated

---

#### 6. Model Router
**Files:** `src/model_router/`, `src/gateway/mod.rs:530-593`, `src/gateway/mod.rs:841-848`

- Provider health monitoring
- Automatic failover between providers
- Model aliasing (fast, smart, default)
- Runtime provider switching via API
- Configured from Gateway config

**Status:** Fully integrated

---

#### 7. Web Terminal
**Files:** `src/gateway/mod.rs:793-797`, `src/web.rs`

- Spawned in background in `Gateway::start()`
- Serves web UI on configured web_port

**Status:** Fully integrated

---

### ⚠️ PARTIALLY INTEGRATED

#### 8. Hot Reload
**Files:** `src/config/hot_reload.rs`, `src/gateway/mod.rs:676-691`, `src/gateway/mod.rs:739-753`

**Wired:**
- `HotReloadManager` created in `Gateway::new()`
- File watcher started in `Gateway::start()`

**Gap:**
- No actual config change handlers registered
- Not used to dynamically update running config
- Mainly boilerplate, not functional for live updates

**Status:** Skeleton implementation

---

#### 9. Security (AuthManager + RateLimiter)
**Files:** `src/security/`, `src/gateway/mod.rs:539-545`, `src/gateway/mod.rs:378-380`

**Wired:**
- `AuthManager` and `RateLimiter` created in `Gateway::new()`
- Stored in `GatewayState`
- Security config structs defined

**Gap:**
- ❌ **NOT applied as middleware to any routes!**
- Only `tailscale_only_middleware` is applied (network-level)
- No authentication required for admin APIs
- No rate limiting actually enforced
- Auth endpoints not exposed

**Status:** Initialized but not enforced

---

#### 10. Storage Adapter
**Files:** `src/adapters/`, `src/gateway/mod.rs:547-563`, `src/gateway/mod.rs:382`

**Wired:**
- Storage initialized based on config (sqlite/file/memory)
- Stored in `GatewayState`

**Gap:**
- Not used by VectorMemory (uses its own storage)
- Not used by Agent (uses its own memory)
- Not used by PluginManager
- No persistence layer actually connected

**Status:** Placeholder implementation

---

#### 11. Tools
**Files:** `src/tools/`, `src/gateway/mod.rs:1330-1368`

**Registered Tools:**
- ✅ File tools: FileReadTool, FileWriteTool, FileEditTool, GlobTool, GrepTool
- ✅ Shell: ShellTool
- ✅ Code execution: CodeExecutionTool
- ✅ Web: WebSearchTool, WebFetchTool
- ✅ Todo: TodoTool
- ✅ Cron: CronTool
- ✅ Time: TimeTool
- ✅ ACP: AcpSpawnTool, AcpSessionTool
- ✅ Browser: BrowserTool (feature-gated)

**Missing from Registry:**
- ❌ `MemoryTool` - Not registered!
- ❌ `DelegateTool` - Not registered!
- ❌ `McpConnectionTool` - Not registered!

**Status:** Partial - some tools not available to agents

---

#### 12. Channels
**Files:** `src/channels/`, `src/gateway/mod.rs:1037-1052`, `src/gateway/webhooks.rs`

**Wired:**
- Webhook routes registered for WhatsApp, Telegram, Feishu
- `init_channels()` called in `Gateway::start()`

**Gap:**
- Channels just log "will be initialized by adapter"
- No actual channel adapters spawned
- No active channel connections (Telegram bot polling, Discord gateway, etc.)
- Channel registry in state stays empty

**Status:** Webhooks ready but no active channel connections

---

### ❌ STANDALONE (Not Integrated)

#### 13. Skills
**Files:** `src/skills/`, `src/cli.rs`

**Status:**
- Only used in CLI commands (`src/cli.rs`)
- `SkillManager` not created in Gateway
- Not integrated into Agent or Tool system
- Hot reload for skills not connected to running system

**Gap:** Completely standalone - only accessible via CLI

---

## Recommendations

### High Priority

1. **Apply Security Middleware**
   ```rust
   // In build_router(), add:
   .layer(from_fn_with_state(state.clone(), auth_middleware))
   .layer(from_fn_with_state(state.clone(), rate_limit_middleware))
   ```

2. **Register Missing Tools**
   Add to `create_default_tool_registry()`:
   - `MemoryTool`
   - `DelegateTool`
   - `McpConnectionTool`

3. **Connect Storage Adapter**
   - Use storage for VectorMemory persistence
   - Use storage for Agent memory
   - Use storage for Plugin data

### Medium Priority

4. **Activate Channel Adapters**
   Spawn channel adapter tasks in `init_channels()` based on config

5. **Complete Hot Reload**
   Register config change handlers that update running system

6. **Integrate Skills**
   Create SkillsManager in Gateway, integrate with Agent tool system

### Low Priority

7. **Add Auth Endpoints**
   Expose login/register endpoints for AuthManager

8. **Canvas Event Handling**
   Complete the Canvas event processing (currently receives but doesn't process events)
