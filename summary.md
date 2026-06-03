# Syscity vs OpenClaw — 差距分析报告

> 本文档对比 Syscity（Rust 实现，约 83.6K LOC）与其参考原型 OpenClaw（TypeScript 实现，约 750K+ TypeScript LOC + 142K Swift/Kotlin）。

---

## 一、项目规模与工程成熟度

| 维度 | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| **总代码量** | ~750K TS + 111K Swift + 31K Kotlin ≈ **892K LOC** | ~83.6K Rust ≈ **84K LOC** | **10.6x** |
| **模块/扩展数** | 126 个 extensions + 50+ src 子模块 | 35 个 pub mod（无 extensions） | OpenClaw 模块数 ≈ **4–5x** |
| **测试** | 4,798 个 `.test.ts` 文件 | 519 个 `#[test]`，107 文件 | **~9x** 测试文件数 |
| **文档** | 469 个 Markdown，~118K LOC | README + CHANGELOG + plan.md | OpenClaw 文档量 **~50x** |
| **CI/交付** | 成熟 CI、Docker、Podman、Nix、自动发布 | 基础 CI（fmt/clippy/test/audit） | 差一个量级 |
| **原生客户端** | macOS menu-bar、iOS preview、Android preview | 无 | 仅有 CLI 入口 |
| **Web UI** | Vite + Lit SPA，实时 WebSocket（~94K LOC） | SSE 占位符 + 未实装前端面板 | 差一个完整产品 |

**关键结论**：Syscity 在代码量上是 OpenClaw 的 **~1/10**，模块数为 **~1/4**，测试覆盖为 **~1/9**。这不是"Rust 更紧凑"能解释的——OpenClaw 的 channel 扩展（仅 Discord 就 72K LOC）单一个扩展就接近整个 Syscity。

---

## 二、Channel（消息通道）对比

| Channel | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| Telegram | ✅ 完整（70K LOC） | ✅ 完整 | 平齐 |
| Discord | ✅ 完整（72K LOC） | ✅ 完整 | 平齐 |
| Slack | ✅ 完整（40K LOC），双向 | ❌ 仅出站，`start()` 不订阅入站 | **缺 inbound** |
| WhatsApp | ✅ 完整（31K LOC），双向 | ❌ 仅出站 | **缺 inbound + 验签 bug** |
| QQ | ✅ 完整（30K LOC），双向 | ❌ 仅出站 | **缺 inbound** |
| Feishu/Lark | ✅ 完整（44K LOC），双向 | ❌ 仅出站 | **缺 inbound** |
| Signal | ✅ | ❌ 无 | 缺失 |
| iMessage | ✅ (BlueBubbles) | ❌ 无 | 缺失 |
| Google Chat | ✅ | ❌ 无 | 缺失 |
| Microsoft Teams | ✅ | ❌ 无 | 缺失 |
| Matrix | ✅ | ❌ 无 | 缺失 |
| IRC | ✅ | ❌ 无 | 缺失 |
| LINE | ✅ | ❌ 无 | 缺失 |
| Mattermost | ✅ | ❌ 无 | 缺失 |
| Nextcloud Talk | ✅ | ❌ 无 | 缺失 |
| Nostr | ✅ | ❌ 无 | 缺失 |
| Twitch | ✅ | ❌ 无 | 缺失 |
| Zalo | ✅ | ❌ 无 | 缺失 |
| 网页/WebChat | ✅ 内置 | ❌ 无 | 缺失 |
| **Voice Call** | ✅ 独立扩展 | ❌ 无 | 缺失 |

**差距量化**：OpenClaw 支持 **26+** 通道，全部双向；Syscity 仅 **6** 个通道声明支持，其中仅 **2** 个双向，其余 4 个有 outbound 无 inbound。Syscity 的 Slack/WhatsApp/QQ/Lark 入站缺失意味着用户在这些平台发消息给 bot，bot **不会收到**。

---

## 三、LLM Provider 对比

| Provider | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| OpenAI | ✅ | ✅ | 平齐 |
| Anthropic (Claude) | ✅ | ✅ | 平齐 |
| Google (Gemini) | ✅ | ❌ | 缺失 |
| Azure / Azure Speech | ✅ | ❌ | 缺失 |
| Amazon Bedrock | ✅ | ❌ | 缺失 |
| xAI (Grok) | ✅ | ❌ | 缺失 |
| Ollama (本地) | ✅ | ❌ | 缺失 |
| Groq | ✅ | ❌ | 缺失 |
| Mistral | ✅ | ❌ | 缺失 |
| Moonshot | ✅ | ❌ | 缺失 |
| DeepSeek | ✅ | ❌ | 缺失 |
| Perplexity | ✅ | ❌ | 缺失 |
| Qwen | ✅ | ❌ | 缺失 |
| Together / Fireworks | ✅ | ❌ | 缺失 |
| OpenRouter | ✅ | ❌ | 缺失 |
| LiteLLM | ✅ | ❌ | 缺失 |
| LM Studio | ✅ | ❌ | 缺失 |
| vLLM / SGLang | ✅ | ❌ | 缺失 |
| 本地 GGUF (llama-cpp) | 通过 Ollama/LM Studio | `local-embeddings` feature，仅 embedding | 缺对话推理 |
| **Fallback 链** | 有 | 有 | 平齐 |
| **模型路由 + 熔断器** | 有 | 有 | 平齐 |
| **Provider 插件化** | 126 个 extensions 动态加载 | 硬编码在 core | 差距巨大 |

**差距量化**：OpenClaw 支持 **40+** provider；Syscity 仅 **2** 个（OpenAI + Anthropic）。Syscity 虽有 fallback 链和 model_router，但 router 中声明的 Azure/Ollama/Custom 变体在 `create_provider` 中直接返回 `InvalidValue` 错误（未实现）。

---

## 四、Memory 系统对比

| 能力 | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| **SQLite + BM25 + Vector Hybrid** | ✅ memory-core（42K LOC） | ✅ `memory/db.rs` + `hybrid.rs` | 平齐（Syscity 实现正确） |
| **FTS5** | 通过 memory-core | ✅ `session_search.rs` | 平齐 |
| **LanceDB 后端** | ✅ memory-lancedb | ❌ 无 | 缺失 |
| **Wiki/Obsidian 同步** | ✅ memory-wiki（13K LOC） | ❌ 无 | 缺失 |
| ** dreaming / 记忆巩固** | ✅ Light/Deep/REM 阶段 + Dream Diary | ❌ 无 | 缺失 |
| **QMD sidecar（本地优先搜索）** | ✅ 可选升级路径 | ❌ 无 | 缺失 |
| **Embedding 提供商** | OpenAI, Gemini, Voyage, Mistral, Ollama, local (node-llama-cpp) | OpenAI API + `llama-cpp-2` GGUF（feature-gated） | Syscity embedding 选择少 |
| **MEMORY.md / 人格记忆** | ✅ 完整加载 | ✅ `personality.rs` | 平齐 |
| **Workspace state** | ✅ | ✅ `workspace_state.rs` | 平齐 |
| **Memory flush / compaction** | ✅ | ✅ `flush.rs` + `compaction.rs` | 平齐 |
| **Temporal decay + MMR** | ✅ | ✅ `hybrid.rs` | 平齐 |
| **实际 hybrid 检索** | ✅ 真实融合 | ⚠️ `MemorySearchTool` 降级为 LIKE | **Syscity 有实现缺陷** |

**差距量化**：核心 SQLite hybrid 搜索两者都实现了，但 OpenClaw 有 **3 个可选后端** + **dreaming** + **Wiki 同步**，Syscity 只有单一 SQLite 路径，且 `MemorySearchTool` 未真正调用 hybrid 路径。Syscity 的 `local-embeddings` 仅支持 embedding（没有本地 GGUF 对话推理）。

---

## 五、Agent / Tool / Skill 系统对比

| 能力 | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| **Agent runtime** | pi-agent-core 嵌入（384K LOC） | `agent/mod.rs` + 13 子模块 | OpenClaw 更成熟 |
| **Multi-agent routing** | ✅ 多 agent + 隔离 workspace | ⚠️ `session.rs` 仅置 busy，不实际路由 | **Syscity 未实现** |
| **Tool harness + sandbox** | ✅ Docker-based 可选 | ⚠️ `libc::setrlimit` 已移除，cgroups 未替代 | **Syscity sandbox 弱** |
| **Approval gate / 审批队列** | ✅ 有 | ✅ `approval.rs` | 平齐 |
| **Skill 生态 (ClawHub)** | ✅ 53 built-in + ClawHub 公共注册表 | ✅ 13 built-in skills + OpenClaw frontmatter 兼容 | Syscity 数量少 |
| **Skill 安装方式** | npm/git/ClawHub | brew/npm/uv/go/cargo/shell/download | 平齐（Syscity 甚至更多安装器） |
| **Skill workshop** | ✅ 捕获可重复工作流 | ❌ 无 | 缺失 |
| **Plugin SDK** | ✅ `plugin-sdk` + `memory-host-sdk` | ❌ 无正式 SDK | 缺失 |
| **Plugin 扩展化** | ✅ 126 个 extensions 动态加载 | ⚠️ `plugins/mod.rs` + `extensions/plugin_host.rs` (WASM) | Syscity 有 WASM 但无生态 |
| **Browser automation** | ✅ Playwright/CDP + profiles + tabs（~24K LOC） | ⚠️ `chromiumoxide` 可选，基础 action | OpenClaw 远更完整 |
| **Code execution** | ✅ sandboxed Docker | ⚠️ Python-only，无内存限制 | **Syscity 隔离不足** |
| **Web search** | ✅ DuckDuckGo + SearXNG + Perplexity + Brave | ⚠️ DuckDuckGo HTML scrape（易碎） | OpenClaw 选择多 |
| **Web fetch** | ✅ | ✅ | 平齐 |
| **File read/write/edit** | ✅ | ✅ | 平齐 |
| **Shell exec** | ✅ | ✅ | 平齐 |
| **Grep** | ✅ | ✅ | 平齐 |
| **Cron** | ✅ | ✅ | 平齐 |
| **Canvas / A2UI** | ✅ canvas-host（live reload） | ✅ `canvas/mod.rs` | 平齐 |
| **TTS / STT / Voice** | ✅ 多个 provider + realtime voice | ❌ 无 | 缺失 |
| **Image generation** | ✅ 多个 provider | ❌ 无 | 缺失 |
| **Video generation** | ✅ 多个 provider | ❌ 无 | 缺失 |
| **Music generation** | ✅ 多个 provider | ❌ 无 | 缺失 |
| **Lobster (flow runtime)** | ✅ 可恢复审批工作流 | ❌ 无 | 缺失 |
| **Trajectory recording** | ✅ 诊断/审计轨迹导出 | ❌ 无 | 缺失 |
| **Standing orders** | ✅ 永久自主程序 | ❌ 无 | 缺失 |

