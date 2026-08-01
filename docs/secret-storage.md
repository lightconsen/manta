# Secret Storage 设计(敏感信息存储)

> 状态:设计稿(Design Proposal)
> 目标读者:syscity 贡献者、维护者
> 关联模块:[config](modules/config.md)、[security](modules/security.md)、[mcp](modules/mcp.md)、[channels](modules/channels.md)、[plugins](modules/plugins.md)

本文档盘点 syscity 当前的所有敏感信息及其存储位置,给出基于行业最佳实践
(OWASP Secrets Management、OS keychain 优先、OAuth token 生命周期管理)的
统一存储设计:**值进 OS keyring、无 keyring 回退 0600 文件、config 只存引用**。

---

## 1. 背景与目标

当前 syscity 的敏感信息散落在 5 套机制里,成熟度差异很大:

| 机制 | 内容 | 权限/强度 |
|------|------|-----------|
| `config.toml` 明文 | LLM key、channel token、security token、OAuth client_secret | 默认 0644,值即明文 |
| 环境变量 / `$VAR` 插值 | `SYSCITY_PROVIDER_{NAME}_KEY` 等 | 不落盘,最安全 |
| `~/.syscity/mcp_tokens/{id}.json` | MCP OAuth token(access + refresh) | **0644,明文,无 sanitize** |
| `~/.syscity/mcp_env/{id}.toml` | MCP env token(用户输入) | **dir 0700 / file 0600,原子写,sanitize**(唯一做对的) |
| `secrets.rs`(内存) | SecretRef 解析值(env/file/exec) | zeroize + Debug `[REDACTED]` + TTL |

目标:

1. 把所有**用户输入 / 第三方签发**的持久 secret 统一收进 OS keyring。
2. 无 keyring 的环境(无头 Linux / 容器 / CI)**回退**到 0600 加密文件。
3. `config.toml` **永不存值,只存引用**。
4. 不聚合 —— 每实体(provider / server / channel / plugin)独立条目。
5. 不引入超出需要的复杂度;逐步迁移,不破坏现有用户配置。

---

## 2. 现状盘点(敏感信息清单)

按**子系统**枚举,标注当前存储位置与生命周期。

### 2.1 LLM 凭证

| 项 | 位置 | 现状 |
|----|------|------|
| `[providers.{name}] api_key` | `config.toml` + `secrets.rs` | `SecretRef`,支持 env/file/exec/inline;内联值=明文 config |
| `[providers.{name}] api_keys` | `config.toml` | `SecretRef` 列表(轮换候选) |
| 单 `api_key`(兼容字段) | `config.toml` | 明文 |
| env 覆盖 | 环境变量 | `SYSCITY_PROVIDER_{NAME}_KEY` |

优先级(`src/model_router/oauth_credential.rs:172`):
`SYSCITY_PROVIDER_{NAME}_KEY` > config `api_keys` > 单 `api_key`。

多 LLM 现状:env 名已是 `{NAME}` 分键,但语义上仍是"单 key 全局使用",
不支持多 provider 各自的 key 并行管理。**未来切换 LLM 要求 per-provider 分键**。

### 2.2 MCP 服务器凭证

| 项 | 位置 | 现状 |
|----|------|------|
| OAuth access + refresh token | `~/.syscity/mcp_tokens/{id}.json` | 明文 JSON,0644,无 sanitize,mtime 缓存(`src/mcp/oauth.rs:556`) |
| env token(用户输入,如 `GITHUB_PERSONAL_ACCESS_TOKEN`) | `~/.syscity/mcp_env/{id}.toml` | 0600/0700,原子写,sanitize(`src/mcp/env_store.rs`) |
| `$VAR` 引用 | `config.toml` `[mcp.servers.{id}.env]` | 保留 env-ref feature,不落盘值 |

### 2.3 Channel 凭证

全部在 `config.toml` `[channels.{id}.*]` 明文:

| Channel | 字段 |
|---------|------|
| whatsapp | `access_token`(`src/channels/whatsapp.rs:38`) |
| qq | `app_secret`、`access_token`(`src/channels/qq.rs:32,36`) |
| imessage | `api_password`(`src/channels/imessage.rs:43`) |
| lark | `app_secret`、`tenant_access_token`(`src/channels/lark.rs:30,38`) |

