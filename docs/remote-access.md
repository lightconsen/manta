# 远程访问（Remote Gateway Access）

本文说明如何让 Syscity Gateway 监听远程客户端，以及客户端如何连接远程 Gateway。

## 场景

- 在办公室/家里的机器上跑 Gateway，人在外面用 Desktop / Mobile / 浏览器连过来。
- 多个设备共享同一个 Gateway（同一份会话、配置、渠道），而不是各跑一个。

## 一、开启远程监听（Gateway 侧）

### 1. 绑定所有网卡

Gateway 默认只监听 `127.0.0.1`。要接受远程连接，绑定 `0.0.0.0`：

```bash
syscity start --host 0.0.0.0
```

> ⚠️ 注意：当前 `config.toml` 里的 `[server] host` 字段**不会被 Gateway 采用**——Gateway 的
> `GatewayConfig` 无法解析自动生成的 `[server]`/`[model]` 配置段（模板与 serde 结构不匹配，
> 启动日志会报 `Failed to parse config.toml`）。监听地址**只能通过 `--host` 指定**。

### 2. 开启鉴权（必须）

远程监听默认 `auth_mode = "none"`，**裸奔**。启用 token 鉴权（通过环境变量，这是当前可靠的路径）：

```bash
SYSCITY_SECURITY_AUTH_MODE=token \
SYSCITY_SECURITY_AUTH_REQUIRED=true \
SYSCITY_SECURITY_SHARED_TOKEN=<你的token> \
syscity start --host 0.0.0.0
```

- `SYSCITY_SECURITY_AUTH_MODE`：`none` | `token` | `device` | `tailscale`
- `SYSCITY_SECURITY_SHARED_TOKEN`：客户端需要出示的共享令牌
- 同样地，`[security]` 段的配置当前也不被 Gateway 采用，必须走 env。

> 更多鉴权模式（Device 配对、Tailscale）见 `docs/security-config.md`。

### 3. 防火墙

确保机器防火墙放行 `18080`（或你配置的端口），并且该端口对目标网络可达。

### 4. 验证

```bash
curl --noproxy "*" http://127.0.0.1:18080/live   # 200
# WS 握手：
#   无 token → 401
#   带 Authorization: Bearer <token> → 101
```

## 二、客户端连接远程 Gateway

客户端启动时选择「连远程 Gateway」或「本地运行」。远程模式需要填：地址（host:port）+ token。

| 客户端 | 状态 |
|---|---|
| **Desktop** | 设置页「连接」区可选远程；启动时按配置走 |
| **Mobile** | 设置页可选远程 |
| **Web** | 直接打开远程 Gateway 的 Web UI（`http://<远程>:18080`），首次连接输入 token |

### 安全建议

- 局域网内可用明文 `http://`。
- 跨公网必须走 `https://`（WSS）或 Tailscale/VPN，避免 token 明文暴露。
- 远程模式下客户端填写的 token 就是 Gateway 的 `SYSCITY_SECURITY_SHARED_TOKEN`。

## 三、已知问题

- **config.toml 解析 bug**：Gateway 无法解析自动生成的 `[server]`/`[model]` 配置段，
  导致 `config.toml` 里的 `host`/`port`/`security` 等配置**全部被忽略**（Gateway 用默认值 +
  `--host`/env 覆盖）。已通过 env 覆盖规避，但 config 文件本身需要修复（模板与
  `GatewayConfig` serde 对齐）。