**差距量化**：OpenClaw 的工具生态是 **插件化 + 多后端** 的；Syscity 将许多功能硬编码在 core 中，没有插件注册表。Syscity 的浏览器自动化、代码执行、web 搜索都处于 MVP 级别。Voice、媒体生成、Lobster flow runtime 完全缺失。

---

## 六、Security / Auth 对比

| 能力 | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| **Audit engine** | ✅ 50+ checkId，`security audit` CLI | ⚠️ `audit.rs` 硬编码报告 | **Syscity 审计是桩** |
| **Pentest** | ✅ 动态探测 | ⚠️ `pentest.rs` 两个测试恒过 | **Syscity 渗透测试是桩** |
| **Secret scanning** | ✅ 多模式，低误报 | ⚠️ `SecretScanner::contains_secrets` 返回反了 | **Syscity 有 bug** |
| **Rate limiting** | ✅ Token bucket + sliding window | ⚠️ SlidingWindow `Default` 无限递归 | **Syscity 有 bug** |
| **Webhook 签名验证** | ✅ HMAC-SHA256，常量时间比较 | ⚠️ 签名头缺失时 fail-open，`==` 比较 | **Syscity 有安全漏洞** |
| **WhatsApp 验签** | ✅ 正确 | ❌ 用 access_token 而非 app_secret，重序列化 body | **Syscity 永远验签失败** |
| **Pairing / DM policy** | ✅ 完整（challenge/response + allowlist） | ✅ `pairing.rs` + `Allowlist` | 平齐（Syscity 功能对） |
| **Device fingerprinting** | ✅ | ✅ `fingerprint.rs` | 平齐 |
| **Security headers** | ✅ | ✅ `headers.rs` | 平齐 |
| **Auth profiles** | ✅ per-agent `auth-profiles.json` | ❌ 全局单一 | 缺失 |
| **Plugin trust model** | ✅ 社区/可信分级 | ⚠️ `SkillTrust` 声明但未充分使用 | 弱 |
| **Sandbox browser config** | ✅ | ⚠️ `SandboxedTool` 仅名字启发式 | 弱 |
| **Formal verification** | ✅ TLA+/TLC（单独仓库） | ❌ 无 | 缺失 |
| **MITRE ATLAS threat model** | ✅ 文档化 | ❌ 无 | 缺失 |
| **Docker sandbox** | ✅ 可选 | ❌ 无 | 缺失 |

**差距量化**：OpenClaw 的安全体系是 **可运行的生产级**；Syscity 的安全模块有 **4 个高危 bug**（递归、逻辑反、fail-open、验签密钥错）+ **多个恒过桩**。Syscity 的 pentest 和 audit 目前只能出"看起来有报告"的幻觉，没有实际探测能力。

---

## 七、Gateway / Server 对比

| 能力 | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| **Gateway 结构** | 737 文件，~187K LOC，高度模块化 | 4 文件 + `mod.rs`（7268 行单文件） | **Syscity 严重 overdue 拆分** |
| **WebSocket server** | ✅ 核心通信方式 | ✅ `server/mod.rs` + `gateway` | 平齐 |
| **HTTP API / REST** | ✅ 完整 RPC 方法集 | ✅ 基本 CRUD | OpenClaw 更全 |
| **OpenAI-compatible API** | ✅ `/v1/chat/completions` | ✅ `/v1/chat/completions` | 平齐 |
| **SSE streaming** | ✅ | ⚠️ 占位符 | **Syscity 未实装** |
| **Control UI (Web)** | ✅ Vite + Lit SPA（~94K LOC） | ❌ 无前端面板 | 缺失 |
| **TUI** | ✅ `crestodian/` + `tui/` | ❌ 无 | 缺失 |
| **Native companion apps** | ✅ macOS/iOS/Android | ❌ 无 | 缺失 |
| **Node connections** | ✅ iOS/Android/macOS headless nodes | ❌ 无 | 缺失 |
| **Send policy engine** | ✅ 完整规则引擎 + glob | ✅ `send_policy.rs` | 平齐 |
| **Middleware 栈** | ✅ auth + rate limit + tailscale + security headers | ✅ 同左 | 平齐 |
| **Cron announce → SSE** | ✅ | ✅ | 平齐 |
| **MCP server surface** | ✅ `openclaw mcp serve` + bridge | ⚠️ `mcp.rs` 仅 client | **Syscity 缺 MCP server** |
| **ACP bridge (IDE)** | ✅ `openclaw acp` stdio bridge | ❌ 无 | 缺失 |
| **ACPX** | ✅ 嵌入式 ACP runtime backend | ❌ 无 | 缺失 |
| **Wizard / Onboard** | ✅ `openclaw onboard` 交互式引导 | ❌ 无 | 缺失 |
| **Auto-reply dispatch** | ✅ 智能回复调度 | ❌ 无 | 缺失 |
| **Device pairing** | ✅ 首次连接配对 | ❌ 无 | 缺失 |
| **Realtime voice relay** | ✅ voiceclaw-realtime | ❌ 无 | 缺失 |

**差距量化**：Syscity 的 gateway 是一个 **7K 行单文件**，而 OpenClaw 的 gateway 是 **737 文件、187K LOC** 的模块化控制平面。Syscity 缺少 Web UI、TUI、原生应用、IDE 桥接、MCP server 表面、向导流程——这些都是"产品化"层面而非"工程化"层面的差距。

---

## 八、CLI 对比

| 能力 | OpenClaw | Syscity | 差距 |
|---|---|---|---|
| **命令数量** | ~30+ 子命令 | ~22 个子命令 | 接近 |
| **Onboard / Wizard** | ✅ 交互式首次配置 | ❌ 无 | 缺失 |
| **Doctor / 诊断** | ✅ `openclaw doctor` | ❌ 无 | 缺失 |
| **Security audit CLI** | ✅ 50+ checks + `--fix` | ✅ 有但底层是桩 | 表面平齐 |
| **MCP CLI** | ✅ `mcp serve` / `mcp bridge` | ✅ `mcp` list/connect/call | Syscity 缺 serve |
| **Chat REPL** | ✅ | ✅ | 平齐 |
| **Web 终端** | ✅ 内置 WebSocket 聊天 | ⚠️ `run_web` 仅打印说明 | **Syscity 是桩** |
| **Log tail** | ✅ | ✅ | 平齐 |
| **Daemon 管理** | ✅ | ✅ | 平齐 |
| **Export** | ✅ conversations/memories | ✅ conversations/memories/all | 平齐 |

---

## 九、缺失的核心产品特性

以下特性在 OpenClaw 中存在，在 Syscity 中 **完全缺失**（无任何代码或桩）：

| 特性 | OpenClaw 中的形态 | Syscity 状态 |
|---|---|---|
| **Native companion apps** | macOS menu-bar、iOS、Android | 无 |
| **Web UI / Control UI** | Vite + Lit SPA (~94K LOC) | 无 |
| **TUI (Terminal UI)** | `crestodian/` + `tui/` | 无 |
| **Realtime voice / talk mode** | 连续语音对话 + 打断 | 无 |
| **Realtime transcription** | WebSocket 实时音频转录 | 无 |
| **Image generation** | DALL-E, Flux, Midjourney 等 | 无 |
| **Video generation** | Runway, Fal 等 | 无 |
| **Music generation** | 多个 provider | 无 |
| **TTS / STT** | ElevenLabs, Deepgram, Azure Speech 等 | 无 |
| **Dreaming / 记忆巩固** | Light/Deep/REM + Dream Diary | 无 |
| **Wiki 同步** | Obsidian vault 双向同步 | 无 |
| **LanceDB / 可选向量后端** | memory-lancedb | 无 |
| **Flow runtime (Lobster)** | 可恢复审批工作流 | 无 |
| **Trajectory recording** | 诊断轨迹导出 | 无 |
| **Standing orders** | 永久自主程序 | 无 |
| **ACPX / IDE 桥接** | `openclaw acp` stdio bridge | 无 |
| **MCP server surface** | `openclaw mcp serve` | 仅 client |
| **Wizard / Onboard** | 交互式首次引导 | 无 |
| **Auto-reply dispatch** | 智能回复调度 | 无 |
| **Docker sandbox** | 工具执行隔离 | 无 |
| **Playwright/CDP 浏览器** | 完整 profiles/tabs/snapshots | 仅 chromiumoxide 基础 |
| **40+ LLM providers** | 插件化动态加载 | 仅 2 个 |
| **20+ messaging channels** | 插件化动态加载 | 仅 6 个，4 个 inbound 缺失 |
| **Plugin SDK / memory-host-sdk** | 公开扩展接口 | 无 |
| **ClawHub 生态** | 公共技能/插件注册表 | 无 |

---

## 十、Syscity 做得好的地方（相对优势）

尽管差距巨大，Syscity 在以下方面做得不错：

1. **Rust 工程规范**：`cargo fmt` / `cargo clippy` 干净，0 编译错误，23 条仅为 dead-code 警告。OpenClaw 的 TypeScript 代码量虽然大，但维护成本也高。
2. **紧凑的核心实现**：83.6K LOC 覆盖了 OpenClaw 最核心的 20% 功能（Telegram/Discord、OpenAI/Anthropic、SQLite hybrid memory、基本工具集、Gateway、Cron）。
3. **WASM 插件 host**：OpenClaw 的插件是 npm/TypeScript 扩展；Syscity 尝试了 WASM (wasmtime) 插件，安全边界更硬。虽然无生态，但技术选型合理。
4. **Skill 安装器多样**：brew/npm/uv/go/cargo/shell/download 七种方式，甚至超过了 OpenClaw 的技能安装路径。
5. **Memory hybrid search 实现正确**：`memory/hybrid.rs` 的 BM25+vector 融合、temporal decay、MMR reranking 与 OpenClaw memory-core 的算法对齐，且有 ~20 个测试覆盖。
6. **Gateway 基本功能齐全**：虽然单文件 7K 行是债务，但 Axum 路由、中间件、WS handler、OpenAI-compatible API、webhook router、self-repair loop 都已存在。
7. **Zeroize 密钥处理**：`secrets.rs` 使用 `zeroize` crate，内存安全层面比 OpenClaw 的纯 JS secret 管理更严格。
8. **Feature flags 设计**：`telegram`/`discord`/`slack`/`browser`/`local-embeddings`/`plugins` 等 feature gate 组织合理。

---

## 十一、综合差距评估