### 2.4 Webhook / 安全层

| 项 | 位置 | 现状 |
|----|------|------|
| `webhook_secret` | `config.toml` channel `credentials` map(`src/gateway/webhooks.rs:431,716`) | 明文 |
| `security.shared_token` | `config.toml` `[security]` 或 env `SYSCITY_SECURITY_SHARED_TOKEN` | 明文 / env |
| 网关 OAuth `client_secret` | `config.toml` `[security.oauth]` | 明文 |

> 注意:`[security.rate_limit.shared_secret/device_token/hook_auth]` 是**限流 tier**,
> 不是 secret 值;真正的 secret 是 `shared_token` 与 device registry 里的 token。

### 2.5 插件 / 其它

| 项 | 位置 | 现状 |
|----|------|------|
| plugin manifest `secret_key`(签名用) | 请求内传入(`src/gateway/handlers/plugins.rs:26,303`) | 明文,请求体 |
| device pairing token | device registry / 会话 | 系统生成 |
| `~/.syscity/` 下其它目录(agents/artifacts/budget/…) | 磁盘 | 非 secret,本文档不涉及 |

---

## 3. 威胁模型

syscity 主要形态是**本地单用户桌面运行**(macOS 为主),但也可部署为服务器。

| 威胁 | 现有防护 | 缺口 |
|------|----------|------|
| 同机其他用户读文件 | 0600(mcp_env) | config.toml / mcp_tokens 0644 |
| `~` 目录备份 / 同步到云 | 无 | **0600 挡不住备份泄露 → 需要加密** |
| 子进程 env 泄露 | env 只在需要时注入 | MCP stdio 子进程继承完整 env |
| 日志 / 崩溃转储 | secrets.rs 已 redact | 其它路径可能打印 token |
| 路径穿越 | mcp_env 已 sanitize | mcp_tokens 未 sanitize |
| 恶意进程(同用户) | 无 | keyring 值不进文件,天然免疫 |

关键结论:**0600 只防同机其他用户;防不了备份/同步/同用户恶意进程。
OS keyring 同时解决这两者**。

---

## 4. 设计原则(行业最佳实践)

基于 OWASP Secrets Management Cheat Sheet 与 2026 年桌面 OAuth 存储共识:

1. **config 永不存值,只存引用**(`$VAR` / `SecretRef` / keyring key 名)。
2. **不聚合**:每实体独立条目 —— 避免"一个明文文件读一次全暴露"。
3. **OS keyring 是桌面默认**:macOS Keychain / Windows DPAPI / Linux Secret Service;
   无头环境回退 0600 文件 + 用户知情。
4. **OAuth 生命周期**:access token **只留内存**;仅 refresh token 持久化。
5. **PKCE + 最小 scope**:桌面公开客户端不需要 client_secret。
6. **短生命周期 + 轮换**:能短则短,泄露即吊销。
7. **禁用/删除即清理**:关闭服务时同步删除其 secret。
8. **全路径 redact + zeroize**:Debug 输出、日志、内存值。

---

## 5. 分类体系(Taxonomy)

三组正交维度:

### 5.1 生命周期(lifecycle)

| 类型 | 特征 | 代表 |
|------|------|------|
| **静态稳定型** | 设一次、长期读、极少轮换 | LLM key、MCP env token、channel token、webhook_secret、`secret_key` |
| **动态轮换型** | 会过期 / 可撤销 / 需刷新 | OAuth access(refresh)、session、device pairing token |
| **临时会话型** | 只在内存、TTL、drop 清零 | secrets.rs 解析值、access token |

### 5.2 作用域(scope)

| 类型 | 代表 |
|------|------|
| **global** | `shared_token`、网关 OAuth `client_secret` |
| **per-entity**(provider / server / channel / plugin / device) | LLM key、MCP env、channel token、`secret_key` |

> 多 LLM 切换要求 LLM key 按 **per-provider** 分键 —— 与 MCP env token 按
> `server_id`、keyring 按 (service, account) 的模式一致。

### 5.3 方向(direction) / 威胁模型

| 类型 | 泄露后果 | 代表 |
|------|----------|------|
| **对外认证** | 冒充我们去消费 / 调第三方 API | LLM key、MCP token、channel token |
| **对内验证** | 伪造请求打我们 | `shared_token`、`webhook_secret`、device token |
| **签名** | 伪造合法插件 | plugin `secret_key` |

