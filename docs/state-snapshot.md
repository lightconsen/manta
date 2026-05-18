# 状态快照机制（State Snapshot）

> **关联文档**: [protocol.md](./protocol.md)
> **状态**: 参考文档（OpenClaw 实现），Manta 暂不实现

---

## 1. 问题背景

在多客户端场景下，多个设备（Web、App、CLI）同时连接到同一个 Gateway 时，会面临状态同步问题：

- **新客户端首次连接**：如何知道"当前"的完整状态？
- **断线重连**：断线期间错过了哪些事件？
- **多端并发**：一个客户端的操作，其他客户端如何感知？

SSE 或纯事件流无法解决这些问题——客户端只能收到"从现在开始"的事件，对之前的状态一无所知。

---

## 2. 核心概念

状态快照机制通过三个要素解决上述问题：

| 概念 | 说明 |
|------|------|
| **Snapshot** | 连接握手时服务端发送的完整状态快照 |
| **StateVersion** | 每个状态维度的单调递增版本号 |
| **Event + Version** | 每个事件携带其发生时的状态版本 |

### 2.1 Snapshot（快照）

在 WebSocket `connect` 握手成功后，服务端回复 `hello-ok`，其中包含一个 `snapshot` 字段：

```json
{
  "type": "hello-ok",
  "protocol": 3,
  "features": { "methods": [...], "events": [...] },
  "snapshot": {
    "presence": [
      {
        "deviceId": "device_abc",
        "host": "192.168.1.100",
        "platform": "macos",
        "roles": ["operator"],
        "scopes": ["chat", "read", "write"],
        "ts": 1716123456
      }
    ],
    "health": {
      "status": "healthy",
      "agents": 5,
      "channels": 3
    },
    "stateVersion": {
      "presence": 42,
      "health": 7
    },
    "uptimeMs": 123456789,
    "authMode": "token",
    "sessionDefaults": {
      "defaultAgentId": "default",
      "mainKey": "web:user_001",
      "mainSessionKey": "web:user_001"
    }
  }
}
```

**Snapshot 包含的数据：**

| 字段 | 说明 |
|------|------|
| `presence` | 当前所有在线设备/客户端列表 |
| `health` | Gateway 健康状态（Agent 数量、Channel 状态等） |
| `stateVersion` | 各维度的当前版本号 |
| `uptimeMs` | Gateway 运行时长 |
| `authMode` | 当前鉴权模式 |
| `sessionDefaults` | 默认 Session 配置 |
| `updateAvailable` | 是否有新版本可用（可选） |

### 2.2 StateVersion（状态版本）

每个需要同步的状态维度都有一个独立的单调递增版本号：

```json
{
  "stateVersion": {
    "presence": 42,    // presence 列表第 42 版
    "health": 7        // health 状态第 7 版
  }
}
```

**规则：**
- 每次该维度的状态发生变化，版本号 +1
- 不同维度的版本独立递增，互不干扰
- 版本号从 0 开始，永不清零

### 2.3 Event + StateVersion

后续的每个 `event` 帧都携带其发生时的状态版本：

```json
{
  "type": "event",
  "event": "presence.update",
  "payload": {
    "deviceId": "device_xyz",
    "status": "online"
  },
  "seq": 100,
  "stateVersion": {
    "presence": 43,
    "health": 7
  }
}
```

客户端可以对比 `event.stateVersion` 和自己本地的版本，判断是否有事件丢失。

---

## 3. 典型场景

### 3.1 新客户端首次连接

```
浏览器首次打开
  |
  |---- connect req --->
  |
  |<--- hello-ok res ---
  |      snapshot: {
  |        presence: [手机, iPad],
  |        stateVersion: { presence: 45 }
  |      }
  |
  浏览器现在知道：当前有 2 个设备在线
  后续通过 event 增量更新
```

### 3.2 断线重连

```
客户端 A 在线，stateVersion: { presence: 40 }
  |
  |<--- event: presence.update (v41) ---
  |<--- event: presence.update (v42) ---
  |<--- event: presence.update (v43) ---
  |
  [网络中断]
  |
  |<--- event: presence.update (v44) ---  [丢失]
  |<--- event: presence.update (v45) ---  [丢失]
  |
  [客户端重连]
  |
  |---- connect req --->
  |
  |<--- hello-ok res ---
  |      snapshot.stateVersion: { presence: 45 }
  |
  客户端发现：40 → 45，错过了 5 个 presence 更新
  决策：重新拉取完整 presence 列表，或接受最终一致性
```