| 维度 | OpenClaw 成熟度 | Syscity 成熟度 | 差距级别 |
|---|---|---|---|
| **架构 / 模块化** | ⭐⭐⭐⭐⭐ 高度插件化 | ⭐⭐⭐ 单 crate + 硬编码 | **大** |
| **Channel 覆盖** | ⭐⭐⭐⭐⭐ 26+ 双向 | ⭐⭐ 6 个，仅 2 双向 | **巨大** |
| **Provider 覆盖** | ⭐⭐⭐⭐⭐ 40+ | ⭐⭐ 2 个 | **巨大** |
| **Memory 系统** | ⭐⭐⭐⭐⭐ 多后端 + dreaming | ⭐⭐⭐ 单 SQLite，hybrid 有降级 | **中** |
| **Agent runtime** | ⭐⭐⭐⭐⭐ 成熟多 agent | ⭐⭐⭐ 基本单 agent，路由 stub | **大** |
| **Tool / Skill 生态** | ⭐⭐⭐⭐⭐ 插件化 + ClawHub | ⭐⭐⭐ 硬编码 13 skills | **大** |
| **Security** | ⭐⭐⭐⭐⭐ 生产级 + 形式验证 | ⭐⭐ 有功能但含高危 bug | **巨大** |
| **Gateway** | ⭐⭐⭐⭐⭐ 模块化控制平面 | ⭐⭐⭐ 单文件 7K 行，功能有 | **中** |
| **UI / 客户端** | ⭐⭐⭐⭐⭐ Web + TUI + 原生 App | ⭐ 仅有 CLI | **巨大** |
| **Voice / 媒体** | ⭐⭐⭐⭐⭐ 实时语音 + 生成 | ❌ 完全缺失 | **巨大** |
| **MCP / ACP / IDE** | ⭐⭐⭐⭐⭐ 双向桥接 | ⭐⭐ 仅 MCP client | **大** |
| **文档 / 生态** | ⭐⭐⭐⭐⭐ 469 docs + ClawHub | ⭐⭐ README + plan.md | **巨大** |

### 一句话总结

> **Syscity 是 OpenClaw 的约 10% 体积、20% 功能、50% 核心骨架的 Rust 复刻。**
>
> Syscity 已经搭好了最核心的一条路径（Telegram/Discord → Agent → OpenAI/Anthropic → SQLite Memory → Tools），但缺少 OpenClaw 赖以成为"产品"的一切外围：20+ 通道、40+ provider、原生客户端、Web UI、Voice、媒体生成、安全审计、插件生态、IDE 桥接。如果要让 Syscity 达到 OpenClaw 的生产可用水平，至少需要 **再补充 200–300K LOC**（主要是通道扩展、provider 扩展、前端、测试、文档），并修复现有的 4 个高危安全 bug 和多个 inbound 缺失问题。

---

## 十二、追平 OpenClaw 的路线图（MVP → Feature Product）

以下计划按**从 Foundation → Core Expansion → Feature Parity** 的三阶段推进。每个阶段都是前一个阶段的必要前提；不鼓励跳阶段。

---

### Phase 0：Foundation（0–2 周）— 让现有代码诚实可用

**目标**：修复所有会让用户踩坑的 bug，让 "声称已实现" 的功能真正可用。

| # | 任务 | 工作量 | 交付标准 |
|---|---|---|---|
| 0.1 | **修复 4 个高危安全 bug** | 2–3 天 | `sliding_window` 递归消除；`SecretScanner` 逻辑修正；webhook 签名强制校验（fail-closed）；WhatsApp 验签改用 `app_secret` + 原始 body 字节 |
| 0.2 | **修复 `MemorySearchTool` hybrid 降级** | 1–2 天 | `tools/memory.rs` 真正调用 `hybrid_search`（`DatabaseStore` + vector），不再回退 LIKE；补 3–5 个集成测试 |
| 0.3 | **修复 `agent/session.rs` 路由 stub** | 2–3 天 | `RouteToAgent` 真正触发目标 agent 的 `process_message`，而非仅置 busy；Broadcast 同理 |
| 0.4 | **清理 23 条 dead-code 警告** | 1–2 天 | 未读字段要么接通（`auto_load`、`cache_dir`、`default_timeout`），要么删除；未用函数删除或标记 `#[allow(dead_code)]` + 文档说明 |
| 0.5 | **修正 `plan.md` 诚实状态** | 0.5 天 | 将 `[✅]` 改为真实状态：`[✅]` 完成 / `[🚧]` 进行中 / `[⬜]` 待办 / `[⚠️]` 有已知缺陷 |
| 0.6 | **补齐 4 个通道的 inbound** | 3–5 天 | Slack Socket Mode、WhatsApp Cloud webhook、QQ WebSocket Gateway、Lark webhook 事件订阅接入 `message_tx` |
| 0.7 | **拆分 `gateway/mod.rs`** | 2–3 天 | 拆为 `handlers/`、`channels/`、`admin/`、`webhooks/`、`middleware/` 等子模块；单文件 < 1K 行 |

**预期产出**：Syscity 从"演示可用"升级到"诚实可用"；编译 0 警告；plan.md 反映真实进度。

---

### Phase 1：Core Expansion（3–8 周）— 补齐核心骨架

**目标**：让 Syscity 成为可用的多通道、多 provider、多 agent 产品基础，而非单一路径 demo。

#### 1.1 Provider 扩展（~3 周）

| Provider | 优先级 | 说明 |
|---|---|---|
| **Ollama** | P0 | 本地推理入口；社区最常用本地 provider |
| **Google (Gemini)** | P0 | 全球前三大 provider，必须支持 |
| **DeepSeek** | P1 | 高性价比编码模型，中文社区高频使用 |
| **Azure OpenAI** | P1 | 企业部署标准 |
| **Groq / Mistral / Moonshot** | P2 | 按社区需求排序 |

**技术路径**：抽象 `ProviderFactory` trait；每个 provider 一个文件（仿 `providers/openai.rs`）；model_router 中 Azure/Ollama/Custom 的 `InvalidValue` stub 替换为真实构造。

#### 1.2 Channel 扩展（~4 周）

| Channel | 优先级 | 说明 |
|---|---|---|
| **Signal** | P1 | 隐私导向用户刚需 |
| **Microsoft Teams** | P1 | 企业 IM 主战场 |
| **WebChat / 网页** | P1 | 产品化必备（嵌入网站） |
| **iMessage (BlueBubbles)** | P2 | macOS 用户高价值 |
| **Matrix / IRC** | P2 | 开源社区 |
| **Slack Socket Mode** | P0 | 已在 Phase 0 修复，此处是优化 |

**技术路径**：每个 channel 一个 `src/channels/{name}.rs`；实现 `Channel` trait；复用 `channels/health.rs`、`channels/metrics.rs`、`channels/state.rs` 基础设施。

#### 1.3 MCP Server 表面 + ACP 桥接（~2 周）

| 任务 | 说明 |
|---|---|
| **MCP Server** | 实现 `syscity mcp serve`（stdio + SSE），暴露 tools/memory/chat 到 MCP client |
| **ACP 桥接** | 参考 OpenClaw `openclaw acp`，实现 stdio ACP 桥，映射到 Gateway WebSocket |

#### 1.4 安全审计做实（~1 周）

| 任务 | 说明 |
|---|---|
| **Audit engine 动态化** | 将 `audit.rs` 硬编码结论替换为真实文件系统/配置扫描 |
| **Pentest 动态化** | `AuthenticationTest` / `DataExposureTest` 接入真实 HTTP 探测（参考 OpenClaw `ConfigurationTest`） |
| **Docker sandbox 可选** | 为 `code_exec.rs` / `browser.rs` 提供 Docker 隔离路径（feature-gated） |

**预期产出**：Syscity 支持 **8–10 个 provider**、**8–10 个双向 channel**、MCP serve、ACP 桥接；安全审计可运行。

---

### Phase 2：Feature Product（9–20 周）— 从"能跑"到"好用"

**目标**：补齐 OpenClaw 中让产品"好用"的一切外围：UI、Voice、媒体、插件生态。

#### 2.1 Web UI / Control UI（~4 周）

| 任务 | 说明 |
|---|---|
| **Web 终端** | 用 Vite + React/Vue（或保持 Rust 栈用 Leptos/Yew）实现 Gateway 内置聊天面板 |
| **Gateway 管理页** | channel 状态、provider 健康、session 列表、skill 管理 |
| **Device pairing** | 首次连接二维码/配对码，参考 OpenClaw `pairing.rs` |
| **Canvas A2UI** | 已有 `canvas/mod.rs`，补前端渲染层（WebSocket 接收 `CanvasUpdate`） |

**技术建议**：若团队前端能力强，用 React/Vite；若坚持全 Rust 栈，用 Leptos + Axum SSR，但开发速度会更慢。

#### 2.2 Voice / 实时交互（~3 周）

| 任务 | 说明 |
|---|---|
| **Realtime transcription** | WebSocket 音频流 → STT provider（Deepgram / Azure Speech） |
| **TTS** | 文本 → 音频，支持 ElevenLabs / Azure TTS / 本地 Piper |
| **Talk mode** | 连续语音对话循环（参考 OpenClaw `realtime-voice/`） |

#### 2.3 媒体生成（~2 周）

| 任务 | 说明 |
|---|---|
| **Image generation** | 接入 DALL-E / Stability / Flux provider |
| **(Optional) Video/Music** | 优先级最低；先 image 再考虑 |

#### 2.4 Memory 增强（~2 周）

| 任务 | 说明 |
|---|---|
| **Dreaming / 记忆巩固** | 后台定时任务，汇总会话生成 `DREAMS.md`；Light/Deep/REM 三阶段可选 |
| **Wiki / Obsidian 同步** | 读取 Obsidian vault，作为 memory search 的额外来源 |
| **LanceDB 可选后端** | 对大规模向量场景提供 LanceDB 替代 SQLite |

#### 2.5 插件 SDK + 注册表（~3 周）

| 任务 | 说明 |
|---|---|
| **Plugin SDK 发布** | 定义插件 manifest schema、API 契约、发布 npm crate / Rust crate |
| **Plugin registry (ClawHub-lite)** | 简单的 JSON registry，支持 `syscity plugin search` / `install` |
| **Bundle-style plugins** | 支持 skill + MCP server + config 打包安装 |

#### 2.6 原生客户端（~6 周，可与前端并行）

| 任务 | 说明 |
|---|---|
| **macOS menu-bar** | Swift / Tauri 最小化 wrapper，连接本地 Gateway |
| **iOS / Android** | 共享 Swift/Kotlin 代码，作为 Gateway 的 "node" |

**预期产出**：Syscity 有 Web UI、Voice、Image Gen、Wiki 同步、插件市场；达到 OpenClaw **60–70%** 功能覆盖。

---

### Phase 3：Full Parity（21–40 周）— 细节与生态

**目标**：追平 OpenClaw 的剩余 30% 差距，建立可持续的开发者生态。

