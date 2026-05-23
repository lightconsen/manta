# Issue: 多平台客户端架构

## 背景

Manta 当前是一个服务端 Rust 应用，通过 CLI 和 Channel（Telegram/Discord 等）与用户交互。OpenClaw 则构建了一套完整的多平台客户端架构，包含 iOS、Android、macOS 原生应用和共享的 Swift 框架（`OpenClawKit`）。这套架构使得 OpenClaw 能够提供一致的用户体验，同时利用各平台的原生能力。

---

## OpenClaw 的多平台架构

### 1. 整体架构概览

```
OpenClaw 多平台架构
├── apps/
│   ├── android/          # Android 原生应用 (Kotlin + Gradle)
│   ├── ios/              # iOS 原生应用 (Swift + XcodeGen)
│   ├── macos/            # macOS 原生应用 (Swift + SPM)
│   ├── macos-mlx-tts/    # macOS MLX TTS 扩展
│   └── shared/
│       └── OpenClawKit/  # 共享 Swift 框架
├── src/gateway/          # Node.js Gateway 服务端
├── ui/                   # Web UI (Lit-based)
└── extensions/           # 插件扩展
```

### 2. 共享框架层（OpenClawKit）

`apps/shared/OpenClawKit/` 是整个多平台架构的核心，使用 Swift Package Manager 管理：

```swift
// Package.swift
let package = Package(
    name: "OpenClawKit",
    platforms: [.iOS(.v18), .macOS(.v15)],
    products: [
        .library(name: "OpenClawProtocol", targets: ["OpenClawProtocol"]),
        .library(name: "OpenClawKit", targets: ["OpenClawKit"]),
        .library(name: "OpenClawChatUI", targets: ["OpenClawChatUI"]),
    ],
    dependencies: [
        .package(url: "https://github.com/steipete/ElevenLabsKit", exact: "0.1.0"),
        .package(url: "https://github.com/gonzalezreal/textual", exact: "0.3.1"),
    ],
    targets: [
        .target(name: "OpenClawProtocol", ...),   // 协议定义
        .target(name: "OpenClawKit", ...),        // 核心逻辑
        .target(name: "OpenClawChatUI", ...),     // UI 组件
        .testTarget(name: "OpenClawKitTests", ...),
    ]
)
```

**三层设计**：

1. **OpenClawProtocol** — 纯数据模型和协议定义
   - 与 Gateway 通信的消息类型
   - 配置模型
   - 使用 `StrictConcurrency` 确保线程安全

2. **OpenClawKit** — 业务逻辑和 Gateway 客户端
   - Gateway 连接管理
   - 设备身份管理（`loadOrCreateDeviceIdentity`）
   - 会话状态管理
   - TTS 集成（ElevenLabsKit）
   - 依赖 `OpenClawProtocol`

3. **OpenClawChatUI** — 可复用的 SwiftUI 组件
   - 聊天界面组件
   - Markdown 渲染（Textual 库）
   - 跨 iOS/macOS 共享
   - 依赖 `OpenClawKit`

### 3. 平台特定实现

#### iOS 应用
`apps/ios/` 使用 `xcodegen` 管理项目（`project.yml`）：

```yaml
targets:
  OpenClaw:
    type: application
    platform: iOS
    dependencies:
      - target: OpenClawShareExtension
      - target: OpenClawActivityWidget
      - target: OpenClawWatchApp
      - package: OpenClawKit
      - package: OpenClawKit
        product: OpenClawChatUI
      - package: OpenClawKit
        product: OpenClawProtocol
      - package: Swabble
        product: SwabbleKit
```

**功能模块**：
- **Share Extension** — 系统分享菜单集成
- **Activity Widget** — 主屏幕小部件
- **Watch App** — Apple Watch 配套应用
- **Voice Wake** — 语音唤醒
- **Screen Record** — 屏幕录制分享
- **Camera** — 相机集成
- **Talk Mode** — 语音对话模式

#### Android 应用
`apps/android/` 使用 Gradle 构建：

```kotlin
// settings.gradle.kts
rootProject.name = "OpenClawNodeAndroid"
include(":app")
include(":benchmark")
```

- 通过 JNI/Node.js 嵌入运行 OpenClaw 核心
- 原生 UI 层与 Node.js Gateway 通信
- 支持 benchmark 模块进行性能测试

#### macOS 应用
`apps/macos/` 同样使用 Swift Package Manager：
- 复用 `OpenClawKit` 和 `OpenClawChatUI`
- 支持 MLX TTS（本地语音合成）
- 菜单栏集成
- 原生通知系统

### 4. Gateway 协议层

多平台客户端通过统一的 Gateway 协议与后端通信：

#### 协议版本管理
```typescript
// src/gateway/protocol/index.ts
const PROTOCOL_VERSION = 4;  // 协议版本号
```

#### 通信方式
- **HTTP REST** — `/v1/models`, `/v1/chat/completions` 等 OpenAI-compatible 端点
- **WebSocket** — 实时流式通信
- **Protocol Schema** — `scripts/protocol-gen.ts` 生成 JSON Schema 作为契约
- **Swift 代码生成** — `scripts/protocol-gen-swift.ts` 生成 Swift 模型类

#### 设备发现
- `GatewayDiscoveryService` 支持局域网自动发现 Gateway 实例
- `gateway-dev` 模式支持本地开发调试

### 5. 认证与安全

#### 多模式认证
```typescript
type GatewayCredentialMode =
  | "token"     // Bearer token
  | "password"  // 密码认证
  | "none";     // 无认证（开发模式）
```

#### 设备配对
- `src/pairing/` 模块实现设备配对流程
- 支持 QR 码扫描配对
- `src/gateway/pairing.ts` 处理配对请求

### 6. 配置同步