### 5.4 来源(source)

| 类型 | 代表 |
|------|------|
| **用户 / 第三方签发** | 各类 token、api key —— 丢失不可自愈 |
| **系统生成** | device pairing token、plugin `secret_key`(若由系统生成)—— 可重新生成 |

---

## 6. 存储分层架构

五层,由"最安全"到"最常读写":

```
┌─────────────────────────────────────────────────────────┐
│ Tier 0  配置引用层  config.toml                          │  ← 只存引用,永不存值
│         SecretRef("$VAR") / keyring key 名               │
├─────────────────────────────────────────────────────────┤
│ Tier 1  OS keyring(默认)                                 │  ← 用户输入、持久、高价值
│         macOS Keychain / Win DPAPI / Linux SecretService │     → 走 keyring crate
├─────────────────────────────────────────────────────────┤
│ Tier 2  文件回退(无头/系统生成)                          │  ← 无 keyring 或可重新生成
│         ~/.syscity/secrets/{ns}/{entity}.toml  0600/0700 │     → file_store 原子写
├─────────────────────────────────────────────────────────┤
│ Tier 3  内存 only(access token、运行期解析值)             │  ← zeroize + TTL
│         secrets.rs runtime snapshot                      │
├─────────────────────────────────────────────────────────┤
│ Tier 4  外部注入(env / file / exec)                      │  ← 运维注入的 SecretRef
│         SYSCITY_PROVIDER_{NAME}_KEY 等                   │
└─────────────────────────────────────────────────────────┘
```

**选层规则**(判定优先级从高到低):

1. **临时会话型 / access token** → Tier 3(内存)。
2. **运维注入来源**(env/file/exec)已配置 → Tier 4,不落盘。
3. **系统生成且无头必须可用** → Tier 2(文件)。
4. **用户输入 / 第三方签发、持久、高价值** → 有 keyring → Tier 1,否则 Tier 2。

---

## 7. 各类别设计方案

| 类别 | 生命周期 | 作用域 | 方向 | 存储方案 |
|------|----------|--------|------|----------|
| LLM api key | 静态 | per-provider | 对外 | Tier 1(主)/ Tier 2(回退);env 优先保留 |
| MCP env token | 静态 | per-server | 对外 | Tier 1(主)/ Tier 2(回退) |
| MCP OAuth **refresh** | 轮换 | per-server | 对外 | **Tier 1(必须)**;无头回退 Tier 2 |
| MCP OAuth **access** | 临时 | per-server | 对外 | **Tier 3 内存 only** |
| Channel token | 静态/轮换 | per-channel | 对外 | Tier 1(主)/ Tier 2(回退) |
| `webhook_secret` | 静态 | per-channel | 对内 | Tier 2 / Tier 4(运维注入) |
| `security.shared_token` | 静态 | global | 对内 | **Tier 4 env 为主**,config 明文向后兼容 |
| 网关 OAuth `client_secret` | 静态 | global | client | 桌面用 **PKCE 免存**;需要则 Tier 1 |
| plugin `secret_key` | 静态 | per-plugin | 签名 | Tier 2(系统生成) |
| device pairing token | 轮换 | per-device | 对内 | Tier 2 注册表(系统生成) |

### 7.1 LLM 凭证(多 LLM 支持)

```toml
# config.toml —— 只存引用
[providers.anthropic]
api_key = { store = "keyring", ns = "llm", entity = "anthropic" }  # 新引用形式
# 兼容:api_key = "$ANTHROPIC_API_KEY" 或 { env = "..." } 保留

[providers.openai]
api_key = { store = "keyring", ns = "llm", entity = "openai" }
```

- 每个 provider 一个 keyring 条目:`(service="syscity/llm", account="{provider}")`。
- 路由切换 LLM 时,`model_router` 按 provider 名取对应条目。
- env `SYSCITY_PROVIDER_{NAME}_KEY` **优先级最高**,不变(Tier 4)。

### 7.2 MCP