| 领域 | 任务 | 说明 |
|---|---|---|
| **Channel 补全** | Signal、Zalo、Twitch、Nostr、LINE 等长尾通道 | 每个 1–2 周 |
| **Provider 补全** | 剩余 20+ niche provider | 每个 2–3 天 |
| **TUI** | `syscity tui` 终端界面（参考 OpenClaw `crestodian`） | ~2 周 |
| **Trajectory / 诊断** | 会话轨迹导出、redaction、审计 | ~1 周 |
| **Standing orders** | 永久自主程序配置 | ~2 周 |
| **Lobster-like flow** | 可恢复审批工作流运行时 | ~3 周 |
| **Formal verification** | TLA+ 安全模型（可选，极高端） | ~4 周 |
| **文档 / 生态** | 50+ docs、教程、示例项目 | 持续进行 |
| **测试补全** | 端到端集成测试、契约测试、load test | 持续进行 |

**预期产出**：Syscity 功能覆盖达到 OpenClaw **85–90%**；剩余差距主要在社区生态规模（ClawHub 已有大量第三方插件）。

---

### 总体里程碑

| 阶段 | 时间 | 功能覆盖 | 代码量估算 | OpenClaw 对比 |
|---|---|---|---|---|
| **Phase 0** | 0–2 周 | 20% → 22% | +5K LOC | 修复 bug，让诚实 |
| **Phase 1** | 3–8 周 | 22% → 40% | +40K LOC | 多 provider + 多 channel |
| **Phase 2** | 9–20 周 | 40% → 65% | +100K LOC | UI + Voice + Media + Plugins |
| **Phase 3** | 21–40 周 | 65% → 85% | +100K LOC | 长尾通道 + 生态 + 文档 |
| **总计** | ~40 周 | 20% → 85% | **+245K LOC** | 接近 feature parity |

### 关键决策建议

1. **语言栈取舍**：前端 UI 建议用 React/Vite（生态成熟，招人容易）；坚持全 Rust 用 Leptos 会拖慢 30–50% 进度。
2. **Channel 优先级**：企业用户先 Teams/Slack；个人用户先 Signal/WebChat；中文市场先 QQ/微信（若可行）。
3. **Provider 优先级**：Ollama（本地）和 Google（企业）是 P0；DeepSeek 是中文社区 P1。
4. **安全先行**：Phase 0 的 4 个高危 bug **必须在任何 public release 前修复**。
5. **不要重写**：OpenClaw 的 892K LOC 是 2–3 年的积累；Syscity 不应追求 1:1 复刻，而应追求**核心路径更优**（Rust 性能、内存安全、WASM 插件边界）。
6. **插件化是核心差异**：OpenClaw 的核心竞争力是 126 个 extensions 的生态系统。Syscity 的 Phase 2 必须尽早发布 Plugin SDK，让第三方填补长尾需求。

---

## 十三、从 Prompt 到 Response 的完整路径对比

以下以 **Telegram 用户发送一条消息** 为例，逐层拆解 Syscity 和 OpenClaw 中"用户输入 → 系统返回"的完整数据流。

---

### Syscity 的完整路径（以 Telegram 为例）

```
用户发送消息
  ↓
[1] Telegram Bot API (teloxide Dispatcher)
  ↓
[2] TelegramChannel::handle_message_with_sender()
      · 检查 DM Policy (Open / Allowlist / Pairing)
      · 处理 /new 命令（重置 session）
      · 创建 IncomingMessage { user_id, session_id, content }
      · 通过 message_tx 发送给 Gateway
  ↓
[3] Gateway::process_message_queue()
      · 从 message_queue_rx 接收 QueuedMessage
      · resolve_agent_for_session() → "default" agent
      · 发送 AgentCommand::ProcessMessage 到 agent 的 mpsc channel
  ↓
[4] spawn_agent_inner() 的 loop 接收命令
      · 创建 progress callback（广播 ToolCalling/ToolResult/Completed 到 event_tx）
      · 调用 agent.process_message_with_progress(incoming_msg, progress_cb)
  ↓
[5] Agent::process_message()
      · LLM Cache Classifier: 调用 provider 判断 query 是否可缓存
      · 如果缓存命中 → 直接返回 cached response
      · 否则继续:
      ·   ① 存入 ChatHistory (SQLite) + SessionSearch FTS5 索引
      ·   ② TaskPlanner::needs_planning() → 如需则创建 TaskPlan 并返回
      ·   ③ 获取/创建 Thread + Context
      ·   ④ build_fresh_context():
      ·        · 加载 Personality (SOUL.md / IDENTITY.md / BOOTSTRAP.md / USER.md / TOOLS.md / HEARTBEAT.md / MEMORY.md)
      ·        · MemoryManager::retrieve() 检索相关记忆
      ·        · SkillManager::prefilter_skills() 动态匹配技能
      ·        · PromptBuilder::build_from_context() 构建 system prompt
      ·        · 计算动态 tool iteration limit
      ·   ⑤ get_completion():
      ·        · Context 超预算 → compaction (LLM 辅助摘要 或 启发式丢弃)
      ·        · 获取可用工具列表
      ·        · 组装 CompletionRequest → provider.complete()
      ·        · Provider (OpenAI/Anthropic) 返回响应
      ·        · 如果 response 含 tool_calls → handle_tool_calls()
      ·             · 检查 iteration limit (默认 10)
      ·             · 去重检测 (同一 tool+args 不再执行)
      ·             · 并发执行最多 max_concurrent_tools 个工具
      ·             · ToolRegistry::execute_call() 调用具体工具
      ·             · 工具结果 → 组装 ToolResult message → 递归调用 get_completion()
      ·        · 最终响应 → 添加 assistant message 到 Context
      ·   ⑥ 结果存入 ChatHistory + SessionSearch
      ·   ⑦ 如果可缓存 → ResponseCache::set()
      ·   ⑧ 返回 OutgoingMessage
  ↓
[6] Agent loop 发送 GatewayEvent::AgentResponse 到 event_tx
  ↓
[7] Gateway response handler task (per channel)
      · 接收 AgentResponse，按 channel 过滤（telegram/discord/slack...）
      · 从 session_channels 查找 chat_id
      · 调用 channel.send(OutgoingMessage)
  ↓
[8] TelegramChannel::send()
      · markdown → Telegram HTML 转换
      · 调用 teloxide bot.send_message(chat_id, text)
  ↓
用户收到回复
```

**关键时序特征**：
- 每次用户消息触发 **1 次可选的 cache classifier LLM 调用**（若未命中缓存）
- 每个 tool call round 触发 **1 次 provider 调用**
- 最多 `max_tool_iterations` 轮（默认 10，动态计算上限 30）
- 工具执行是**并发**的（最多 `max_concurrent_tools`，默认 5）
- Context compaction 在超预算时触发（可选 LLM 辅助摘要）

---

### OpenClaw 的完整路径（以 Telegram 为例）

```
用户发送消息
  ↓
[1] Telegram Bot API (channel extension: telegram)
  ↓
[2] Channel extension 处理入站消息
      · 通过 extension API 标准化为 OpenClaw message format
      · 可能包含 media attachments (图片/音频/视频)
  ↓
[3] Auto-reply 系统 (src/auto-reply/)
      · 智能调度：判断是否需要回复、延迟回复、批量处理
      · 队列管理：消息排队、去重、优先级排序
      · 可能触发 follow-up 检测
  ↓
[4] Media understanding (src/media-understanding/)
      · 如果消息含图片 → VisionProvider::describe()
      · 如果消息含音频 → SttProvider::transcribe()
      · 预处理结果作为文本注入 agent context
  ↓
[5] Gateway WebSocket / HTTP server (src/gateway/server/)
      · Session management: 分配/复用 session
      · 多 agent 路由: 根据 workspace/agent_id 路由到对应 agent
      · 可能涉及 node connection (iOS/macOS/Android companion app)
  ↓
[6] Agent runner (src/auto-reply/ + src/agents/)
      · Auth profile 加载 (per-agent auth-profiles.json)
      · Context engine 构建上下文
      · Skill loading: 从 workspace/project/personal/managed/bundled 加载 skills
      · Prompt assembly: SOUL.md / IDENTITY.md / TOOLS.md / USER.md / BOOTSTRAP.md / HEARTBEAT.md / MEMORY.md
      · Memory retrieval: memory-core (SQLite+BM25+vector) 或 memory-lancedb 或 memory-wiki
      · 调用 Provider (通过 provider extension plugin)
  ↓
[7] Provider plugin (extensions/openai, anthropic, google, etc.)
      · 每个 provider 是独立 extension，通过 plugin SDK 注册
      · 支持 tools (function calling)
      · Streaming response (SSE/WS)
  ↓
[8] Tool execution (通过 plugin SDK)
      · 工具由 extensions 或 core 提供
      · Approval gates: 危险操作需用户确认
      · Elevated mode: 高权限操作需额外授权
      · Sandbox: Docker-based 可选隔离
      · 工具结果返回给 agent
  ↓
[9] Response pipeline
      · 结果通过 auto-reply 返回 Gateway
      · Gateway 更新:
      ·   · 通过 channel extension 发送回复
      ·   · 更新 Canvas (A2UI) 如果有活跃的 canvas session
      ·   · 广播 SSE events 到 Web UI
      ·   · 记录 trajectory (诊断/审计轨迹)
      ·   · 更新 memory (memory-core 自动索引)
      ·   · 可能触发 cron job / webhook
  ↓
用户收到回复
```

**关键时序特征**：
- Auto-reply 层有**智能调度**，不是每条消息都立即处理
- Media understanding 在 agent 之前预处理媒体
- Provider 是**插件化**的，通过 extension API 注册
- 工具也是**插件化**的，通过 plugin SDK 提供
- Trajectory 全程记录，支持诊断导出
- Canvas update 和 SSE broadcast 是并行的副作用

---

### 路径差异逐层对比

| 层级 | Syscity | OpenClaw | 差异说明 |
|---|---|---|---|
| **入站消息处理** | Channel handler → message_tx → Gateway queue | Channel extension → auto-reply dispatch → media-understanding → Gateway | OpenClaw 多了 auto-reply 调度和 media 预处理层 |
| **Session 路由** | `resolve_agent_for_session()` 固定 "default" | Gateway session manager 多 agent 路由 | Syscity 实际单 agent；OpenClaw 真多 agent |
| **Context 构建** | `build_fresh_context()` 直接组装 | Context engine + auth profile + skill loading | OpenClaw context 构建更复杂，支持 per-agent auth |
| **System Prompt** | `PromptBuilder::build_from_context()` | 同样的 markdown 文件体系，但通过 context-engine 管理 | 核心概念对齐，实现方式不同 |
| **Memory 检索** | `MemoryManager::retrieve()` 单次检索 | memory-core 多后端 + dreaming + wiki | OpenClaw 记忆系统更丰富 |
| **Provider 调用** | 直接调用 `provider.complete()` | 通过 provider extension plugin 调用 | OpenClaw provider 是插件；Syscity 硬编码 |
| **工具系统** | `ToolRegistry::execute_call()` 硬编码工具 | Plugin SDK 动态注册工具 + approval gates + sandbox | OpenClaw 工具更灵活，审批更完善 |
| **缓存** | LLM-based cache classifier + ResponseCache | 有缓存机制但文档未详述 | Syscity 的缓存设计有特色但多一次 LLM 调用 |
| **Compaction** | `ContextCompressor` LLM 辅助或启发式 | 有更完善的 context engine 管理 | OpenClaw 的上下文管理更成熟 |
| **并发** | 工具并发执行（最多 5 个） | 并发 + 异步调度 + queue | OpenClaw 调度更复杂 |
| **结果返回** | event_tx broadcast → per-channel handler → channel.send() | Gateway → channel extension + canvas + SSE + trajectory | OpenClaw 副作用更多（canvas、SSE、trajectory） |
| **Progress 事件** | ProgressCallback → event_tx → SSE | 更完善的 event system + WebSocket broadcast | OpenClaw 事件系统更成熟 |