### 3.3 多设备感知

```
手机连接 Gateway
  |
  |<--- event: presence.update ---
  |      新设备 "Web浏览器" 上线
  |
  手机 UI 显示："Web浏览器 已连接"
```

---

## 4. 实现要点

### 4.1 服务端

1. **维护版本号**
   ```typescript
   const stateVersions = {
     presence: 0,
     health: 0,
   };

   function bumpPresenceVersion() {
     stateVersions.presence++;
   }
   ```

2. **构建快照**
   ```typescript
   function buildSnapshot() {
     return {
       presence: getAllOnlineClients(),
       health: getHealthStatus(),
       stateVersion: { ...stateVersions },
       uptimeMs: Date.now() - startTime,
     };
   }
   ```

3. **事件携带版本**
   ```typescript
   function broadcastEvent(event, payload) {
     if (event.startsWith('presence.')) bumpPresenceVersion();
     if (event.startsWith('health.')) bumpHealthVersion();

     sendToAllClients({
       type: 'event',
       event,
       payload,
       stateVersion: { ...stateVersions },
     });
   }
   ```

### 4.2 客户端

1. **初始化状态**
   ```typescript
   let localState = null;
   let localVersions = { presence: 0, health: 0 };

   ws.onmessage = (msg) => {
     if (msg.type === 'hello-ok') {
       localState = msg.snapshot;
       localVersions = msg.snapshot.stateVersion;
     }
   };
   ```

2. **增量更新**
   ```typescript
   if (msg.type === 'event') {
     applyEventToLocalState(msg.event, msg.payload);
     localVersions = msg.stateVersion;
   }
   ```

3. **断线检测**
   ```typescript
   function onReconnect(newSnapshot) {
     const missedPresence = newSnapshot.stateVersion.presence - localVersions.presence;
     if (missedPresence > 10) {
       // 丢失太多，全量刷新
       localState = newSnapshot;
     }
     localVersions = newSnapshot.stateVersion;
   }
   ```

---

## 5. 与 Manta 的关系

### 5.1 当前状态

Manta 暂不实现状态快照机制，原因：

| 考虑 | 结论 |
|------|------|
| 主要场景 | 单 Web 终端聊天，暂不需要多设备同步 |
| 复杂度 | 需要维护版本号、快照构建、增量更新逻辑 |
| 收益 | 当前场景下收益有限 |
| 协议兼容性 | `event` 帧预留了扩展字段，未来可无缝添加 |

### 5.2 未来扩展

当 Manta 需要支持以下场景时，可引入状态快照：

- **App + Web 同时在线**：需要感知"其他设备"的状态
- **断线恢复**：需要精确判断丢失了多少事件
- **管理后台**：需要实时展示 Gateway 全局状态

### 5.3 协议预留

Manta 的 `event` 帧格式预留了扩展空间：

```json
{
  "type": "event",
  "event": "chat.delta",
  "payload": { ... },
  "seq": 42
  // 未来可添加:
  // "stateVersion": { "presence": 10, "health": 3 }
}
```

添加 `stateVersion` 不会影响现有客户端（它们会忽略未知字段）。

---

## 6. 对比：有无状态快照

| 场景 | 无状态快照 | 有状态快照 |
|------|-----------|-----------|
| 新客户端连接 | 只能收到后续事件，对当前状态一无所知 | 收到完整 snapshot，立即了解全局状态 |
| 断线 5 分钟 | 重连后不知道丢了什么 | 对比 version，精确知道丢失范围 |
| 多端在线 | 各客户端状态可能不一致 | 基于 snapshot + 增量更新保持一致 |
| 实现复杂度 | 低 | 中（需要版本管理和快照构建） |

---

## 7. 参考实现

- **OpenClaw**: `src/gateway/protocol/schema/snapshot.ts`
- **OpenClaw 握手**: `src/gateway/server/ws-connection/message-handler.ts` (hello-ok 发送 snapshot)
- **OpenClaw 广播**: `src/gateway/server-broadcast.ts` (event 携带 stateVersion)