| 值 | 新方案 |
|----|--------|
| OAuth refresh token | keyring:`(service="syscity/mcp", account="{server_id}")`;无头回退 `mcp_tokens/{id}.json` 加密版 |
| OAuth access token | **内存 only**,从 refresh 续期;不再持久化 |
| env token(用户输入) | keyring:`(service="syscity/mcp-env", account="{server_id}")`;无头回退 `~/.syscity/secrets/mcp-env/{id}.toml` |
| `$VAR` 引用 | 保留在 `config.env`,不动 |

`McpManager::connect` 的合并点不变:`resolved_env` 字段在 spawn 前从统一
SecretStore 读取并合并,保证「重启后自动重连」。

### 7.3 Channel / Webhook

```toml
# config.toml
[channels.whatsapp]
# access_token 不再明文
access_token = { store = "keyring", ns = "channel", entity = "whatsapp" }
# 或兼容现有:access_token = "plaintext"(向后兼容,提示迁移)

[channels.qq]
app_secret = { store = "keyring", ns = "channel", entity = "qq" }
access_token = { store = "keyring", ns = "channel", entity = "qq" }
```

- channel token:用户输入、持久、对外 → Tier 1。
- `webhook_secret`:对内验证、常由运维配置 → 保留 env/文件注入(Tier 4),
  明文向后兼容。

### 7.4 Security

- `shared_token`:**默认仍走 env**(`SYSCITY_SECURITY_SHARED_TOKEN`,运维注入)。
  无 env 时回退 config 明文(现状),可加 keyring 可选。
- 网关 OAuth `client_secret`:桌面 PKCE 下不需要;如需,进 Tier 1。
- device pairing token:系统生成,注册表(Tier 2)保持不变。

### 7.5 Plugin

- `secret_key`:签名用途、系统生成 → Tier 2 文件;请求体明文路径保留向后兼容,
  后续可改为从 store 读取。

---

## 8. 实现设计

现有单文件 `src/secrets.rs` 转为 `src/secrets/` 目录,按职责拆分:

```
src/secrets/
├── mod.rs             ← 现 secrets.rs 迁入(SecretRef / SecretResolver / resolve_secret_or_ref)
├── store.rs           ← SecretStore trait + SecretId + 选层路由
├── keyring_store.rs   ← Tier 1 后端(OS keyring)
├── file_store.rs      ← Tier 2 后端(0600/0700 原子写;含 sanitize + 写入助手)
└── in_memory.rs       ← Tier 3 后端(zeroize 内存)
```

分层规则:**`secrets/` 是叶子库,其它模块依赖它,它不依赖其它模块**。集中的是
"存储",分散的是"生命周期"——各子系统以调用 `SecretStore` 读值的形式接入,
OAuth 续期等生命周期逻辑留在各自模块。

### 8.1 统一抽象层 `SecretStore`

`SecretStore` trait 与 `SecretId` 定义在 `src/secrets/store.rs`(与现有
`SecretResolver` 并存):

```rust
/// 逻辑 secret 标识 —— 值 = Tier1/Tier2 里的条目;引用 = config 里的键。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretId {
    pub namespace: String, // "llm" | "mcp" | "mcp-env" | "channel" | "security" | "plugin"
    pub entity: String,    // provider / server_id / channel id / 固定名
    pub kind: String,      // "api_key" | "refresh_token" | "access_token" | "secret"
}

pub enum SecretOrigin { UserEntered, SystemGenerated, OperatorInjected }

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, id: &SecretId) -> crate::Result<Option<String>>;
    async fn set(&self, id: &SecretId, value: &str, origin: SecretOrigin) -> crate::Result<()>;
    async fn delete(&self, id: &SecretId) -> crate::Result<()>;
    async fn has(&self, id: &SecretId) -> bool;
}
```

### 8.2 后端

**`KeyringStore`(Tier 1)**

- 依赖:`keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }`。
- 映射:`Entry::new("syscity/{namespace}", &format!("{entity}/{kind}"))`。
- **可用性探测**:启动时写/删一个 `probe` 条目;失败(无 keychain /
  "SS error: prompt dismissed")→ 全局切 Tier 2。
- 同步后端在 async runtime 内调用会阻塞 → 用 `spawn_blocking` 包裹。

**`FileStore`(Tier 2,回退)**

- 写入模式沿用 `src/mcp/env_store.rs` 已验证的实现(temp + `set_permissions(0o600)`
  + 原子 rename;目录 `0o700`;id `sanitize`),整体吸收进 `src/secrets/file_store.rs`。