---

### 核心架构差异总结

| 维度 | Syscity | OpenClaw |
|---|---|---|
| **消息入口** | 直连（Channel → Gateway → Agent） | 调度层（Channel → Auto-reply → Media → Gateway → Agent） |
| **Agent 数量** | 单 agent（"default"），多 agent stub | 真多 agent，每个有独立 workspace + auth profile |
| **Provider** | 硬编码 2 个 | 插件化 40+ 个 |
| **工具** | 硬编码 19 个 | 插件化，核心 + extensions |
| **媒体处理** | 无 | media-understanding 预处理层 |
| **审批** | `ApprovalQueue` 基础版 | Approval gates + elevated mode + sandbox |
| **诊断** | 无 | Trajectory 全程记录 |
| **副作用** | Channel 回复 + SSE | Channel 回复 + Canvas + SSE + Trajectory + Memory 更新 + Cron |

---

### 一句话路径对比

> **Syscity 的路径是：Channel → Queue → Agent → Provider → Tool → Channel，一条直线。**
>
> **OpenClaw 的路径是：Channel → Auto-reply (调度) → Media (预处理) → Gateway (路由) → Agent (多 workspace) → Provider Plugin → Tool Plugin (审批+sandbox) → Gateway (副作用: Canvas+SSE+Trajectory+Memory) → Channel，一个带调度、预处理、副作用的完整 DAG。**

---

## 十四、入站前层与出站后副作用系统详解

以下详细拆解 OpenClaw 在"用户消息进入 agent"之前和"agent 返回结果"之后分别做了哪些工作，以及 Syscity 为何缺失了这些能力。

---

### 一、入站前处理层（OpenClaw）

#### 1.1 Auto-reply 调度系统（`src/auto-reply/`）

OpenClaw 的 auto-reply 不是简单的"收到消息就回复"，而是一个**智能调度层**，核心职责包括：

**a) 入站去抖（Inbound Debounce）**
- `createInboundDebouncer` 为每个 `channelId`/`threadId` 维护一个缓冲队列
- 最大跟踪 2048 个 key，防止内存无限增长
- 对连续消息进行合并/去抖，避免用户在 5 秒内发 3 条消息就触发 3 次 agent 调用
- 不可去抖的消息（审批、心跳、紧急指令）直接绕过缓冲

**b) 回复调度（Reply Dispatcher）**
- `createReplyDispatcher` 将出站回复**串行化**，避免并发发送导致的消息乱序
- 人性化延迟：`800ms–2500ms` 的随机延迟，模拟人类打字速度
- 三种 dispatch kind：
  - `tool` — 工具执行中间状态（如"正在搜索..."）
  - `block` — 中间结果块（如长回复的分段发送）
  - `final` — 最终回复
- 支持静默最终负载（silent final payload），在某些场景下不发送任何回复

**c) 智能路由（Dispatch from Config）**
- `dispatchReplyFromConfig` 是核心编排函数：
  - **入站去重**：`claimInboundDedupe` 防止同一消息被处理两次
  - **Plugin-owned binding**：判断消息是否属于某个 plugin 的专属会话
  - **Send policy**：`resolveSendPolicy` 应用发送策略（如 suppressDelivery 静默模式）
  - **Route-reply**：如果回复目标与当前 surface 不同（如在 Slack 收到消息但需要通过 Web 回复），自动路由
  - **TTS 应用**：如果用户开启了语音模式，自动将文本转为语音
  - **Approval events**：如果 agent 触发了审批请求，自动发送审批事件

**d) 队列模式（Queue Modes）**
- `runPreparedReply` 支持多种队列模式：
  - `interrupt` — 新消息打断当前 agent 执行
  - `steer` — 用户消息可以"引导"正在执行的 agent 改变方向
  - `followup` — 收集多轮消息后再统一处理
  - `collect` — 批量收集消息，合并后一次性处理
- 通过 `pi-agent-core` 嵌入式运行时解析队列状态

**Syscity 缺失了什么？**
Syscity 的 `process_message_queue` 是一个简单的 FIFO 队列，`process_message` 也是同步阻塞式的。没有 debounce、没有串行化发送、没有人性化延迟、没有队列模式（interrupt/steer/followup/collect）、没有 plugin-owned binding。这意味着：
- 用户连发 3 条消息 → Syscity 会并发触发 3 次 LLM 调用，可能互相干扰
- 回复瞬间到达 → 没有"人类在打字"的拟真感
- 不支持打断 → agent 正在执行长任务时用户无法中途干预

---

#### 1.2 Media-understanding 预处理（`src/media-understanding/`）

当用户发送的消息**包含媒体附件**（图片、音频、视频、文件）时，OpenClaw 不会直接把原始字节丢给 LLM，而是先进行**媒体理解**预处理。

**a) 支持的媒体类型**

| 能力 | 说明 | Provider |
|---|---|---|
| `image` | 图片描述（"图中有什么"） | Claude (vision)、GPT-4V、Gemini |
| `audio` | 语音转文字 | Whisper、sherpa-onnx、Gemini、本地 CLI |
| `video` | 视频内容描述 | Gemini、视频能力模型 |
| `file` | 文档文本提取 | PDF extractors、document parsers |

**b) 处理流程**

```
用户发送带图片的消息
  ↓
[1] Channel extension 提取附件
      · 图片 → 下载为 buffer
      · 音频 → 下载为 buffer
      · 视频 → 下载为 buffer
  ↓
[2] Media-understanding runner (`runCapability`)
      · 根据 capability 类型选择 provider
      · Fallback 链：配置指定模型 → 当前 active model → key providers → 本地 CLI → auto providers
  ↓
[3] 执行理解
      · Image: `describeImagesWithModel()` → 调用 vision-capable model，传入 base64 图片
      · Audio: 调用 STT provider → 返回 transcript
      · Video: 调用 video-capable model → 返回描述
  ↓
[4] Apply to context (`applyMediaUnderstanding`)
      · 将理解结果注入 `ctx.Body`
      · 音频 transcript 额外存入 `ctx.Transcript`
      · 支持 "echo transcript"（朗读转录结果）
  ↓
[5] Agent 看到的不是 "[图片]"，而是 "用户发送了一张图片，图中有一只猫在沙发上..."
```

**c) 智能跳过**
- 如果当前使用的 LLM **原生支持 vision**（如 GPT-4V、Claude 3），则**跳过** media-understanding，直接将图片 base64 传给 LLM，避免浪费一次 API 调用
- 如果模型不支持 vision，才走 media-understanding 预处理路径

**d) 图像优化管道（`src/media/`）**
- `getImageMetadata` — 读取 PNG/GIF/WebP/JPEG 尺寸，无需加载整个文件
- `normalizeExifOrientation` — 校正 JPEG EXIF 方向
- `resizeToJpeg` — 按质量梯度 [85, 75, 65, 55, 45, 35] 压缩，直到满足大小限制
- `convertHeicToJpeg` — HEIC/HEIF 转 JPEG
- `optimizeImageToPng` — PNG 优化，保留 alpha 通道
- 最大输入像素：2500 万像素
- 最大文件：图片 6MB、音频 16MB、视频 16MB、文档 100MB

**Syscity 缺失了什么？**
Syscity 的 `IncomingMessage` 只有 `{ user_id, conversation_id, content }` 三个字段，没有 `attachments` 字段。`channels/mod.rs` 虽然定义了 `Attachment` 类型，但 Telegram/Discord handler 中**从未解析媒体附件**。这意味着：
- 用户发图片 → Syscity 只收到空文本或图片链接，看不到内容
- 用户发语音 → Syscity 无法转录，完全忽略
- 没有图像优化管道，即使未来支持图片也无法处理大文件
- 没有 "模型原生 vision vs 预处理" 的智能判断逻辑

---

### 二、出站后副作用系统（OpenClaw）

Agent 返回结果后，OpenClaw 不会只做"发一条消息给用户"这一件事，而是触发一系列**副作用（side effects）**，形成完整的产品闭环。

#### 2.1 Trajectory 记录（`src/trajectory/`）

Trajectory 是 OpenClaw 的**诊断/审计系统**，默认开启，记录 agent 运行的完整轨迹。

**a) 记录内容**
- `createTrajectoryRuntimeRecorder` 默认开启（`OPENCLAW_TRAJECTORY=0` 才关闭）
- 以 JSONL 格式写入 `~/.config/openclaw/trajectory/`
- 每条事件有序列号（seq numbering）
- 单事件最大 256KB，单文件最大 512MB，超限自动截断
- 记录的信息包括：
  - 运行元数据：OpenClaw 版本、操作系统、运行时、已加载插件、技能列表
  - 会话分支：完整的对话历史
  - 运行时事件：tool calls、provider responses、errors
  - 最终状态：success/failure、token usage、工具元数据
  - 消息发送记录

**b) 导出与 redaction**
- `exportTrajectoryBundle` 将轨迹导出为诊断包
- 自动 redact（脱敏）workspace 路径，防止泄露敏感目录结构
- 输出到 `.openclaw/trajectory-exports/`
- 可用于：bug 报告、安全审计、性能分析

**Syscity 缺失了什么？**
Syscity 完全没有轨迹记录系统。`tracing::info!`/`debug!` 日志只记录到控制台，没有结构化持久化。这意味着：
- 用户报告 bug 时，开发者无法复现 agent 的完整决策路径
- 没有审计能力，无法回溯 agent 在某个时间点做了什么
- 没有诊断包导出，技术支持成本高

---

#### 2.2 Canvas / A2UI（`src/canvas-host/`）

Canvas 是 OpenClaw 的**动态可视化界面系统**，agent 可以在回复用户的同时，渲染一个可交互的网页界面。

**a) 服务端**
- `startCanvasHost` 启动一个 HTTP 服务器（可配置端口）
- 默认根目录：`~/.config/openclaw/canvas/`
- 使用 `chokidar` 监视文件变化，通过 WebSocket 推送 live reload
- A2UI 路径通过 `handleA2uiHttpRequest` 处理

**b) 客户端 live reload**
- `injectCanvasLiveReload` 在 HTML 中注入 WebSocket reload 脚本
- 支持移动端 action bridge：
  - iOS: `webkit.messageHandlers`
  - Android: `window.openclawCanvasA2UIAction`
