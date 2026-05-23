# Issue: OpenClaw Security 层的复杂机制

## 背景

Manta 当前在 `src/security/` 中实现了渗透测试、审计、滑动窗口、mention gate、设备配对、运行时审计等安全能力。OpenClaw 的 Security 层在相似的功能基础上，构建了一套更完善的安全体系，涵盖多维度允许列表（allowlist）、认证模式策略（auth mode policy）、精细化速率限制（rate limit）、设备配对挑战（pairing challenge）和密钥引用解析（secret resolution）等机制。这些机制共同保障了 OpenClaw 作为多平台 AI Gateway 在开放网络环境中的安全性。

---

## 1. Allowlist System — 多维度允许列表系统

### 核心类型定义

`src/channels/allowlist-match.ts` 定义了允许列表的完整匹配模型：

```typescript
export type AllowlistMatchSource =
  | "wildcard"      // 通配符 *
  | "id"            // 用户/发送者 ID
  | "name"          // 显示名称
  | "tag"           // 标签
  | "username"      // 用户名
  | "prefixed-id"   // 带前缀的 ID（如 telegram:12345）
  | "prefixed-user" // 带前缀的用户名
  | "prefixed-name" // 带前缀的名称
  | "slug"          // 渠道 slug
  | "localpart";    // 本地部分（如 email 的 @ 前部分）

export type AllowlistMatch<TSource extends string = AllowlistMatchSource> = {
  allowed: boolean;
  matchKey?: string;
  matchSource?: TSource;
};

export type CompiledAllowlist = {
  set: ReadonlySet<string>;
  wildcard: boolean;
};
```

### 编译与匹配引擎

**`compileAllowlist(entries)`** — 将字符串列表编译为高效查找结构：
- 过滤空字符串
- 检测通配符 `*` 存在性
- 生成 `ReadonlySet` 用于 O(1) 查找

**`resolveAllowlistMatchByCandidates(params)`** — 主匹配引擎：
1. 接收候选列表：`{ value?: string; source: AllowlistMatchSource }[]`
2. 按优先级遍历候选（id → username → name → tag → ...）
3. 返回首个匹配的 `{ allowed: true, matchKey, matchSource }`
4. 通配符 `*` 立即授予访问权限
5. 无匹配返回 `{ allowed: false }`

**`resolveAllowlistMatchSimple(params)`** — 简化变体：
- senderId 和 senderName 小写规范化
- 支持可选的 name matching（`allowNameMatching` 标志）
- 优先检查 senderId，其次 senderName

### Allow-from 存储层

`src/pairing/pairing-store.ts` 实现了持久化的允许列表存储：

```typescript
// 存储格式
{
  version: 1,
  allowFrom: string[]  // 允许列表条目
}
```

**特性**：
- **文件级锁定**：`withFileLock` 保证并发安全（10 次重试 + 指数退避）
- **Account 作用域**：`${channel}-${accountId}-allowFrom.json`
- **向后兼容**：无作用域的旧文件合并到默认 account
- **读取缓存**：基于 `mtimeMs + size` 避免重复磁盘读取
- **源合并**：`mergeDmAllowFromSources()` 合并配置文件 allowlist + store-backed allowlist

### 群组与私聊差异化策略

- **DM Policy**：合并显式配置和 store-backed allowlist，任何来源允许即可
- **Group Policy**：`resolveGroupAllowFromSources()` 支持 fallback 逻辑，可按群组独立配置

---

## 2. Auth Mode Policy — 认证模式策略

### 多模式认证体系

`src/gateway/auth.ts` 支持六种认证方式：

```typescript
type GatewayAuthMethod =
  | "none"            // 无认证（开发模式）
  | "token"           // Bearer Token
  | "password"        // 密码认证
  | "tailscale"       // Tailscale 网络认证
  | "trusted-proxy"   // 可信代理认证
  | "bootstrap-token"; // 设备引导令牌
```

### 模式歧义检测

`src/gateway/auth-mode-policy.ts` 实现了配置时的歧义检测：

```typescript
function hasAmbiguousGatewayAuthModeConfig(cfg: OpenClawConfig): boolean {
  // 当 token 和 password 同时配置，但 mode 未显式设置时
  // 返回 true，否则 false
}

function assertExplicitGatewayAuthModeWhenBothConfigured(cfg: OpenClawConfig): void {
  // 抛出 EXPLICIT_GATEWAY_AUTH_MODE_REQUIRED_ERROR
  // "Invalid config: gateway.auth.token and gateway.auth.password are both configured,
  //  but gateway.auth.mode is unset. Set gateway.auth.mode to token or password."
}
```