OpenClaw 通过 Gateway 实现多端配置同步：
- 配置存储在服务端
- 各客户端通过 Gateway API 拉取/推送配置
- 支持配置变更的实时推送

### 7. UI 技术栈

#### 原生 UI
- **iOS/macOS**：SwiftUI（`OpenClawChatUI`）
- **Android**：Jetpack Compose（通过 Android Node 集成）

#### Web UI
- `ui/` 目录包含基于 Lit（Web Components）的 Web 界面
- 支持浏览器内嵌使用

---

## 对 Manta 的借鉴建议

### 短期（协议标准化）

1. **定义 Gateway Protocol Schema**
   - 将 `src/server/` 的 API 定义提取为 JSON Schema 或 OpenAPI 规范
   - 为客户端生成类型安全的绑定代码
   - 使用 `schemars` crate 从 Rust 类型自动生成 JSON Schema

2. **设备身份系统**
   - 实现 `DeviceIdentity` 类型，包含设备 ID、平台、版本
   - 在 SQLite 中持久化设备身份
   - 支持设备配对（复用 `src/security/pairing.rs`）

3. **多认证模式**
   - 扩展 `src/security/` 支持 token/password/none 三种模式
   - 在 Gateway 端实现 `AuthRateLimiter`
   - 参考 OpenClaw 的 `credentials.ts` 实现凭证优先级链

### 中期（共享库设计）

4. **跨平台共享库**
   - 如果未来需要原生客户端，设计共享库层：
     ```
     libs/
     ├── manta-protocol/      # 核心协议类型（可生成 Swift/Kotlin 绑定）
     ├── manta-client/        # Gateway 客户端逻辑
     └── manta-ui-components/ # 可复用 UI 组件
     ```
   - 使用 `uniffi` 从 Rust 生成 Swift/Kotlin FFI 绑定
   - 或者将核心逻辑保持为 Rust，通过 HTTP/WebSocket 与原生 UI 通信

5. **TUI 客户端增强**
   - Manta 已有 CLI，可扩展为功能完整的 TUI：
     - 使用 `ratatui` crate 构建交互式终端界面
     - 支持实时消息流显示
     - 会话管理和切换
     - 配置编辑器
   - 参考 OpenClaw 的 `src/tui/` 实现

6. **Web UI 基础**
   - 在 `src/server/` 中增加静态文件服务
   - 提供基础的 Web 聊天界面
   - 使用 WebSocket 实现实时通信
   - 可参考 OpenClaw 的 `ui/` 目录结构

### 长期（多平台客户端）

7. **移动端策略**
   - **方案 A：Rust 核心 + 原生 UI**
     - 使用 `cargo-mobile` 或 `tauri-mobile` 构建移动端 Rust 核心
     - iOS 使用 SwiftUI，Android 使用 Jetpack Compose
     - 通过 FFI 与 Rust 核心通信

   - **方案 B：WebView 方案**
     - 使用 Tauri 或类似框架打包 Web UI
     - 复用 Web UI 代码
     - 适合快速原型，性能稍差

   - **方案 C：PWA**
     - 将 Web UI 构建为 PWA
     - 支持离线使用
     - 安装到主屏幕

8. **桌面客户端**
   - 使用 Tauri（Rust + WebView）构建桌面应用
   - 复用 Rust 后端代码作为 Tauri 命令
   - 支持 Windows、macOS、Linux
   - 或者使用 `egui`/`iced` 构建纯 Rust GUI

9. **协议代码生成流水线**
   - 建立从 Rust 类型到多语言绑定的自动化流水线：
     ```
     Rust Types → JSON Schema → Swift/Kotlin/TypeScript
     ```
   - 在 CI 中验证协议兼容性
   - 参考 OpenClaw 的 `protocol:gen` 和 `protocol:gen:swift` 脚本

### 架构决策建议

| 决策 | 推荐方案 | 理由 |
|------|----------|------|
| 移动端 | Tauri Mobile 或 React Native + Gateway | 最小化原生代码，复用现有 Rust 后端 |
| 桌面端 | Tauri | Rust 原生集成，无需额外运行时 |
| TUI | ratatui | Rust 生态成熟，与现有代码无缝集成 |
| Web UI | Lit/Vue/React + Axum WebSocket | 灵活选择前端框架 |
| 共享协议 | JSON Schema + uniffi | 标准化、可验证 |

---

## 参考代码位置（OpenClaw）

| 文件/目录 | 职责 |
|-----------|------|
| `apps/ios/project.yml` | iOS 项目配置（xcodegen） |
| `apps/ios/Sources/` | iOS 原生源码 |
| `apps/android/app/build.gradle.kts` | Android 构建配置 |
| `apps/macos/Package.swift` | macOS SPM 配置 |
| `apps/shared/OpenClawKit/Package.swift` | 共享 Swift 框架 |
| `apps/shared/OpenClawKit/Sources/OpenClawProtocol/` | 协议定义 |
| `apps/shared/OpenClawKit/Sources/OpenClawKit/` | 核心逻辑 |
| `apps/shared/OpenClawKit/Sources/OpenClawChatUI/` | UI 组件 |
| `src/gateway/protocol/` | Gateway 协议版本管理 |
| `src/gateway/client.ts` | Gateway 客户端 |
| `src/gateway/call.ts` | Gateway 调用入口 |
| `src/gateway/credentials.ts` | 认证凭证管理 |
| `src/infra/device-identity.ts` | 设备身份管理 |
| `src/pairing/` | 设备配对 |
| `scripts/protocol-gen.ts` | 协议 Schema 生成 |
| `scripts/protocol-gen-swift.ts` | Swift 模型生成 |
| `ui/` | Web UI |
| `src/tui/` | TUI 实现 |