- **`src/mcp/env_store.rs` 退役**:逻辑并入 `file_store.rs`,`mcp` 侧不再有独立的
  env-store 模块,所有调用(manager/ws)改走 `SecretStore`。
- 路径:`~/.syscity/secrets/{namespace}/{entity}.toml`(每实体一个文件,不聚合)。
- 文件结构:`[secrets] kind = "value"`。
- 兼容读旧格式:`~/.syscity/mcp_env/{id}.toml`(`[env]` 表)在迁移完成前仍可读。
- 可选增强(阶段 2):值 AES-256-GCM 加密,主密钥放 keyring;无 keyring 时
  用派生密钥或退明文 + 0600(显式标记)。

**`EnvStore`(Tier 4)/ `InMemoryStore`(Tier 3)**

- Tier 4 复用现有 `SecretResolver`(env/file/exec)。
- Tier 3 复用现有 zeroize 内存结构(access token 放这里)。

### 8.3 选层路由

```rust
// 启动时探测一次;运行中不变
static BACKEND: OnceCell<BackendKind> = ...; // Keyring | File

fn choose_store(id: &SecretId) -> Arc<dyn SecretStore> {
    match (BACKEND.get(), id.origin) {
        (Keyring, UserEntered) => keyring_store(),
        (Keyring, OperatorInjected) => env_backed_store(), // 运维注入不落盘
        (Keyring, SystemGenerated) => file_store(),        // 系统生成留文件
        (File, _) => file_store(),
    }
}
```

### 8.4 统一读取点

所有子系统读取 secret 一律走:

```rust
// 返回:env 优先 > keyring/file store > config 明文(兼容,打警告)
async fn resolve_secret_or_ref(id: Option<SecretId>, legacy_ref: Option<SecretRef>) -> ...
```

- 已有 `SecretRef`(env/file/exec)的路径保持优先。
- 新增 keyring 引用(`{ store = "keyring", ... }`)解析到 Tier 1/2。
- 兼容既有明文字段:读到非引用值 → 照常使用 + `warn!` 提示迁移。

### 8.5 OAuth 改造(`src/mcp/oauth.rs`)

- `handle_callback_complete`:只把 **refresh token** 写 Tier 1;access token 留在
  内存缓存,到期由 `refresh_expiring_tokens` 续。
- `handle_get_token`:keyring 读 refresh → 需要时换 access(内存)。
- `handle_clear_token`:删除 keyring 条目(覆盖现有删文件路径)。

### 8.6 迁移

| 从 | 到 | 策略 |
|----|----|------|
| `mcp_env/*.toml`(0600 明文) | keyring / `~/.syscity/secrets/mcp-env/*.toml` | 首次启动自动迁入,迁移成功后删旧文件;`env_store.rs` 随之退役 |
| `mcp_tokens/*.json`(access+refresh 明文) | refresh→keyring,access→内存 | 自动拆解;旧文件删除 |
| config.toml channel/security/plugin 明文 | keyring 引用 | **不自动改用户文件**;遇到明文值 → 用 + `warn!` 引导 |
| 已有 env `$VAR` | 保留 | 优先级最高,不动 |

迁移必须**原子且可回滚**:写入新位置成功后才删旧文件;任一步失败保持现状。

---

## 9. 兼容性

- 现有 `config.toml` 用户:**零破坏**。明文字段照常工作,只加 `warn!` 提示。
- 现有 `mcp_env` / `mcp_tokens` 文件:**自动迁移**到新布局,失败回退旧格式读取
  (兼容读保留至迁移确认后删除)。
- `SYSCITY_PROVIDER_{NAME}_KEY`、`SYSCITY_SECURITY_SHARED_TOKEN` env:**不变**。
- MCP `$VAR` env-ref feature:**不变**。
- keyring v3 → v4(alpha,breaking):锁定 v3;v4 稳定后评估(默认 feature、
  移除 async-secret-service/keyutils)。

---

## 10. 测试与验证

**单元测试**
- `SecretId` 规范化、`sanitize`(拒 `../`、`/`、空)。
- `FileStore` 写读删 roundtrip + unix 权限断言(0600/0700)+ 原子写。
- `KeyringStore` 逻辑测试走 **mock 后端**(keyring 的 mock feature),不依赖真实钥匙串。
- 选层路由:按 origin / backend 矩阵断言。
- OAuth:access 不落盘;refresh 落 keyring;clear 删除条目。