**设计意图**：防止用户同时配置 token 和 password 但忘记指定使用哪种，导致不可预期的认证行为。

### 认证解析优先级

`src/gateway/auth-resolve.ts` 的解析顺序：

```typescript
type ResolvedGatewayAuth = {
  mode: "none" | "token" | "password" | "trusted-proxy";
  modeSource?: "override" | "config" | "password" | "token" | "default";
  token?: string;
  password?: string;
  allowTailscale: boolean;
  trustedProxy?: GatewayTrustedProxyConfig;
};
```

**解析流程**：
1. `authOverride.mode` → modeSource: "override"
2. `authConfig.mode` → modeSource: "config"
3. 配置了 password → mode: "password", modeSource: "password"
4. 配置了 token → mode: "token", modeSource: "token"
5. 默认 → mode: "token", modeSource: "default"

### 凭证来源与优先级

`src/gateway/credentials.ts` 支持环境变量与配置文件的优先级控制：

```typescript
type CredentialPrecedence = "env-first" | "config-first";

// 环境变量
OPENCLAW_GATEWAY_TOKEN
OPENCLAW_GATEWAY_PASSWORD
```

- `tokenPrecedence` / `passwordPrecedence` 独立配置
- 支持 `SecretRef` 对象（非明文存储）
- 当 `SecretRef` 无法在当前命令路径解析时，抛出 `GatewaySecretRefUnavailableError`

### 请求级认证授权

`src/gateway/auth.ts` 的 `authorizeGatewayConnect(params)`：

- **速率限制前置**：先检查限流器，再验证凭证
- **Tailscale 认证**：验证 `tailscale-user-login` 请求头，通过 `tailscale whois` 反向查询确认
- **可信代理认证**：
  - 验证代理 IP 是否在白名单
  - 检查必需请求头是否存在
  - 从配置的头中提取用户标识
  - 应用 `allowUsers` 白名单
- **Token/Password**：使用 `safeEqualSecret()` 进行常量时间比较，防止时序攻击

---

## 3. Rate Limiting — 精细化速率限制

### 滑动窗口认证限流

`src/gateway/auth-rate-limit.ts` 实现了基于滑动窗口的认证限流：

```typescript
export interface RateLimitConfig {
  maxAttempts?: number;      // @default 10
  windowMs?: number;         // @default 60_000 (1 分钟)
  lockoutMs?: number;        // @default 300_000 (5 分钟)
  exemptLoopback?: boolean;  // @default true
  pruneIntervalMs?: number;  // @default 60_000
}

export interface RateLimitEntry {
  attempts: number[];        // 失败尝试时间戳数组
  lockedUntil?: number;      // 锁定到期时间戳
}

export interface RateLimitCheckResult {
  allowed: boolean;
  remaining: number;
  retryAfterMs: number;
}
```

**核心逻辑**：
1. **Key 格式**：`${scope}:${normalizedIp}`
2. **多作用域隔离**：
   - `AUTH_RATE_LIMIT_SCOPE_DEFAULT` — 默认认证
   - `AUTH_RATE_LIMIT_SCOPE_SHARED_SECRET` — 共享密钥
   - `AUTH_RATE_LIMIT_SCOPE_DEVICE_TOKEN` — 设备令牌
   - `AUTH_RATE_LIMIT_SCOPE_HOOK_AUTH` — Webhook 认证
3. **本地回环豁免**：`127.0.0.1` 和 `::1` 默认豁免
4. **滑动窗口**：检查时过滤掉 `now - windowMs` 之前的记录
5. **自动锁定**：`attempts.length >= maxAttempts` 时锁定 `lockoutMs`
6. **自动清理**：定时器移除过期条目（`unref` 不阻塞进程退出）

### 控制平面写限流

`src/gateway/control-plane-rate-limit.ts` 保护配置变更等写操作：

```typescript
const MAX_REQUESTS = 3;      // 每窗口最大请求数
const WINDOW_MS = 60_000;    // 1 分钟窗口
const BUCKET_MAX_ENTRIES = 10_000;  // 内存 DoS 保护
```

