# 引擎接入 Syscity Cloud — 工作清单

> 云端能力已全部实现（syscity-cloud/server，见 `cloud-design.md` §2.8 与 §3.6/§3.7）。
> 本清单列引擎侧要做的接入工作，按依赖顺序分组；每一项标注所需云端端点。

**依赖顺序**：P0（登录态）是一切的前提 → P1（分发，已有基础）→ P2（云端服务使用）→ 延后（同步）。

---

## P0 — 账号/登录（用云端的前提）

- [ ] **1. Cargo feature `cloud` 隔离**（§2.7）
      - 所有云集成代码收进 `cloud` feature（默认关闭、运行时双闸 `cloud.enabled` + 登录态）。
      - 目的：公开仓库不含任何云耦合，运营侧代码可审计。
- [ ] **2. gateway 登录态 + 账号绑定**（P0-2）
      - 引擎 web/UI 加「登录」入口 → 云端 OAuth `GET /auth/{provider}?redirect=<engine>`（github/google/wechat）。
      - 回调拿 session token → 存本地（config/secrets）→ `GET /auth/me` 验证 → 显示账号/登录态。
- [ ] **3. Session token 管理**
      - 存储 + 所有云端调用带 `Authorization: Bearer <token>` + 失效/登出处理（`POST /auth/logout` 吊销）。

## P1 — 市场分发

- [x] **4. 分发调度器** — **已有**（`src/mcp/connectors/catalog.rs`：拉 catalog.json + 归档 + sha256 校验 + 安装）。
- [ ] 补：**登录后带 token 拉取** `GET /catalog.json`，可见 member 条目（登录 vs 匿名）。

## P2 — 云端服务使用（价值主体）

- [ ] **5. 云 provider（LLM/嵌入）**（§2.8 云模式）
      - model_router 加「云 provider」→ `POST /v1/chat/completions` + `POST /v1/embeddings`（基于现有 OpenAI-compatible provider + custom base_url + token 注入）。
      - 效果：登录即用云端 LLM，零配置，积分云端扣。
- [ ] **6. 搜索 Cloud provider**
      - → `POST /v1/search`（Tavily/Bing/Serp 归一化，固定积分扣费）。
- [ ] **7. 知识库连接器**（§3.7）
      - agent 工具调 `POST /api/v1/kb/:id/query` 注入 RAG 上下文；上传（multipart）可后补。
- [ ] **8. 云端采购连接器（kind=cloud）**（§3.6）
      - 引擎侧 cloud connector 走云端 MCP 代理：`POST /api/v1/mcp/tools` / `POST /api/v1/mcp/call`（带 token，按 credits 扣费）。
- [ ] **9. 设备绑定**
      - 引擎启动时 `POST /api/v1/devices` 拿 device_token，作为设备身份（为未来同步打底）。
- [ ] **10. 用量/订阅展示**
      - UI 显示积分余额/用量/套餐（`GET /api/v1/subscription`、`GET /api/v1/usage`），低积分提示升级。

## 延后

- [ ] **11. 云端同步 client**（P0-3）— session/artifact/connectors 增量同步；memory 明确不同步。

## 不需要做的

- **本地用量上报**：计量在云端代理层实时扣（`deduct_fixed`/`deduct_usage`），引擎无需上报。
- **BYOA 连接器**：本地、免费、不走云端。

---

## 云端端点速查（引擎侧将消费）

| 用途 | 端点 |
|---|---|
| 登录/验证 | `GET /auth/{provider}?redirect=` · `GET /auth/me` · `POST /auth/logout` |
| 市场 | `GET /catalog.json` · `GET /archives/:id/:version/:file` |
| LLM/嵌入 | `POST /v1/chat/completions` · `POST /v1/embeddings` |
| 搜索 | `POST /v1/search` |
| 知识库 | `POST /api/v1/kb/:id/query` · `POST /api/v1/kb/:id/documents` |
| 云端连接器 | `POST /api/v1/mcp/tools` · `POST /api/v1/mcp/call` |
| 设备 | `POST /api/v1/devices` |
| 订阅/用量 | `GET /api/v1/subscription` · `GET /api/v1/usage` |

> 云端设计详见 `syscity-cloud/cloud-design.md`（§2.7 代码隔离、§2.8 双模式、§3.6 市场/连接器/积分、§3.7 知识库/配额）。