**集成测试**
- 有 keyring:真实读写在 macOS Keychain,roundtrip per namespace。
- 无 keyring(无头):强制 `File` 后端,全链路可跑。
- 迁移:旧 `mcp_env`/`mcp_tokens` → 新 store 原子迁移 + 回滚;迁移后
  `env_store` 模块可安全删除。

**手动验证矩阵**

| 场景 | 预期 |
|------|------|
| macOS 启用 GitHub 预设 | 填 token → 存 Keychain,无密码/指纹提示 |
| 无头 Linux 启用 | 回退 0600 文件,启动日志提示后端为 File |
| 重启 daemon | MCP 自动重连,无需重填 |
| 禁用服务器 | keyring/file 条目删除,`mcp.list` 无 `env_configured` |
| 备份 `~/.syscity` | 值在 keyring,备份文件无明文 token |
| 多 LLM 切换 | 每个 provider 独立条目,路由按名取 key |

---

## 11. 风险与未决问题

| 项 | 说明 | 对策 |
|----|------|------|
| keyring 平台差异 | macOS 完美;Linux 无头不可用;WSL 权限问题 | 自动探测 + File 回退 + 日志提示 |
| 同步后端阻塞 async | SecretService 是同步 API | `spawn_blocking` 包裹 |
| 无头对"必须 keyring"类的依赖 | refresh token 无 keyring 时落加密文件 | 阶段 2 加密回退 |
| 迁移破坏用户配置 | 自动改文件风险 | 只迁 mcp_env/mcp_tokens;config 明文仅提示 |
| keyring v4 breaking | v3→v4 API/feature 变化 | 锁 v3,后续评估 |
| `warn!` 迁移提示噪音 | 明文旧配置触发告警 | 提供 `syscity secrets migrate` 一键迁移命令 |

---

## 12. 分阶段实施

| 阶段 | 内容 | 依赖 |
|------|------|------|
| **0** | `src/secrets.rs` → `src/secrets/` 目录;`FileStore` 吸收 `env_store` 逻辑并退役后者;`mcp_tokens` 对齐 0600/0700 + sanitize;OAuth 拆 access(内存)/refresh(持久) | 零新依赖 |
| **1** | `SecretStore` 抽象 + `KeyringStore`(macOS 优先)+ `FileStore` 回退;LLM/MCP 接入 | 引入 `keyring` crate |
| **2** | channel/security/plugin 迁移;`resolve_secret_or_ref` 统一读取;`syscity secrets migrate` | 阶段 1 |
| **3** | 加密文件回退(AES-GCM,key 进 keyring) | 阶段 2 |

---

## 附:与现有代码的对接点

| 文件 | 改动 |
|------|------|
| `src/secrets.rs` → `src/secrets/mod.rs` | `SecretRef` 增加 keyring 引用形式;`resolve_secret_or_ref` |
| `src/secrets/store.rs`(新) | `SecretStore` trait + `SecretId` + 选层路由 |
| `src/secrets/keyring_store.rs`(新) | Tier 1 OS keyring 后端 |
| `src/secrets/file_store.rs`(新) | Tier 2 文件后端;吸收 `env_store` 写入逻辑 |
| `src/mcp/env_store.rs` | **删除**;旧 `mcp_env/*.toml` 由迁移路径读入 `FileStore` |
| `src/mcp/oauth.rs` | refresh→keyring,access→内存,clear 删条目 |
| `src/mcp/manager.rs` | connect 合并点改走 SecretStore |
| `src/mcp/config.rs` | `McpServerConfig.resolved_env` 读取来源扩展 |
| `src/model_router/oauth_credential.rs` | per-provider keyring 读取 |
| `src/channels/*` | token 支持 keyring 引用 + 明文兼容 |
| `src/gateway/webhooks.rs` | `webhook_secret` 支持 env/file 注入 |
| `src/gateway/handlers/plugins.rs` | `secret_key` 支持从 store 读取 |
| `src/gateway/ws.rs` | `mcp.add` env 写入改走 SecretStore;`mcp.list` 状态不变 |