- **Key**：`${deviceId}|${clientIp}`（未知时回退到 `connId`）
- **固定窗口**：简单高效，适合低频写操作
- **内存保护**：达到最大条目数时驱逐最旧的 bucket
- **定期清理**：移除 5 分钟以上的过期 bucket

### 尝试序列化防竞态

`src/gateway/rate-limit-attempt-serialization.ts` 解决并发竞态：

```typescript
function withSerializedRateLimitAttempt(params: {
  scope: string;
  ip: string;
  attempt: () => Promise<boolean>;
}): Promise<boolean>;
```

**问题场景**：多个并发请求同时通过限流检查，但失败记录尚未写入，导致全部通过。
**解决方案**：使用 `Map<string, Promise<void>>` 按 `{scope, ip}` 串行化异步尝试。

### 集成点

- 认证限流器传给 `authorizeGatewayConnect()` 和 `authorizeHttpGatewayConnect()`
- 失败调用 `limiter.recordFailure(ip, scope)`
- 成功调用 `limiter.reset(ip, scope)`
- 限流响应包含 `Retry-After` 头和 `retryAfterMs`

---

## 4. Pairing Challenge — 设备配对挑战系统

### 配对流程架构

`src/pairing/pairing-challenge.ts` 实现了完整的设备配对挑战机制：

```typescript
export type PairingChallengeParams = {
  channel: string;
  senderId: string;
  senderIdLine: string;
  meta?: PairingMeta;
  upsertPairingRequest: (params: { id: string; meta?: PairingMeta }) => Promise<{ code: string; created: boolean }>;
  sendPairingReply: (text: string) => Promise<void>;
  buildReplyText?: (params: { code: string; senderIdLine: string }) => string;
  onCreated?: (params: { code: string }) => void;
  onReplyError?: (err: unknown) => void;
};
```

### 挑战码生成与存储

`src/pairing/pairing-store.ts` 的 `upsertChannelPairingRequest(params)`：

**编码设计**：
- 8 位字符，来自 `ABCDEFGHJKLMNPQRSTUVWXYZ23456789`
- 排除易混淆字符：`0/O`, `1/I`
- 唯一性保证：最多 500 次尝试，耗尽则抛出错误

**存储策略**：
- TTL：`PAIRING_PENDING_TTL_MS = 60 * 60 * 1000`（1 小时）
- 单 account 最大待处理数：`PAIRING_PENDING_MAX = 3`
- 每次读取时自动清理过期请求
- 超出最大数时保留最新（按 `lastSeenAt`）
- 已存在请求时更新 `lastSeenAt`，保持原 code

### 审批流程

`approveChannelPairingCode(params)`：
1. 规范化 code 为大写
2. 在 pending 列表中查找匹配的 code + accountId
3. 从 store 中移除该请求
4. 将请求者的 `id` 添加到 channel allow-from store
5. 返回 `{ id, entry }` 或 `null`

### 设置码与移动配对

`src/pairing/setup-code.ts` 的 `resolvePairingSetupFromConfig(cfg, options)`：

- **Gateway URL 解析**：按优先级尝试 `publicUrl` → `gateway.remote.url` → Tailscale serve/funnel → 网卡扫描
- **移动端验证**：非本地主机要求 `wss://`
- **引导令牌**：生成 `bootstrapToken`（通过 `issueDeviceBootstrapToken()`）
- **编码**：`{ url, bootstrapToken }` 以 base64url 编码，支持扫码一键配对

### 配对消息模板

`src/pairing/pairing-messages.ts` 提供了可定制的回复模板：
- 默认生成包含 challenge code 的回复文本
- 支持渠道插件自定义 `buildReplyText`

---

## 5. Secret Resolution — 密钥引用解析系统

### SecretRef 类型体系

`src/config/types.secrets.ts` 定义了三种密钥来源：

```typescript
export type SecretRef = {
  source: "env" | "file" | "exec";
  provider: string;
  id: string;
};
```

### 环境变量提供者

```typescript
export type EnvSecretProviderConfig = {
  source: "env";
  allowlist?: string[];  // 可选白名单，拒绝非白名单变量
};
```

**安全特性**：
- 可选白名单限制可访问的环境变量
- 拒绝缺失或空值变量

### 文件提供者