- agent 可以实时更新 canvas 内容（如生成图表、展示数据看板、渲染表单）

**c) 典型使用场景**
- 用户问"帮我分析这个 CSV" → agent 生成一个交互式图表页面
- 用户问"展示我的 todo 列表" → agent 渲染一个可勾选的 todo UI
- 用户问"给我做个 Pomodoro 计时器" → agent 渲染一个带倒计时的网页

**Syscity 缺失了什么？**
Syscity 有 `canvas/mod.rs` 定义了 `CanvasComponent`/`CanvasUpdate`/`CanvasEvent` 等类型，以及 WebSocket handler，但：
- 没有 `canvas-host/` HTTP 服务器来实际 serve 网页
- 没有 live reload 机制
- 没有移动端 action bridge
- Canvas 系统只停留在类型定义和 WebSocket 消息收发层面，没有完整的"agent 生成页面 → 用户看到页面 → 用户交互 → agent 收到事件"闭环

---

#### 2.3 SSE / WebSocket Broadcast（`src/gateway/server-broadcast.ts`）

OpenClaw 的 Gateway 不仅向 channel 发回复，还向**所有连接的客户端**广播事件。

**a) Gateway Broadcaster**
- `createGatewayBroadcaster` 维护所有 WebSocket 客户端连接
- 支持 scope guards：`agent`、`chat`、`cron`、`health`、`exec.approval`、`plugin.approval` 等
- 每个客户端只接收其 scope 内的事件
- 慢消费者处理：
  - 如果 `dropIfSlow` 为 true → 丢弃事件
  - 否则 → 关闭连接（1008 "slow consumer"）
- 每个消息有序列号（seq tracking）

**b) Session Events**
- `createTranscriptUpdateBroadcastHandler` — 广播 `session.message` 和 `sessions.changed`
- `createLifecycleEventBroadcastHandler` — 广播会话生命周期事件（创建、销毁、暂停）

**c) 典型事件流**
```
Agent 开始处理消息
  → 广播 "agent.status: processing"
Agent 调用工具
  → 广播 "agent.tool_calling: {name, args}"
Agent 收到工具结果
  → 广播 "agent.tool_result: {name, result}"
Agent 完成回复
  → 广播 "agent.completed: {response}"
  → 同时发送给 channel + Canvas + SSE
```

**Syscity 缺失了什么？**
Syscity 的 `gateway/mod.rs` 中有 `event_tx`（broadcast channel），也有 `ProgressCallback` 将事件广播到 SSE，但：
- 没有 scope guard，所有客户端收到所有事件
- 没有慢消费者保护，广播通道可能无限积压
- `web.rs` 的 SSE handler 是**占位符**，注释写 "in production you'd want to integrate with the agent's event system"
- 没有 `session.message` / `sessions.changed` 等标准化事件类型
- Web UI 不存在，所以 SSE broadcast 没有消费者

---

#### 2.4 Memory 自动更新

OpenClaw 的 memory 更新不是显式调用，而是**内嵌在整个响应管道中的自动副作用**。

**a) 自动索引**
- 每次用户消息和 agent 回复都会**自动**存入 memory-core
- memory-core 维护 SQLite + BM25 + vector 混合索引
- 不需要 agent 显式调用 `memory_search` 或 `memory_get`

**b) Dreaming / 记忆巩固**
- `extensions/memory-core/` 包含 dreaming 系统
- 后台任务定期运行（Light/Deep/REM 三阶段）
- Light dreaming：短时记忆合并
- Deep dreaming：长时记忆整理，生成 `DREAMS.md`
- REM dreaming：创造性联想，生成洞察
- Dream Diary：人类可读的 dreaming 摘要

**c) QMD sidecar**
- 可选的本地优先向量搜索
- 支持 reranking 和 query expansion
- 不依赖云端 embedding API

**Syscity 缺失了什么？**
Syscity 有 `memory_manager` 和 `chat_history`，每次消息会存入 SQLite 和 FTS5，这基本对齐 OpenClaw 的自动索引。但：
- 没有 dreaming 系统（light/deep/REM）
- 没有 `DREAMS.md` 生成
- 没有 QMD sidecar
- `MemorySearchTool` 宣称 hybrid 但实际降级为 LIKE，这是关键缺陷

---

#### 2.5 Cron 触发（`src/cron/`）

OpenClaw 的 cron 不是独立的定时任务系统，而是**与 agent 深度集成**的。

**a) Cron 服务**
- `CronService`：start/stop/status/list/add/update/remove/run
- `start` 时处理中断的作业（crash recovery）
- `run` 时：
  - `prepareManualRun` — 加锁、持久化 running marker
  - `finishPreparedManualRun` — 执行核心逻辑、应用结果、emit 事件
  - Task run ledger 集成（`createRunningTaskRun` / `completeTaskRunByRunId`）

**b) Delivery**
- `sendCronAnnouncePayloadStrict` — 解析 delivery target，构建 session context
- 通过 `deliverOutboundPayloads` 交付结果
- `sendFailureNotificationAnnounce` — 失败时的最佳 effort 通知（30s timeout）

**c) 与 agent 的集成**
- Cron job 的目标可以是一个 agent（`agentId`）
- Cron job 的执行结果通过 Gateway 的广播系统发送
- Agent 可以在回复中触发 cron job（如"每天提醒我喝水"）

**Syscity 缺失了什么？**
Syscity 有 `cron/cron.rs` 和 `cron/mod.rs`，以及 `tools/cron_tool.rs`，但：
- Cron 调度器是通过全局 `OnceCell` 接入的（`tokio::sync::OnceCell<CronScheduler>`），初始化顺序 load-bearing
- Cron job 只能 delivery 到 gateway 的 SSE broadcast，不能指定 target agent
- 没有 task run ledger（持久化执行记录）
- 没有 failure notification
- Agent 回复后不会自动更新 cron 状态

---

### 三、Syscity 与 OpenClaw 的副作用系统对比总表

| 副作用 | OpenClaw 实现 | Syscity 状态 | 差距评估 |
|---|---|---|---|
| **入站 Debounce** | 2048 key 缓冲队列 + setTimeout 去抖 | 无 | 缺失 |
| **入站去重** | `claimInboundDedupe` 防止重复处理 | 无 | 缺失 |
| **回复串行化** | Reply dispatcher 人性化延迟 800–2500ms | 直接发送 | 缺失 |
| **队列模式** | interrupt/steer/followup/collect | 无 | 缺失 |
| **媒体理解** | 图片/音频/视频预处理后注入 context | 无 attachment 解析 | 缺失 |
| **图像优化** | resize/compress/HEIC 转换/alpha 保留 | 无 | 缺失 |
| **Trajectory 记录** | JSONL 事件 + 导出 + redaction | 无 | 缺失 |
| **Canvas / A2UI** | HTTP serve + WebSocket live reload + 移动端桥接 | 类型定义有，serve 缺失 | 半实现 |
| **SSE Broadcast** | Scope guard + slow consumer 保护 + seq tracking | 占位符 | 缺失 |
| **Memory dreaming** | Light/Deep/REM + DREAMS.md | 无 | 缺失 |
| **QMD sidecar** | 本地优先向量搜索 | 无 | 缺失 |
| **Cron delivery** | Target agent + task ledger + failure notify | SSE broadcast only | 弱 |
| **Plugin approval** | Elevated mode + sandbox + gates | 基础 ApprovalQueue | 弱 |

---

### 四、为什么这些层很重要？

**入站前层解决的是"不要乱"的问题**：
- 用户连发消息 → debounce 防止 agent 被冲垮
- 用户发图片 → media-understanding 让 agent "看到"内容
- 用户在群里@bot → auto-reply 判断是否需要回复、回复给谁
- 用户发语音 → STT 让 agent "听到"内容

**出站后副作用解决的是"不只是聊天"的问题**：
- Trajectory → 让产品可诊断、可审计
- Canvas → 让 agent 输出超越纯文本（图表、UI、交互）
- SSE → 让 Web UI 实时感知 agent 状态
- Memory dreaming → 让 agent 越用越"懂"用户
- Cron → 让 agent 的回复可以触发后续自动化

**Syscity 目前是一条"直线"**：消息进来 → agent 处理 → 发回复。这条线在工作，但缺少所有让产品从"demo"变成"daily driver"的周边系统。

---

## 十五、Syscity 骨架对齐计划：从直线到分层 DAG

> 本计划聚焦于**架构骨架**层面的改造，让 Syscity 的消息流转路径从"一条直线"变成和 OpenClaw 一致的"分层 DAG"。不追求功能追平（那是 Phase 0–3 的目标），而是追求**架构同构**——每个层都有对应的模块、接口和数据流。

---

### 一、目标骨架总览

```
┌─────────────────────────────────────────────────────────────────────┐
│                         INBOUND PIPELINE                            │
├─────────────────────────────────────────────────────────────────────┤
│  [Channel Extension] → [Inbound Debounce] → [Media Understanding]   │
│         ↓                                                        │
│  [Auto-reply Dispatch] ──→ [Queue Mode Resolve] ──→ [Agent Router] │
└─────────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────────┐
│                         AGENT LAYER                                 │
├─────────────────────────────────────────────────────────────────────┤
│  [Auth Profile] → [Context Engine] → [Prompt Assembly]              │
│         ↓                                                           │
│  [Memory Retrieval] → [Skill Loading] → [Provider Call]             │
│         ↓                                                           │
│  [Tool Execution] ──→ [Approval Gate] ──→ [Sandbox]                 │
└─────────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────────┐
│                        OUTBOUND PIPELINE                            │
├─────────────────────────────────────────────────────────────────────┤
│  [Agent Response] ──→ [Trajectory Recorder]                         │
│         ↓                                                           │
│  [Parallel Side Effects] ──┬──→ [Canvas Update]                     │
│                            ├──→ [SSE Broadcast]                     │
│                            ├──→ [Memory Index]                      │
│                            └──→ [Cron Trigger]                      │
│         ↓                                                           │
│  [Reply Dispatcher] ──→ [Channel Extension]                         │
└─────────────────────────────────────────────────────────────────────┘
```

---

### 二、当前骨架 vs 目标骨架对比