```typescript
export type FileSecretProviderConfig = {
  source: "file";
  path: string;
  mode?: "singleValue" | "json";
  timeoutMs?: number;        // @default 5000
  maxBytes?: number;         // @default 1MB
  allowInsecurePath?: boolean;
};
```

**路径安全断言** (`assertSecurePath()`)：
- 必须是绝对路径
- 可选 `trustedDirs` 包含检查
- **符号链接**：默认拒绝，除非 `allowSymlinkPath` 启用；启用时解析到 realpath
- **权限检查**：
  - 拒绝 world/group 可写
  - 拒绝 world/group 可读（除非 `allowReadableByOthers`）
  - Windows：拒绝 ACL source 为 "unknown"（除非 `allowInsecurePath`）
  - Unix：文件必须由当前 UID 拥有

**读取模式**：
- `singleValue`：返回原始文本（去除尾部换行）
- `json`：解析 JSON，使用 JSON Pointer（`readJsonPointer`）按 `id` 提取值

### 执行命令提供者

```typescript
export type ExecSecretProviderConfig = {
  source: "exec";
  command: string;
  args?: string[];
  timeoutMs?: number;         // @default 5000
  noOutputTimeoutMs?: number;
  maxOutputBytes?: number;    // @default 1MB
  jsonOnly?: boolean;
  env?: Record<string, string>;
  passEnv?: string[];
  trustedDirs?: string[];
  allowInsecurePath?: boolean;
  allowSymlinkCommand?: boolean;
};
```

**执行流程**：
1. 命令路径通过 `assertSecurePath()` 验证
2. 构建请求 payload：`{ protocolVersion: 1, provider, ids }`
3. 以 `stdio: ["pipe", "pipe", "pipe"]`、`shell: false` 启动子进程
4. 通过 stdin 发送请求 JSON
5. 超时控制：`timeoutMs`（默认 5s）+ `noOutputTimeoutMs`
6. 输出限制：`maxOutputBytes`（默认 1MB），超出则 kill 进程
7. 解析 stdout 为 JSON
8. 验证 `protocolVersion === 1`
9. 从 `values` 和 `errors` 中提取结果

### 批量解析与并发控制

`src/secrets/resolve.ts` 的 `resolveSecretRefValues(refs, options)`：

- 按 `{source, provider}` 分组
- `maxProviderConcurrency`（默认 4）限制并发
- `maxRefsPerProvider`（默认 512）限制单提供者引用数
- 通过 `secretRefKey(ref)` = `${source}:${provider}:${id}` 去重

### 引用 ID 验证模式

```typescript
// 环境变量：大写字母+数字+下划线，128字符内
/^[A-Z][A-Z0-9_]{0,127}$/

// 文件：JSON Pointer 格式
/^(?:value|\/(?:[^~]|~0|~1)*(?:\/(?:[^~]|~0|~1)*)*)$/

// 执行命令：字母数字+._:/-，256字符内，无 . 或 .. 段
/^[A-Za-z0-9][A-Za-z0-9._:\/-]{0,255}$/
```

### 错误处理

- `SecretProviderResolutionError` — 提供者级失败（配置缺失、超时、退出码非 0）
- `SecretRefResolutionError` — 引用级失败（ID 缺失、白名单拒绝）

---

## 总结对比

| 机制 | Manta（当前） | OpenClaw |
|------|-------------|----------|
| **Allowlist** | 基础 allowlist | 10+ 匹配源、编译缓存、文件锁持久化、account 作用域 |
| **Auth Mode** | 单一模式 | 6 种认证方式、歧义检测、优先级解析、Tailscale/代理支持 |
| **Rate Limit** | 滑动窗口（基础） | 多作用域隔离、滑动窗口 + 固定窗口、尝试序列化防竞态 |
| **Pairing** | 基础设备配对 | Challenge code（防混淆字符）、TTL/数量限制、store-backed、扫码配对 |
| **Secret** | 环境变量/明文 | 3 种提供者（env/file/exec）、路径安全断言、JSON Pointer、批量并发 |

---

## 对 Manta 的借鉴建议

### 短期

1. **多源 Allowlist 匹配**
   - 扩展 `src/security/` 中的 allowlist，支持多种匹配源（id, username, name, e164）
   - 实现 `compileAllowlist()` 将列表编译为 `HashSet` 加速查找
   - 支持通配符 `*` 和带前缀格式（`telegram:12345`）
   - 配置文件格式：
     ```toml
     [security.allowlist]
     entries = ["telegram:12345", "discord:67890", "admin@example.com"]
     allow_name_matching = true
     ```

2. **Auth Mode 歧义检测**
   - 在配置验证阶段增加 `has_ambiguous_auth_mode()` 检查
   - 当 token 和 password 同时配置但 mode 未设置时，返回明确错误
   - 在 `manta doctor` 或启动时运行该检查

3. **SecretRef 基础支持**
   - 定义 `SecretRef` 类型：`{ source: "env" | "file", provider: String, id: String }`
   - 在 `Config` 中支持 `api_key = { source = "env", provider = "openai", id = "OPENAI_API_KEY" }`
   - 实现 `resolve_secret_ref()` 基础函数

### 中期

4. **精细化速率限制**
   - 在 Gateway 层实现多作用域限流：
     ```rust
     pub struct RateLimiter {
         scopes: HashMap<String, SlidingWindow>,
         config: RateLimitConfig,
     }
     ```
   - 支持 `exempt_loopback`（默认豁免本地回环）
   - 实现尝试序列化防竞态（`withSerializedAttempt`）
   - 为控制平面写操作（配置变更、session 操作）添加独立限流

5. **设备配对挑战机制**
   - 实现 `PairingStore`：JSON 文件存储 + `fs2::FileLock` 并发锁
   - Challenge code 生成：排除易混淆字符，8 位长度
   - TTL 和最大 pending 数限制
   - 审批后自动添加到 allowlist store
   - 支持生成设置码（base64url 编码的 URL + token）

6. **文件密钥提供者**
   - 实现 `FileSecretProvider`：
     - `singleValue` 模式：读取文件原始内容
     - `json` 模式：JSON Pointer 提取
   - 路径安全断言：绝对路径、权限检查、符号链接策略
   - 读取限制：timeout（5s）、max_bytes（1MB）

### 长期

7. **执行命令密钥提供者**
   - 实现 `ExecSecretProvider`：
     - 安全命令路径验证
     - stdin 发送请求 JSON
     - stdout 解析响应 JSON
     - 超时和输出大小限制
   - 定义协议版本（如 `protocolVersion: 1`）
   - 支持批量请求 `{ provider, ids }` 和批量响应 `{ values, errors }`

8. **Tailscale 与可信代理认证**
   - 集成 `tailscale whois` 验证用户身份
   - 可信代理：IP 白名单 + 必需头 + 用户提取 + allowUsers 白名单
   - 在 `src/gateway/` 中增加这些认证方式的支持

9. **密钥解析缓存**
   - 实现 `SecretRefResolveCache`：
     - 按提供者缓存文件 payload
     - 按引用缓存解析结果
     - 支持 TTL 和手动刷新
   - 在 Gateway 启动时预解析常用密钥

---

## 参考代码位置（OpenClaw）

| 文件 | 职责 |
|------|------|
| `src/channels/allowlist-match.ts` | Allowlist 匹配引擎 |
| `src/channels/allow-from.ts` | Allow-from 基础函数 |
| `src/channels/allowlists/resolve-utils.ts` | Allowlist 解析工具 |
| `src/pairing/allow-from-store-file.ts` | Allow-from 文件存储 |
| `src/pairing/pairing-store.ts` | 配对请求存储 + 审批 |
| `src/pairing/pairing-challenge.ts` | 配对挑战核心逻辑 |
| `src/pairing/pairing-messages.ts` | 配对消息模板 |
| `src/pairing/setup-code.ts` | 设置码生成 |
| `src/gateway/auth-mode-policy.ts` | 认证模式歧义检测 |
| `src/gateway/auth-resolve.ts` | 认证解析 |
| `src/gateway/credentials.ts` | 凭证来源解析 |
| `src/gateway/auth.ts` | 请求级认证授权 |
| `src/gateway/auth-rate-limit.ts` | 滑动窗口认证限流 |
| `src/gateway/control-plane-rate-limit.ts` | 控制平面写限流 |
| `src/gateway/rate-limit-attempt-serialization.ts` | 尝试序列化防竞态 |
| `src/infra/fixed-window-rate-limit.ts` | 固定窗口限流通用实现 |
| `src/secrets/resolve.ts` | SecretRef 批量解析 |
| `src/secrets/ref-contract.ts` | SecretRef 契约与验证 |
| `src/config/types.secrets.ts` | Secret 配置类型 |