| 层级 | 当前 Syscity（直线） | 目标 Syscity（分层 DAG） | OpenClaw 参考 |
|---|---|---|---|
| **Channel** | `channels/{name}.rs` 直接发 `message_tx` | `channels/` 作为 Extension API 实现 | `extensions/{channel}/` |
| **Inbound** | 无 | `inbound/` debounce + media + dispatch | `src/auto-reply/` + `src/media-understanding/` |
| **Queue** | 无 | `queue/` interrupt/steer/followup/collect | `src/auto-reply/reply/get-reply-run.ts` |
| **Router** | `resolve_agent_for_session()` 固定 "default" | `router/` workspace-based 多 agent 路由 | `src/gateway/server/` session mgmt |
| **Agent** | `agent/mod.rs` 单 agent | `agents/` 多 agent + auth profile + context engine | `src/agents/` |
| **Provider** | 硬编码 `providers/{openai,anthropic}.rs` | `providers/` 硬编码 + `provider-sdk/` 插件接口 | `extensions/{provider}/` |
| **Tool** | 硬编码 `tools/{shell,file,...}.rs` | `tools/` 硬编码 + `tool-sdk/` 插件接口 | 核心 + extensions |
| **Outbound** | `event_tx` broadcast → per-channel handler | `outbound/` trajectory + canvas + sse + reply dispatcher | `src/trajectory/` + `src/canvas-host/` + `src/gateway/server-broadcast.ts` |
| **Side Effects** | 无 | `side-effects/` 并行副作用系统 | 内嵌在 gateway 中 |

---

### 三、改造计划（按模块）

#### Phase A：Inbound Pipeline 骨架（2–3 周）

**目标**：建立入站消息的分层处理管道。

##### A.1 新建 `src/inbound/` 模块

```
src/inbound/
├── mod.rs              # 模块根，定义 InboundPipeline trait
├── debounce.rs         # InboundDebouncer：keyed buffer + timeout 去抖
├── media.rs            # MediaUnderstandingPipeline：attachment 预处理
├── dispatch.rs         # AutoReplyDispatch：调度入口
├── queue.rs            # QueueModeResolver：interrupt/steer/followup/collect
└── router.rs           # AgentRouter：workspace-based 多 agent 路由
```

**A.1.1 `inbound/debounce.rs`**
- 定义 `InboundDebouncer` struct，支持 keyed buffer（key = `channel_id` 或 `thread_id`）
- 配置项：`max_tracked_keys`（默认 2048）、`debounce_ms`（默认 500）
- 方法：`enqueue(key, item)`、`flush_key(key)`、`dispose()`
- 不可去抖项（审批、心跳）直接 bypass
- 交付标准：连续 3 条消息在 500ms 内到达 → 合并为 1 条处理

**A.1.2 `inbound/media.rs`**
- 定义 `MediaUnderstandingPipeline` struct
- 支持 capability：`image`、`audio`、`video`、`file`
- Fallback 链：配置指定 → active model（若原生支持则跳过）→ key providers → 本地 CLI
- 定义 `MediaAttachment` struct（当前 `channels/mod.rs` 已有，需扩展）
- 交付标准：用户发图片 → agent 看到的 context 包含图片描述文本

**A.1.3 `inbound/dispatch.rs`**
- 定义 `AutoReplyDispatch` struct
- 职责：
  - 调用 debounce → media → queue → router
  - 构建 `message_sending` hooks
  - 应用 send policy（suppressDelivery）
  - 处理 plugin-owned binding
- 交付标准：每条入站消息都经过完整的 dispatch pipeline

**A.1.4 `inbound/queue.rs`**
- 定义 `QueueMode` enum：`Interrupt`、`Steer`、`FollowUp`、`Collect`
- `QueueModeResolver` 根据消息内容/上下文决定模式
- `Interrupt`：新消息打断当前 agent 执行
- `Steer`：用户消息引导正在执行的 agent 改变方向
- `FollowUp`：收集多轮消息后再统一处理
- `Collect`：批量收集消息，合并后一次性处理
- 交付标准：支持 interrupt 模式（最常用），其他模式 stub

**A.1.5 `inbound/router.rs`**
- 定义 `AgentRouter` struct
- 支持 workspace-based 路由：
  - `route(channel_id, user_id, content) -> agent_id`
  - 从 `session_routing` 查找已绑定 agent
  - 未绑定时创建默认 agent 并绑定
  - 支持 `@agent_name` 显式路由
- 交付标准：支持多 agent，不再固定 "default"

##### A.2 改造 `src/channels/mod.rs`
- `IncomingMessage` 增加 `attachments: Vec<Attachment>` 字段
- `Channel::start()` 不再直接发 `message_tx`，而是调用 `InboundPipeline::process()`
- 每个 channel 的 handler 负责解析附件（图片 URL、音频文件等）

##### A.3 改造 `src/gateway/mod.rs`
- 将 `process_message_queue` 替换为 `InboundPipeline` 的调用
- Gateway 初始化时构建完整的 inbound pipeline
- `spawn_agent_inner` 不再接收 `ProcessMessage` 命令，而是由 `AgentRouter` 调用

---

#### Phase B：Agent 层骨架改造（3–4 周）

**目标**：从单 agent 升级为多 workspace agent 体系。

##### B.1 新建 `src/agents/` 模块（复用现有 `src/agent/`）

```
src/agents/
├── mod.rs              # 模块根，定义 Agent trait + AgentRegistry
├── runtime.rs          # AgentRuntime：单个 agent 的运行时
├── router.rs           # AgentRouter：多 agent 路由（从 inbound/ 迁移）
├── workspace.rs        # Workspace：agent 的工作空间定义
├── auth_profile.rs     # AuthProfile：每个 agent 的认证配置
├── context_engine.rs   # ContextEngine：上下文构建（替代 build_fresh_context）
├── prompt_engine.rs    # PromptEngine：prompt 组装（替代 PromptBuilder）
├── tool_harness.rs     # ToolHarness：工具执行 + 审批 + sandbox
└── session_manager.rs  # SessionManager：会话生命周期管理
```

**B.1.1 `agents/workspace.rs`**
- 定义 `Workspace` struct：
  - `id`、`name`、`path`（目录）
  - `agents: Vec<AgentConfig>`
  - `skills: Vec<Skill>`
  - `memory_backend: MemoryBackend`
- 每个 workspace 有独立的 `SOUL.md`、`IDENTITY.md`、`TOOLS.md`、`USER.md`、`BOOTSTRAP.md`、`HEARTBEAT.md`、`MEMORY.md`
- 交付标准：支持多 workspace，每个 workspace 有独立的 prompt 文件

**B.1.2 `agents/auth_profile.rs`**
- 定义 `AuthProfile` struct：
  - `provider_credentials`：每个 provider 的 API key/endpoint
  - `channel_credentials`：每个 channel 的 token/secret
  - `secret_scope`：secret 的可见范围
- 支持 per-agent auth profile（不是全局统一）
- 交付标准：两个 agent 可以用不同的 OpenAI API key

**B.1.3 `agents/context_engine.rs`**
- 定义 `ContextEngine` struct，替代 `Agent::build_fresh_context()`
- 职责：
  - 加载 personality files（按 workspace）
  - 检索 memory（通过 MemoryManager）
  - 加载 skills（通过 SkillManager）
  - 组装 system prompt（通过 PromptEngine）
  - 注入 media understanding 结果
- 交付标准：上下文构建逻辑从 agent 中抽离，可独立测试

**B.1.4 `agents/tool_harness.rs`**
- 定义 `ToolHarness` struct，封装工具执行全流程：
  - 工具查找（ToolRegistry）
  - 审批检查（ApprovalGate）
  - 沙箱执行（Sandbox）
  - 结果格式化
- 支持 approval gates：
  - `Low`：自动通过
  - `Medium`：静默记录
  - `High`：需要用户确认
  - `Critical`：需要 elevated mode
- 支持 sandbox：Docker-based（可选）
- 交付标准：危险工具（shell、code_exec）默认需要审批

**B.1.5 `agents/session_manager.rs`**
- 定义 `SessionManager` struct
- 职责：
  - session 创建/销毁/暂停/恢复
  - session 与 agent 的绑定/解绑
  - session 生命周期事件广播
- 交付标准：支持 session 的完整生命周期管理

##### B.2 改造现有 `src/agent/mod.rs`
- 将 `Agent` 改造为 `AgentRuntime`：
  - 移除 `build_fresh_context`，改为调用 `ContextEngine`
  - 移除 `handle_tool_calls`，改为调用 `ToolHarness`
  - 保留 `process_message` 作为顶层入口
- `AgentConfig` 增加 `workspace_id` 和 `auth_profile_id` 字段

##### B.3 改造 `src/gateway/mod.rs`
- Gateway 初始化时构建 `AgentRegistry`
- 每个 workspace 初始化一个默认 agent
- `spawn_agent_inner` 改为 `AgentRuntime::spawn()`
- 支持运行时创建/销毁 agent

---

#### Phase C：Provider / Tool 插件化骨架（2–3 周）

**目标**：建立 Provider 和 Tool 的插件化接口，即使初始只有硬编码实现。

##### C.1 新建 `src/provider-sdk/` 模块

```
src/provider-sdk/
├── mod.rs              # 模块根，定义 ProviderExtension trait
├── registry.rs         # ProviderExtensionRegistry：插件注册表
└── manifest.rs         # ProviderManifest：插件元数据
```

**C.1.1 `provider-sdk/mod.rs`**
- 定义 `ProviderExtension` trait：
  ```rust
  pub trait ProviderExtension: Send + Sync {
      fn name(&self) -> &str;
      fn models(&self) -> Vec<ModelInfo>;
      fn supports_tools(&self) -> bool;
      async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
      async fn stream(&self, request: CompletionRequest) -> Result<CompletionStream>;
  }
  ```
- 定义 `ProviderExtensionRegistry`：动态注册/注销 provider
- 交付标准：OpenAI/Anthropic 实现 `ProviderExtension`，model_router 通过 registry 调用

##### C.2 新建 `src/tool-sdk/` 模块

```
src/tool-sdk/
├── mod.rs              # 模块根，定义 ToolExtension trait
├── registry.rs         # ToolExtensionRegistry：插件注册表
├── manifest.rs         # ToolManifest：插件元数据
└── sandbox.rs          # SandboxConfig：沙箱配置
```

**C.2.1 `tool-sdk/mod.rs`**
- 定义 `ToolExtension` trait：
  ```rust
  pub trait ToolExtension: Send + Sync {
      fn name(&self) -> &str;
      fn description(&self) -> &str;
      fn parameters_schema(&self) -> serde_json::Value;
      async fn execute(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult>;
  }
  ```
- 现有 19 个工具实现 `ToolExtension`
- 交付标准：ToolRegistry 从 hard-coded map 改为 dynamic registry

##### C.3 改造 `src/model_router/mod.rs`
- `create_provider` 不再硬编码，改为从 `ProviderExtensionRegistry` 查找
- Azure/Ollama/Custom stub 改为真实的 `ProviderExtension` 实现（即使初始只有空壳）

---

#### Phase D：Outbound Pipeline 骨架（2–3 周）

**目标**：建立出站消息的副作用管道。

##### D.1 新建 `src/outbound/` 模块

```
src/outbound/
├── mod.rs              # 模块根，定义 OutboundPipeline trait
├── trajectory.rs       # TrajectoryRecorder：诊断轨迹记录
├── canvas.rs           # CanvasHost：A2UI 服务
├── sse.rs              # SseBroadcaster：SSE/WS 广播
├── reply_dispatcher.rs # ReplyDispatcher：回复串行化 + 人性化延迟
└── side_effect.rs      # SideEffectRunner：并行副作用执行
```

**D.1.1 `outbound/trajectory.rs`**
- 定义 `TrajectoryRecorder` struct
- 以 JSONL 格式写入 `~/.syscity/trajectory/`
- 记录：
  - 运行元数据（version、OS、plugins、skills）
  - 会话分支（完整对话历史）
  - 运行时事件（tool calls、provider responses、errors）
  - 最终状态（success/failure、token usage）
- 单事件最大 256KB，单文件最大 512MB
- 交付标准：每次 agent 运行都生成轨迹文件

**D.1.2 `outbound/canvas.rs`**
- 定义 `CanvasHost` struct
- 启动 HTTP 服务器（可配置端口）
- 默认根目录：`~/.syscity/canvas/`
- WebSocket live reload
- 支持 A2UI 路径
- 交付标准：agent 可以通过 Canvas API 更新一个网页，用户能看到实时变化

**D.1.3 `outbound/sse.rs`**
- 定义 `SseBroadcaster` struct
- 维护所有 SSE/WebSocket 客户端连接
- 支持 scope guards：`agent`、`chat`、`cron`、`health`、`exec.approval`
- 慢消费者处理：`dropIfSlow` 或关闭连接（1008）
- 每个消息有序列号
- 交付标准：Web UI 能实时接收 agent 事件

**D.1.4 `outbound/reply_dispatcher.rs`**
- 定义 `ReplyDispatcher` struct
- 串行化发送：确保同一 session 的消息按序到达
- 人性化延迟：`800ms–2500ms` 随机延迟
- 支持 dispatch kind：`tool`、`block`、`final`
- 交付标准：回复有"人类打字"的拟真感

**D.1.5 `outbound/side_effect.rs`**
- 定义 `SideEffectRunner` struct
- 并行执行所有副作用：
  - trajectory recorder（写入磁盘，不阻塞）
  - canvas update（WebSocket push，不阻塞）
  - sse broadcast（广播，不阻塞）
  - memory index（SQLite 写入，不阻塞）
  - cron trigger（若有需要）
- 使用 `tokio::spawn` 并行化
- 交付标准：agent 回复后，所有副作用在 100ms 内触发

##### D.2 改造 `src/gateway/mod.rs`
- Agent loop 返回结果后，调用 `OutboundPipeline::process()`
- 不再直接发送 `GatewayEvent::AgentResponse`，而是让 OutboundPipeline 决定如何分发
- 保留 per-channel response handler，但改为由 ReplyDispatcher 驱动

---

#### Phase E：Channel / Extension 骨架（1–2 周）

**目标**：将 Channel 从硬编码模块改造为 Extension API 实现。

##### E.1 新建 `src/extension-sdk/` 模块

```
src/extension-sdk/
├── mod.rs              # 模块根，定义 Extension trait
├── channel.rs          # ChannelExtension trait
├── provider.rs         # ProviderExtension trait（从 provider-sdk/ 迁移）
├── tool.rs             # ToolExtension trait（从 tool-sdk/ 迁移）
├── manifest.rs         # ExtensionManifest：扩展元数据
└── loader.rs           # ExtensionLoader：动态加载
```

**E.1.1 `extension-sdk/channel.rs`**
- 定义 `ChannelExtension` trait：
  ```rust
  pub trait ChannelExtension: Send + Sync {
      fn name(&self) -> &str;
      fn capabilities(&self) -> ChannelCapabilities;
      async fn start(&self, inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()>;
      async fn stop(&self) -> Result<()>;
      async fn send(&self, message: OutgoingMessage) -> Result<MessageId>;
      async fn edit(&self, message_id: MessageId, new_content: String) -> Result<()>;
      async fn delete(&self, message_id: MessageId) -> Result<()>;
  }
  ```
- 现有 `channels/telegram.rs`、`channels/discord.rs` 等实现 `ChannelExtension`
- 交付标准：ChannelRegistry 从 hard-coded 改为 dynamic registry

##### E.2 改造 `src/channels/mod.rs`
- `ChannelRegistry` 支持动态注册/注销 channel extension
- `Gateway` 初始化时扫描 `extensions/` 目录加载 channel extensions
- 每个 channel 独立运行，通过 `inbound_tx` 向 InboundPipeline 发送消息

---

### 四、文件重构清单

| 现有文件 | 改造后 | 说明 |
|---|---|---|
| `src/agent/mod.rs` | `src/agents/runtime.rs` | Agent 运行时 |
| `src/agent/mod.rs::build_fresh_context` | `src/agents/context_engine.rs` | 上下文构建 |
| `src/agent/mod.rs::handle_tool_calls` | `src/agents/tool_harness.rs` | 工具执行 |
| `src/gateway/mod.rs`（7268 行） | `src/gateway/mod.rs` + `src/inbound/` + `src/outbound/` | 拆分 inbound/outbound |
| `src/gateway/mod.rs::process_message_queue` | `src/inbound/dispatch.rs` | 入站调度 |
| `src/gateway/mod.rs::spawn_agent_inner` | `src/agents/runtime.rs::spawn()` | Agent 启动 |
| `src/channels/mod.rs` | `src/channels/mod.rs` + `src/extension-sdk/channel.rs` | Channel Extension API |
| `src/channels/telegram.rs` | `src/channels/telegram.rs`（实现 ChannelExtension） | 实现新 trait |
| `src/providers/mod.rs` | `src/providers/mod.rs` + `src/provider-sdk/` | Provider Extension API |
| `src/tools/mod.rs` | `src/tools/mod.rs` + `src/tool-sdk/` | Tool Extension API |
| `src/memory/mod.rs` | `src/memory/mod.rs` + `src/memory/dreaming.rs` | Dreaming 系统 |
| 新建 | `src/trajectory/` | 诊断轨迹 |
| 新建 | `src/canvas-host/` | A2UI 服务 |

---

### 五、依赖关系图

```
┌────────────────────────────────────────────────────────────┐
│  src/extension-sdk/                                         │
│  ├── ChannelExtension (src/channels/* 实现)                  │
│  ├── ProviderExtension (src/providers/* 实现)                │
│  └── ToolExtension (src/tools/* 实现)                        │
└────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────┐
│  src/inbound/                                               │
│  ├── debounce.rs ←── channels 原始消息                      │
│  ├── media.rs ←── 附件解析                                  │
│  ├── dispatch.rs ←── 调度入口                               │
│  ├── queue.rs ←── 队列模式                                  │
│  └── router.rs ←── agent 路由                               │
└────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────┐
│  src/agents/                                                │
│  ├── runtime.rs ←── 核心 agent 循环                         │
│  ├── context_engine.rs ←── 上下文构建                       │
│  ├── tool_harness.rs ←── 工具执行                           │
│  ├── workspace.rs ←── 工作空间                              │
│  └── auth_profile.rs ←── 认证配置                           │
└────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────┐
│  src/outbound/                                              │
│  ├── trajectory.rs ←── 诊断记录                             │
│  ├── canvas.rs ←── A2UI                                     │
│  ├── sse.rs ←── 实时广播                                    │
│  ├── reply_dispatcher.rs ←── 回复串行化                     │
│  └── side_effect.rs ←── 并行副作用                          │
└────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────┐
│  src/extension-sdk/                                         │
│  └── ChannelExtension::send() ←── 最终回复                  │
└────────────────────────────────────────────────────────────┘
```

---

### 六、改造顺序建议

| 顺序 | 阶段 | 周数 | 依赖 |
|---|---|---|---|
| 1 | **Phase A：Inbound Pipeline** | 2–3 周 | 无（独立新建） |
| 2 | **Phase D：Outbound Pipeline** | 2–3 周 | 无（独立新建） |
| 3 | **Phase B：Agent 层改造** | 3–4 周 | 依赖 Phase A（router） |
| 4 | **Phase C：Provider/Tool SDK** | 2–3 周 | 依赖 Phase B（agent 调用 provider/tool） |
| 5 | **Phase E：Channel Extension** | 1–2 周 | 依赖 Phase A（inbound pipeline） |
| **总计** | | **10–15 周** | |

**为什么是这个顺序？**
1. **先建 Inbound/Outbound**：这两个是纯新增模块，不影响现有代码，风险最低
2. **再改造 Agent**：Agent 层改造需要将 `build_fresh_context` 和 `handle_tool_calls` 抽离，依赖 Inbound Pipeline 的 router
3. **然后 Provider/Tool SDK**：SDK 是接口层，Agent 改造完成后才能确定接口需求
4. **最后 Channel Extension**：Channel Extension 需要 Inbound Pipeline 作为接收端，放在最后

---

### 七、最小可行骨架（MVP Skeleton）

如果 10–15 周太长，可以先实现**最小可行骨架**（6–8 周）：

| 优先级 | 模块 | 说明 |
|---|---|---|
| P0 | `inbound/dispatch.rs` | 调度入口，串联 debounce + router |
| P0 | `inbound/router.rs` | 多 agent 路由（至少支持 "default" + 显式指定） |
| P0 | `agents/context_engine.rs` | 抽离上下文构建 |
| P0 | `outbound/trajectory.rs` | 诊断轨迹（对调试至关重要） |
| P0 | `outbound/sse.rs` | SSE 广播（Web UI 的前提） |
| P1 | `inbound/debounce.rs` | 入站去抖 |
| P1 | `agents/tool_harness.rs` | 工具执行 + 审批 |
| P1 | `outbound/reply_dispatcher.rs` | 回复串行化 |
| P1 | `provider-sdk/` + `tool-sdk/` | 插件化接口（空壳 + 硬编码适配器） |
| P2 | `inbound/media.rs` | 媒体理解 |
| P2 | `outbound/canvas.rs` | A2UI |
| P2 | `extension-sdk/` | 完整 Extension API |

**MVP 交付标准**：
- 消息流转路径：Channel → Debounce → Dispatch → Router → Agent → Trajectory + SSE → Reply Dispatcher → Channel
- 支持多 agent（至少 2 个）
- 每次 agent 运行生成轨迹文件
- Web UI 能实时看到 agent 事件
- 工具执行需要审批（高危工具）
- 回复有人性化延迟

---

### 八、与 Phase 0–3 路线图的关系

| 维度 | Phase 0–3（功能追平） | 本计划（骨架对齐） |
|---|---|---|
| **目标** | 功能数量追平 OpenClaw | 架构同构 OpenClaw |
| **时间** | 40 周 | 10–15 周（MVP 6–8 周） |
| **产出** | 更多 provider、channel、UI | 更清晰的分层、可扩展的接口 |
| **风险** | 高（大量新功能） | 中（主要是重构） |
| **建议** | 骨架对齐完成后，再功能追平 | 先骨架，再功能 |

> **建议执行顺序**：先完成本计划的 MVP Skeleton（6–8 周），再进入 Phase 0–3 的功能追平。骨架不稳，功能越多债务越重。
