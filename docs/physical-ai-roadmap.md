# Syscity 转型 Physical AI OS — 能力演进路线图

> 本文档基于当前已实现的能力，梳理 Syscity 从 "具备 OS 控制工具的 AI 助手" 进化为 "Physical AI 操作系统" 所需的核心功能缺口。
>
> 当前已实现：多平台截图（X11/Wayland/Windows/macOS）、桌面控制（点击/输入/窗口管理）、剪贴板操作、PowerShell/Shell 执行、能力集自动检测、多 Agent 协作、向量记忆、MCP 协议、移动端桥接（Android/iOS）、安全审计与 PII 检测、资源配额与沙箱限制。
>
> **实现状态图例**：
> - ✅ **已实现** — 代码已合入主干，可用
> - 🔄 **部分实现** — 核心骨架完成，仍需完善
> - ⬜ **未实现** — 仅有设计或完全缺失

---

## 一、感知层（Perception）— 从"看见屏幕"到"理解界面"

### 1.1 结构化 UI 感知（最高优先级） ✅ 已实现

纯截图方案效率低、成本高、易误判。Agent 需要的是类似浏览器的 DOM 树，而非像素。

| 平台 | 技术方案 | 能力 | 状态 |
|------|---------|------|------|
| macOS | AXUIElement / Accessibility API | 读取控件类型、文本、位置、可点击状态 | ✅ `src/capabilities/macos/accessibility.rs` |
| Windows | UIAutomation / MSAA | 遍历窗口树、获取按钮/输入框属性 | ✅ `src/capabilities/windows/accessibility.rs` |
| Linux X11 | AT-SPI2 | 获取 GTK/Qt 应用的完整控件结构 | ✅ `src/capabilities/linux_desktop_x11/accessibility.rs` |
| Linux Wayland | xdg-desktop-portal + a11y bus | Wayland 安全限制下获取有限 UI 信息 | ✅ `src/capabilities/linux_desktop_wayland/accessibility.rs` |

**核心循环（混合感知）**：
```
1. 获取 UI 树（Accessibility API） → 结构化感知
2. 截图作为补充验证              → 视觉兜底
3. LLM 分析 UI 树 + 截图 → 决策   → 语义推理
4. 通过坐标或控件 ID 执行动作     → 精准操作
5. 再次获取 UI 树验证状态变化     → 结构化验证
```

### 1.2 视觉理解增强

- ✅ **OCR**：从截图中提取不可选中的文本（图片中的文字、游戏 UI）— `src/computer/vision/ocr_rapid.rs`（RapidOCR ONNX，需 `vision` feature）
- ✅ **UI 元素检测**：训练小模型识别按钮、输入框、下拉菜单的位置（作为 Accessibility API 的 fallback）— `src/computer/vision/ui_onnx.rs`（OmniParser ONNX，需 `vision` feature）
- ✅ **屏幕录制/视频流**：FFmpeg 跨平台捕获 + 环形缓冲区 + 场景稳定性检测 — `ScreenRecorder` 已实现（`src/computer/screen_recorder.rs`）
- ✅ **音频捕获**：麦克风输入（cpal）+ VAD + 音频事件分类（错误提示音/通知/语音）— `AudioCapture` 已实现（`src/computer/audio.rs`）

### 1.3 系统状态感知

- ✅ **进程监控**：CPU/内存/磁盘/网络实时数据，检测应用崩溃、资源泄漏 — `SystemMonitor` + `ProcessMonitor`（实时轮询、阈值告警、冷却去重）已实现（`src/computer/system.rs`）
- ✅ **文件系统监控**：watch 关键目录变化，自动响应配置文件修改 — `src/computer/fs_watch.rs`（基于 `notify`  crate）
- ✅ **网络状态**：端口占用、网络连通性、防火墙规则 — `src/computer/network.rs`（`NetworkInspector`：跨平台端口扫描、ICMP ping、TCP 连通性测试、防火墙规则读取）
- ✅ **日志聚合**：实时 tail syslog/journald/Event Viewer/macOS unified log + 告警规则引擎 — `LogAggregator` 已实现（`src/computer/log_aggregator.rs`）

---

## 二、行动层（Action）— 从"点一下"到"完成一件事"

### 2.1 精细输入控制

当前已有点击、输入、按键。需要补充：

- ✅ **鼠标拖拽**：框选文本、拖拽文件、调整滑块 — `DesktopAction::Drag` 已定义，全平台适配器已实现
- ✅ **滚轮/手势**：页面滚动、缩放 — `DesktopAction::Scroll` 已定义，全平台适配器已实现
- ✅ **右键菜单**：context menu 操作 — `Click` + `MouseButton::Right` 已支持
- ✅ **组合键序列**：带延时的复杂快捷键 — `DesktopAction::KeySequence { keys, delays_ms }` 已实现

### 2.2 应用生命周期管理

Agent 需要像人一样"打开软件、等待加载、执行操作、关闭软件"。

- ✅ **启动应用**：支持各种启动方式（双击、命令行、Spotlight/Start Menu）— `LaunchApp` 已实现，`wait_for_ready` 通过 `wait_for(ProcessRunning, 10s)` 实际检测进程就绪（替代固定 sleep）
- ✅ **等待就绪**：检测窗口出现、加载完成 — `WaitCondition::WindowTitleContains` 已实现（headless xdotool 轮询）
- ✅ **进程管理**：kill 卡死进程、重启服务、设置进程优先级 — `KillProcess`/`ListProcesses`/`RestartProcess`/`SetProcessPriority` 均已实现（`src/computer/system.rs` + 各平台适配器）
- ✅ **软件安装**：包管理器调用 — `DesktopAction::InstallPackage { manager, packages, timeout_secs }` 支持 brew/apt/dnf/pacman/apk/winget/choco/macports

### 2.3 浏览器自动化 ✅ 已实现

现代桌面应用大量基于 Web（Electron、PWA、SaaS）。截图+点击的误点击率高。

- ✅ **CDP 浏览器自动化**：基于 `chromiumoxide` 的完整 Chrome DevTools Protocol 集成 — `src/tools/browser.rs` + `src/browser/`
- ✅ **多标签管理**：切换标签页、获取页面标题/URL — `BrowserAction::ListTabs` / `SwitchTab` / `CloseTab`
- ✅ **Cookie/Storage 操作**：登录态管理、本地存储读写 — `GetCookies` / `SetCookie` / `ClearCookies`
- ✅ **下载管理**：监控下载进度、获取下载文件路径 — `SetDownloadBehavior`
- ✅ **ARIA Snapshot**：LLM 友好的可访问树快照 + ref 标记交互 — `src/browser/aria_snapshot.rs`
- ✅ **浏览器会话池**：持久化实例缓存、空闲驱逐、多配置 — `BrowserPool`（`src/browser/pool.rs`）

### 2.4 文件系统代理 ✅ 已实现

- ✅ **智能文件浏览**：ls + 元数据过滤，支持自然语言排序/过滤 — `DesktopAction::BrowseFiles { path, filter_description, max_results }` 已实现
- ✅ **文件内容操作**：大文件分块读取 + 搜索替换 — `DesktopAction::ReadFileChunked` / `EditFile` 已实现
- ✅ **压缩/解压**：zip/tar — `DesktopAction::Compress { sources, destination, format }` / `Decompress` 已实现
- ✅ **跨设备文件传输**：SCP/Rsync — `DesktopAction::TransferFile { source, destination, method }` 已实现（SMB 占位）

### 2.5 工作流录制与回放 ✅ 已实现

- ✅ **动作录制**：记录用户的鼠标/键盘/命令序列 — `WorkflowRecorder` 已实现
- ✅ **参数化回放**：将录制的动作转换为带变量的可复用脚本 — `Workflow::bind_parameters()` 已实现
- ✅ **异常处理**：回放失败时的自动重试、跳过、人工介入 — `WorkflowPlayer` + `FailureStrategy::Abort/Skip/Retry` 已实现

---

## 三、认知层（Cognition）— 从"执行命令"到"解决问题"

### 3.1 目标分解引擎 ✅ 已实现

当前 Agent 主要靠单次 LLM 调用决策。复杂任务需要显式的规划层：

```
用户指令："帮我把这个项目部署到服务器并配置 HTTPS"
          ↓
规划层分解：
  1. 检查本地项目结构（是否含 Dockerfile/docker-compose.yml）
  2. 检查远程服务器状态（SSH 连通性、Docker 是否安装）
  3. 构建并推送镜像 / 上传代码
  4. 远程部署并启动
  5. 配置 Nginx/Caddy + Let's Encrypt
  6. 验证 HTTPS 可用性
  7. 失败时回滚
```

- ✅ **任务依赖图**：有向无环图（DAG）表示子任务依赖关系 — `src/planner/dag.rs`
- ✅ **条件分支**：根据中间结果动态调整后续步骤 — `TaskExecutor` 支持失败分支与回滚
- ✅ **并行执行**：无依赖的子任务并发执行（如同时检查多个服务器）— `DagScheduler::next_ready()` + `max_concurrency`

### 3.2 工具使用推理 ✅ 已实现

- ✅ **工具链推理**：`ToolChainReasoner` — 启发式规则引擎 + LLM fallback，推断先决条件链 (`src/planner/tool_chain.rs`)
- ✅ **工具合成**：`CompositeTool` / `CompositeToolRegistry` — 参数化多步复合工具，内置 git-clone-build / deploy-ssh / install-and-verify (`src/planner/composite_tool.rs`)
- ✅ **工具学习**：`ToolLearningEngine` — 记录工具执行成败，基于上下文相似度推荐替代方案 (`src/planner/tool_learning.rs`)

### 3.3 反思与自纠正

- ✅ **动作验证**：执行后主动验证结果是否符合预期（而非盲目执行下一步）— `VerificationEngine`（`src/computer/verification.rs`）
- ✅ **错误诊断**：`ErrorDiagnosisEngine` — 启发式规则匹配 + LLM fallback，诊断权限/网络/依赖/资源等错误并生成修复策略 (`src/planner/error_diagnosis.rs`)
- ✅ **经验积累**：将成功案例的解决路径存入向量记忆，供未来复用 — `GoalPlanner::decompose` 自动检索历史经验注入 LLM 提示，`achieve` 执行后将结果存入记忆（`src/planner/mod.rs` + `src/memory/`）

### 3.4 长时程任务管理

- ✅ **持久化任务队列**：`TaskStateStore` (SQLite) + `PersistentTaskManager` — 系统重启后自动恢复未完成任务，运行中任务重置为 Pending (`src/planner/state.rs` + `src/planner/persistent_queue.rs`)
- ✅ **定时/周期任务**：`TaskScheduler` + `Schedule` (Once / Interval / Cron) — Cron 的 Agent 化，支持导入导出 (`src/planner/scheduled_tasks.rs`)
- ✅ **中断与恢复**：用户随时打断，Agent 记住上下文，稍后继续 — `ExecutionController` 已集成到 `ComputerUseLoop` 与 `TaskExecutor`

---

## 四、安全层（Safety）— 从"信任"到"可控"

### 4.1 动作沙箱

当前已有审批系统（human-in-the-loop），但缺少事前限制：

- ✅ **路径白名单**：Agent 只能读写指定目录（如 ~/Projects/），禁止访问 ~/.ssh/、/etc/ — `SandboxInterceptor::path_allowlist`（`src/tools/sandbox_interceptor.rs`）
- ✅ **网络沙箱**：限制可访问的域名/IP 范围，禁止访问内网敏感服务 — `domain_allowlist` + `ip_allowlist`/`ip_blocklist`（CIDR IPv4/IPv6 支持）已实现（`src/tools/sandbox_interceptor.rs`）
- ✅ **命令黑名单**：禁止 rm -rf /、format、fdisk 等危险命令 — `SandboxInterceptor::command_blacklist`
- ✅ **资源配额**：限制 CPU/内存/FD/进程数，防止 runaway agent — `ToolContext` (memory_limit / cpu_limit / fd_limit / process_limit)，通过 `setrlimit` 在沙箱 shell 执行前生效（`src/tools/mod.rs`）


### 4.2 自动回滚

- ✅ **系统快照**：在执行高风险操作前创建还原点
  - 文件层面：备份原文件再修改 — `RollbackManager::snapshot_file()` 已实现（`src/computer/rollback.rs`）
  - 系统层面：APFS snapshot（macOS）、System Restore（Windows）、Btrfs snapshot（Linux）均已实现，失败时优雅降级到目录备份
- ✅ **失败检测**：超时、非零退出码、异常截图变化 → 触发回滚 — `VerificationEngine` 与 `TaskExecutor` 已支持验证失败触发回滚
- ✅ **逐步提交**：将多步操作设计为可逆的，支持单步 undo — `RollbackManager::rollback_last()`（`src/computer/rollback.rs`）

### 4.3 敏感操作自动识别 ✅ 已实现

- ✅ **模式匹配**：检测到密码输入框、支付页面、删除确认对话框时自动暂停 — `SandboxInterceptor` 结合 `ContentFilter`
- ✅ **内容审查**：截图中检测到身份证号、银行卡、API Key 时打码/告警 — `PiiDetector` + `SecretScanner`（`src/security/pii.rs`）、`ContentFilter`（`src/security/content_filter.rs`）
- ✅ **操作审计**：所有动作记录不可篡改日志（含截图、命令、时间戳）— `SecurityAuditReport`（`src/security/audit.rs`）

---

## 五、Physical AI 核心抽象（跨平台统一层）

### 5.1 统一桌面抽象 ✅ 已实现

Agent 不应关心底层是 X11 还是 Wayland 还是 Windows。

```rust
// Agent 代码中只看到统一的 Desktop 接口
desktop.screenshot()       // 不关心背后是 maim/scrot/PowerShell
desktop.click(element)     // 不关心背后是 xdotool/SendKeys
desktop.type_text("hello") // 自动处理输入法、特殊字符转义
desktop.read_ui_tree()     // 统一返回控件树结构
```

实现方式：在 CapabilitySet 之上再封装一层 `PhysicalAiAdapter`，将各平台差异隐藏。

- ✅ `ComputerAdapter` trait（`src/computer/mod.rs`）— 跨平台统一接口，含 screenshot / click / type / read_ui_tree / execute / wait_for
- ✅ `create_adapter()` 工厂函数 — 自动检测平台并创建对应适配器

### 5.2 Computer-Use 标准 API ✅ 已实现

对齐 Anthropic Computer Use 的交互范式：

```python
# 标准化循环
while not task_done:
    screenshot = desktop.screenshot()        # 感知
    action = llm.decide(screenshot, goal)    # 决策
    desktop.execute(action)                  # 行动
    result = desktop.verify(action)          # 验证
```

- ✅ **标准化循环** — `ComputerUseLoop`（`src/computer/use_loop.rs`）：screenshot → decide → execute → verify
- ✅ **坐标系统统一**：不同平台 DPI、缩放比例不同，Agent 使用逻辑坐标 — `Point`/`Rect` 已统一，`DpiScale::detect()` 支持 macOS（`system_profiler`）/Windows（PowerShell）/Linux（`xdpyinfo`/`gsettings`），`to_physical`/`to_logical` 转换已就绪（`src/computer/types.rs`）
- ✅ **截图编码优化**：根据网络状况自动调整分辨率/质量 — `ScreenshotEncoder` + `NetworkCondition::auto_detect()`（ping RTT 自动判断 Local/Normal/Remote），全平台（macOS/Linux/Windows/Headless/Remote）均已接入
- ✅ **延迟补偿**：操作后自动等待动画完成，避免在过渡态截图 — `LoopConfig::settle_delay_ms` + 自适应延时（连续失败时自动加倍）

### 5.3 无头模式（Headless） ✅ 已实现

- ✅ **虚拟显示器**：Linux（Xvfb）、macOS（ Quartz 虚拟屏）、Windows（RDP 会话）— `HeadlessComputerAdapter` 已支持 Xvfb（Linux），其他平台 fallback 到无 GUI 模式
- ✅ **CI/CD 集成**：在 GitHub Actions 等无 GUI 环境中运行桌面自动化测试
- ✅ **远程控制**：SSH 远程桌面控制 — `RemoteControlAdapter` 已实现（Linux/macOS/Windows 三平台截图/点击/输入/剪贴板/文件操作），VNC/RDP 帧缓冲协议预留为未来扩展

---

## 六、扩展层 — 从"电脑"到"物理世界"

### 6.1 移动端桥接 ✅ 已实现

| 平台 | 连接方式 | 能力 | 状态 |
|------|---------|------|------|
| Android | ADB | 截图、点击、滑动、输入、按键事件、应用安装/启动/强制停止、UI 树转储 | ✅ `src/capabilities/mobile/android.rs` |
| iOS | libimobiledevice | 设备列表、截图、应用管理 | ✅ `src/capabilities/mobile/ios.rs` |

### 6.2 嵌入式与物联网 ⬜ 未实现

- ⬜ **树莓派 / Jetson**：GPIO 控制、摄像头输入、传感器读取
- ⬜ **Home Assistant 集成**：控制智能家居设备
- ⬜ **串口/USB 设备**：与 Arduino、PLC、示波器等硬件通信

### 6.3 机器人接口 ⬜ 未实现

- ⬜ **ROS2 桥接**：接收激光雷达/摄像头数据，输出移动/抓取指令
- ⬜ **机械臂控制**：通过 SDK 发送关节角度/末端位姿

---

## 七、演进优先级

### Phase 1：基础可用（1-2 个月）
1. ✅ **Accessibility API 集成** — macOS AXUIElement、Windows UIAutomation、Linux AT-SPI2
2. ✅ **统一桌面抽象层** — 隐藏平台差异的 `ComputerAdapter`
3. ✅ **浏览器自动化** — CDP (chromiumoxide) 集成，覆盖导航/点击/输入/截图/标签管理/Cookie/下载
4. ✅ **动作验证循环** — 执行后自动验证结果，失败重试 — `VerificationEngine` + `ComputerUseLoop`

### Phase 2：生产就绪（2-4 个月）
5. ✅ **路径/网络沙箱** — 事前限制，降低审批频率 — 路径白名单 + 命令黑名单 + IP 段限制（CIDR）+ 资源配额（memory/cpu/fd/process limit）已完成
6. ✅ **自动回滚** — 文件备份 + 系统快照 — 文件级 `RollbackManager` + 系统级快照（APFS/Btrfs/System Restore）+ 单步 undo（`rollback_last`）已完成
7. ✅ **长时程任务管理** — 持久化队列、中断恢复 — `TaskStateStore` + `PersistentTaskManager` + `TaskScheduler` 已完成
8. ✅ **目标分解引擎** — 复杂任务自动拆解为 DAG — `GoalPlanner` + `DagScheduler` + `TaskExecutor`

### Phase 3：生态扩展（4-6 个月）
9. ✅ **移动端桥接** — Android/iOS 控制 — `AndroidSet`（ADB）+ `IosSet`（libimobiledevice）
10. ✅ **无头模式** — CI/CD、远程服务器自动化 — `HeadlessComputerAdapter` + Xvfb
11. ✅ **工作流录制回放** — 用户示范 → Agent 学习 — `WorkflowRecorder` + `WorkflowPlayer` + `FailureStrategy` 已实现
12. ✅ **VLM 微调** — 针对桌面 UI 场景优化的小模型 — OmniParser ONNX 已集成（`vision` feature）

---

## 八、与现有架构的关系

```
┌─────────────────────────────────────────────────────────┐
│  Agent 层（已有）                                         │
│  - 多 Agent 协作、向量记忆、MCP、审批系统                   │
├─────────────────────────────────────────────────────────┤
│  规划层（✅ 已实现）                                       │
│  - 目标分解引擎（GoalPlanner）、任务 DAG、长时程管理         │
│  - 持久化任务队列（TaskStateStore + PersistentTaskManager）  │
├─────────────────────────────────────────────────────────┤
│  抽象层（✅ 已实现）←── 本路线图核心                        │
│  - ComputerAdapter（跨平台统一桌面接口）                    │
│  - ComputerUseLoop（截图-决策-执行-验证循环）               │
├─────────────────────────────────────────────────────────┤
│  能力层（已实现）                                         │
│  - CapabilitySet：X11 / Wayland / Windows / macOS / Linux  │
├─────────────────────────────────────────────────────────┤
│  平台层（已实现）                                         │
│  - xdotool / xclip / PowerShell / AXUIElement ...          │
└─────────────────────────────────────────────────────────┘
```

当前实现的 **CapabilitySet + Tool** 架构已经为上述演进打下了良好基础。下一步的核心工作是在 Tool 之上构建 **PhysicalAiAdapter** 统一抽象，以及 **Planner** 规划层，从而让 Syscity 从一个"能控制 OS 的 AI 工具"真正进化为"Physical AI 操作系统"。

---

## 九、具体实现路径

> 基于当前 `CapabilitySet + ToolRegistry` 架构，给出每个功能的代码位置、模块依赖、实现顺序。
>
> 当前架构基线：
> - `src/capabilities/` — 平台能力集（X11/Wayland/Windows/macOS/Linux Server）
> - `src/tools/` — Tool trait + ToolRegistry（缓存/熔断/审批/沙箱已就绪）
> - `src/tools/ToolContext` — 已含 workspace、sandbox、resource limit、skill trust
> - macOS 已有 `AccessibilityTool`（AppleScript/System Events）

### 9.1 架构分层（目标态）

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 4: Agent / Planner（✅ 已实现）                        │
│  - GoalPlanner: 目标分解、DAG 调度、长时程任务                │
│  - ComputerUseLoop: 截图→决策→执行→验证 循环                │
│  - 持久化任务队列（TaskScheduler / Schedule）                  │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: ComputerAdapter（✅ 已实现）                        │
│  - 跨平台统一接口：screenshot / click / type / read_ui_tree  │
│  - 隐藏 X11/Wayland/Windows/macOS 差异                       │
│  - 坐标系统统一（逻辑坐标，自动处理 DPI 缩放）                │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: CapabilitySet + ToolRegistry（已有，扩展）          │
│  - ✅ 各平台 Accessibility Tool                              │
│  - ✅ BrowserAutomation Tool（chromiumoxide CDP）           │
│  - ✅ ApplicationLifecycle Tool（LaunchApp/KillProcess/RestartProcess/SetProcessPriority）│
├─────────────────────────────────────────────────────────────┤
│  Layer 1: 平台原生实现（已有，扩展）                          │
│  - macOS: AXUIElement（已有 AppleScript 基础）                │
│  - Windows: UIAutomation / MSAA                             │
│  - Linux X11: AT-SPI2                                       │
│  - Linux Wayland: xdg-desktop-portal（有限支持）              │
└─────────────────────────────────────────────────────────────┘
```

---

> **注**：本节中提出的 `PhysicalAiAdapter` 设计已在 `src/computer/` 中实现为 [`ComputerAdapter`](src/computer/mod.rs) trait，包含 `screenshot` / `click` / `type` / `read_ui_tree` / `execute` / `wait_for` 等方法，以及 `DesktopAction` 统一动作枚举、`Point`/`Rect` 逻辑坐标和 `DpiScale` DPI 检测。以下代码为原始设计参考，实际实现路径为 `src/computer/`。

### 9.2 Phase 1: 基础可用（核心骨架，1-2 个月）

#### 1.1 跨平台统一桌面抽象 — `PhysicalAiAdapter`（已实现为 `ComputerAdapter`）

**目标**：Agent 代码中不再出现 `linux_x11_desktop_control`、`windows_desktop_control` 等平台差异。

**新增模块**：

```
src/physical_ai/
├── mod.rs          # PhysicalAiAdapter trait + 统一数据结构
├── adapter.rs      # 平台检测 + 适配器创建
├── types.rs        # 跨平台通用类型：UiElement, Point, Rect, Action
└── verification.rs # 动作执行后的自动验证
```

**核心设计**：

```rust
// src/physical_ai/types.rs
pub struct UiElement {
    pub id: String,           // 平台相关的唯一标识
    pub role: String,         // "button", "text_field", "window"...
    pub label: Option<String>,
    pub value: Option<String>,
    pub bounds: Rect,
    pub enabled: bool,
    pub focused: bool,
    pub children: Vec<UiElement>,
}

pub enum DesktopAction {
    Screenshot { region: Option<Rect> },
    Click { target: ClickTarget, button: MouseButton },
    Type { text: String },
    KeyPress { keys: Vec<String> },
    Scroll { direction: ScrollDirection, amount: i32 },
    Drag { from: Point, to: Point },
    ReadUiTree { app: Option<String> },
    LaunchApp { name: String, args: Vec<String>, wait_for_ready: bool },
    ActivateWindow { title_pattern: String },
}

pub enum ClickTarget {
    Coordinate(Point),      // 绝对坐标
    ElementId(String),      // Accessibility ID
    ElementLabel(String),   // 按标签名查找
}
```

```rust
// src/physical_ai/mod.rs
#[async_trait]
pub trait PhysicalAiAdapter: Send + Sync {
    /// 获取当前屏幕截图（base64 编码）
    async fn screenshot(&self, region: Option<Rect>) -> Result<Screenshot>;

    /// 获取当前活跃窗口的 UI 树
    async fn read_ui_tree(&self) -> Result<Vec<UiElement>>;

    /// 执行桌面动作
    async fn execute(&self, action: DesktopAction) -> Result<ActionResult>;

    /// 等待条件满足（用于动作后验证）
    async fn wait_for(&self, condition: WaitCondition, timeout: Duration) -> Result<bool>;
}

/// 工厂函数：根据当前平台创建合适的适配器
pub fn create_adapter() -> Result<Box<dyn PhysicalAiAdapter>> {
    #[cfg(target_os = "macos")]
    return Ok(Box::new(MacosPhysicalAdapter::new()));
    #[cfg(target_os = "windows")]
    return Ok(Box::new(WindowsPhysicalAdapter::new()));
    #[cfg(target_os = "linux")]
    {
        if has_wayland() {
            Ok(Box::new(WaylandPhysicalAdapter::new()))
        } else if has_x11() {
            Ok(Box::new(X11PhysicalAdapter::new()))
        } else {
            Ok(Box::new(HeadlessPhysicalAdapter::new()))
        }
    }
}
```

**平台适配器实现**：

```
src/physical_ai/platform/
├── macos.rs      # MacosPhysicalAdapter：包装 macOS CapabilitySet 的工具
├── windows.rs    # WindowsPhysicalAdapter：包装 Windows CapabilitySet
├── x11.rs        # X11PhysicalAdapter：包装 Linux X11 CapabilitySet
├── wayland.rs    # WaylandPhysicalAdapter：包装 Linux Wayland CapabilitySet
└── headless.rs   # HeadlessPhysicalAdapter：仅 shell/文件操作，无 GUI
```

每个适配器内部持有 `ToolRegistry` 的引用，将 `DesktopAction` 翻译为对应平台工具的 `execute()` 调用：

```rust
// src/physical_ai/platform/macos.rs
impl PhysicalAiAdapter for MacosPhysicalAdapter {
    async fn screenshot(&self, region: Option<Rect>) -> Result<Screenshot> {
        let args = if let Some(r) = region {
            json!({ "region": { "x": r.x, "y": r.y, "width": r.w, "height": r.h } })
        } else {
            json!({})
        };
        let result = self.registry.execute("macos_screenshot", args, &self.context).await;
        // 解析 ToolExecutionResult → Screenshot 结构体
    }

    async fn read_ui_tree(&self) -> Result<Vec<UiElement>> {
        let result = self.registry.execute("macos_accessibility", json!({"action":"tree"}), &self.context).await;
        // 解析 AppleScript 输出 → UiElement 树
    }

    async fn execute(&self, action: DesktopAction) -> Result<ActionResult> {
        match action {
            DesktopAction::Click { target, button } => {
                match target {
                    ClickTarget::Coordinate(p) => {
                        // 调用 macos_desktop_control click
                    }
                    ClickTarget::ElementId(id) => {
                        // 调用 AXUIElement 通过 ID 点击（更精准）
                    }
                    // ...
                }
            }
            // ... 其他 action
        }
    }
}
```

**关键决策**：
- `PhysicalAiAdapter` **不替代** `Tool`，而是**包装** `Tool`。LLM 仍然通过 `ToolRegistry` 调用原子工具；`PhysicalAiAdapter` 供上层 Planner 使用。
- 坐标统一：所有坐标使用逻辑坐标（0-1920 范围），适配器内部转换为平台实际像素（处理 Retina/HiDPI 缩放）。

---

#### 1.2 Accessibility API 补齐 ✅ 已实现

**现状**：macOS/Windows/Linux X11/Linux Wayland 均已实现结构化 UI 树获取能力。

##### Windows — `src/capabilities/windows/accessibility.rs`

使用 PowerShell + UIAutomation：

```powershell
# 获取前台窗口的 UI 树
Add-Type -AssemblyName UIAutomationClient
$uia = [System.Windows.Automation.AutomationElement]::RootElement
$cond = [System.Windows.Automation.ControlTypeCondition]::Window
$win = $uia.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
# 递归遍历子元素，输出 JSON
```

实现方式：在 `WindowsSet` 中新增 `AccessibilityTool`，通过 PowerShell 脚本获取 UIAutomation 树，输出 JSON 后解析为 `UiElement` 结构。

##### Linux X11 — `src/capabilities/linux_desktop_x11/accessibility.rs`

使用 `atspi2`（Assistive Technology Service Provider Interface）：

```bash
# 通过 dbus 调用 AT-SPI 获取当前焦点窗口的 UI 树
python3 -c "
import pyatspi
app = pyatspi.Registry.getDesktop(0)
# 遍历获取当前活跃应用的控件树
"
```

如果 `pyatspi` 不可用，fallback 到 `xdotool` + `xwininfo` 获取有限信息（窗口标题、尺寸、位置）。

##### Linux Wayland — `src/capabilities/linux_desktop_wayland/accessibility.rs`

Wayland 安全模型限制严格，无法直接获取其他应用的 UI 树。方案：

1. **xdg-desktop-portal + a11y bus**：如果 compositor（如 GNOME/Mutter）支持 a11y portal，通过 D-Bus 获取。
2. **Fallback**：仅支持当前应用自身的 UI 树（自洽操作）。
3. **无 UI 树时的策略**：纯截图 + OCR + 坐标点击（退化为 VLM 方案）。

**代码位置**：

```
src/capabilities/windows/accessibility.rs      # 新增
src/capabilities/linux_desktop_x11/accessibility.rs  # 新增
src/capabilities/linux_desktop_wayland/accessibility.rs  # 新增（有限支持）
```

每个平台 `mod.rs` 中注册新的 `AccessibilityTool`。

---

#### 1.3 动作验证循环 — `VerificationEngine` ✅ 已实现

**目标**：执行动作后自动验证结果，失败时重试或回滚。

**模块位置**：

```
src/computer/
└── verification.rs      # VerificationEngine + VerificationCriteria
```

```rust
pub struct VerificationEngine {
    adapter: Arc<dyn PhysicalAiAdapter>,
}

impl VerificationEngine {
    /// 执行动作并验证
    pub async fn execute_with_verification(
        &self,
        action: DesktopAction,
        expected: VerificationCriteria,
        max_retries: u32,
    ) -> Result<ActionResult> {
        for attempt in 0..=max_retries {
            let result = self.adapter.execute(action.clone()).await?;

            // 等待状态稳定（动画完成）
            tokio::time::sleep(Duration::from_millis(500)).await;

            // 验证
            if self.verify(&expected).await? {
                return Ok(result);
            }

            if attempt < max_retries {
                tracing::warn!("Verification failed, retrying {}/{}...", attempt + 1, max_retries);
                // 可选：自适应调整（如坐标偏移重试）
            }
        }

        Err(Error::VerificationFailed)
    }

    async fn verify(&self, criteria: &VerificationCriteria) -> Result<bool> {
        match criteria {
            VerificationCriteria::UiTreeContains { role, label } => {
                let tree = self.adapter.read_ui_tree().await?;
                Ok(tree.iter().any(|e| e.role == *role && e.label.as_ref() == Some(label)))
            }
            VerificationCriteria::ScreenshotDiff { max_pixel_diff } => {
                let before = self.before_screenshot.as_ref().ok_or(Error::NoBaseline)?;
                let after = self.adapter.screenshot(None).await?;
                Ok(compute_diff(before, &after) <= *max_pixel_diff)
            }
            VerificationCriteria::ProcessRunning { name } => {
                // 检查进程是否存在
            }
            VerificationCriteria::WindowTitleContains { pattern } => {
                // 检查窗口标题
            }
        }
    }
}
```

---

#### 1.4 浏览器自动化 — `BrowserAutomationTool`

**目标**：覆盖现代桌面中大量基于 Web 的应用（Electron、PWA、SaaS）。

**方案**：集成 Playwright（或依赖现有的 `BrowserTool` 扩展）。

**代码位置**：

```
src/tools/browser.rs  # 扩展现有 BrowserTool
```

现有 `BrowserTool` 可能只支持网页浏览。需要增加桌面级能力：

```rust
// 新增操作
pub enum BrowserAction {
    Navigate { url: String },
    Click { selector: String },
    Type { selector: String, text: String },
    Screenshot { full_page: bool },
    GetDom,                    // 获取完整 DOM
    GetElementText { selector: String },
    ExecuteJs { script: String },
    WaitForSelector { selector: String, timeout: u64 },
    Download { url: String, path: String },
    HandleDialog { action: DialogAction },
    // 新增：多标签管理
    ListTabs,
    SwitchTab { index: usize },
    CloseTab { index: usize },
    // 新增：桌面集成
    LaunchBrowser { browser: String, url: Option<String> },  // "chrome", "firefox", "edge"
    GetBrowserWindowInfo,  // 获取浏览器窗口位置和尺寸
}
```

**实现方式**：
1. 依赖 `playwright` Python 包或 `chromiumoxide` Rust crate
2. 如果系统未安装 Playwright，自动下载安装（类似 `npx playwright install`）
3. 对于已打开的浏览器实例，通过 CDP（Chrome DevTools Protocol）连接

---

### 9.3 Phase 2: 生产就绪（安全 + 规划，2-4 个月）

#### 2.1 路径/网络/命令沙箱强化 ✅ 已实现

**现状**：`SandboxInterceptor`（`src/tools/sandbox_interceptor.rs`）已实现统一拦截，在 `ToolRegistry::execute()` 的 policy hook 中执行。

```rust
// src/tools/sandbox_interceptor.rs

pub struct SandboxInterceptor {
    command_blacklist: Vec<Regex>,      // ✅ 已实现
    path_blacklist: Vec<Regex>,         // ✅ 已实现
    path_allowlist: Vec<PathBuf>,       // ✅ 已实现
    domain_allowlist: Vec<String>,      // ✅ 已实现
}

impl SandboxInterceptor {
    pub fn check_tool_call(&self, name: &str, args: &Value) -> Result<(), SandboxError> {
        // 1. 检查命令黑名单
        if name == "shell" {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let base_cmd = cmd.split_whitespace().next().unwrap_or("");
            if self.command_blacklist.contains(base_cmd) {
                return Err(SandboxError::CommandBlocked(base_cmd.to_string()));
            }
        }

        // 2. 检查路径越界
        if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            for pattern in &self.path_blacklist {
                if pattern.is_match(path) {
                    return Err(SandboxError::PathBlocked(path.to_string()));
                }
            }
        }

        Ok(())
    }
}

impl SandboxInterceptor {
    pub fn check_tool_call(&self, name: &str, args: &Value) -> Result<(), SandboxError> {
        // 1. 检查命令黑名单
        if name == "shell" {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let base_cmd = cmd.split_whitespace().next().unwrap_or("");
            if self.command_blacklist.contains(base_cmd) {
                return Err(SandboxError::CommandBlocked(base_cmd.to_string()));
            }
        }

        // 2. 检查路径越界
        if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            for pattern in &self.path_blacklist {
                if pattern.is_match(path) {
                    return Err(SandboxError::PathBlocked(path.to_string()));
                }
            }
        }

        // 3. 敏感内容检测（截图中的个人信息、API Key）
        if name.contains("screenshot") {
            // 标记为需要后续处理（OCR 审查）
        }

        Ok(())
    }
}
```

**集成到 ToolRegistry**：在 `execute()` 的 policy hook 之后、before-hook 之前插入沙箱检查。

---

#### 2.2 自动回滚 — `RollbackManager` ✅ 已实现

**模块位置**：

```
src/computer/
└── rollback.rs          # 文件级 + 系统级 RollbackManager（文件备份 + 系统快照 + 单步 undo）
```

```rust
pub struct RollbackManager {
    snapshots: Vec<Snapshot>,
}

pub enum Snapshot {
    FileBackup { original_path: PathBuf, backup_path: PathBuf },
    DirectoryBackup { original_path: PathBuf, backup_path: PathBuf },
    // 系统级快照（平台相关）
    #[cfg(target_os = "macos")]
    ApfsSnapshot { path: PathBuf, snapshot_name: String },
    #[cfg(target_os = "linux")]
    BtrfsSnapshot { subvolume: PathBuf, snapshot_path: PathBuf },
}

impl RollbackManager {
    /// 在执行高风险操作前创建快照
    pub async fn snapshot_file(&mut self, path: &Path) -> Result<()> {
        let backup = self.backup_dir.join(format!("{}.bak.{}", path.file_name().unwrap().to_string_lossy(), uuid::Uuid::new_v4()));
        tokio::fs::copy(path, &backup).await?;
        self.snapshots.push(Snapshot::FileBackup { original_path: path.to_path_buf(), backup_path: backup });
        Ok(())
    }

    /// 操作失败时回滚
    pub async fn rollback(&self) -> Result<()> {
        for snapshot in self.snapshots.iter().rev() {
            match snapshot {
                Snapshot::FileBackup { original_path, backup_path } => {
                    tokio::fs::copy(backup_path, original_path).await?;
                }
                // ...
            }
        }
        Ok(())
    }
}
```

**触发条件**：
- Tool 返回 error
- VerificationEngine 验证失败且重试用尽
- 用户手动触发 `syscity rollback`

---

#### 2.3 目标分解引擎 — `GoalPlanner` ✅ 已实现

**模块位置**：

```
src/planner/
├── mod.rs          # ✅ Plan, Task, GoalPlanner
├── dag.rs          # ✅ DagScheduler — 有向无环图任务调度与拓扑排序
├── decomposer.rs   # ✅ LLM-based 目标分解
├── executor.rs     # ✅ TaskExecutor — 任务执行引擎（并发 + 回滚 + ExecutionController）
├── state.rs        # ✅ TaskStateStore — SQLite 持久化、崩溃恢复、进度统计
└── verifier.rs     # ✅ 子任务完成验证（已融入 verification.rs）
```

**核心设计**：

```rust
// src/planner/mod.rs
pub struct GoalPlanner {
    llm: Arc<dyn LlmProvider>,
    tool_registry: Arc<ToolRegistry>,
    physical_adapter: Arc<dyn PhysicalAiAdapter>,
    memory: Arc<dyn VectorMemory>,
}

impl GoalPlanner {
    /// 主入口：接收用户目标，分解为任务 DAG，执行
    pub async fn achieve(&self, goal: &str, context: &TaskContext) -> Result<ExecutionResult> {
        // 1. 分解目标
        let tasks = self.decompose(goal).await?;

        // 2. 构建 DAG
        let dag = TaskDag::from_tasks(tasks)?;

        // 3. 拓扑排序并行执行
        let result = self.execute_dag(&dag, context).await?;

        // 4. 记录经验
        self.record_experience(goal, &result).await?;

        Ok(result)
    }

    async fn decompose(&self, goal: &str) -> Result<Vec<SubTask>> {
        // 调用 LLM 分解
        let prompt = format!(
            r#"将以下目标分解为可执行的子任务序列。每个子任务包含：
- id: 唯一标识
- description: 任务描述
- dependencies: 依赖的前置任务 id 列表
- tool_hint: 建议使用的工具类型
- verification: 如何验证任务完成

目标：{}

可用工具：{}
"#,
            goal,
            self.tool_registry.list().join(", ")
        );

        let response = self.llm.complete(&prompt).await?;
        parse_subtasks(&response)
    }
}

pub struct SubTask {
    pub id: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub tool_hint: ToolHint,
    pub verification: VerificationCriteria,
    pub max_retries: u32,
}

pub enum ToolHint {
    DesktopAction(DesktopAction),
    ShellCommand(String),
    BrowserAction(BrowserAction),
    FileOperation(FileOp),
    // LLM 自主选择
    Auto,
}
```

**DAG 执行器**：

```rust
// src/planner/dag.rs
pub struct TaskDag {
    tasks: HashMap<String, SubTask>,
    // 邻接表：task_id -> [dependent_task_ids]
    dependents: HashMap<String, Vec<String>>,
    // 入度表
    in_degree: HashMap<String, usize>,
}

impl TaskDag {
    pub async fn execute_parallel(&self, executor: &TaskExecutor) -> Result<()> {
        let mut completed = HashSet::new();
        let mut in_progress = FuturesUnordered::new();

        loop {
            // 找出所有入度为 0 且未完成的 task
            let ready: Vec<String> = self.in_degree
                .iter()
                .filter(|(id, degree)| **degree == 0 && !completed.contains(*id))
                .map(|(id, _)| id.clone())
                .collect();

            for task_id in ready {
                let task = self.tasks[&task_id].clone();
                in_progress.push(executor.execute(task));
            }

            // 等待任意一个完成
            if let Some(result) = in_progress.next().await {
                let (task_id, outcome) = result?;
                completed.insert(task_id.clone());

                // 减少依赖者的入度
                if let Some(deps) = self.dependents.get(&task_id) {
                    for dep in deps {
                        *self.in_degree.get_mut(dep).unwrap() -= 1;
                    }
                }
            } else if completed.len() == self.tasks.len() {
                break Ok(());
            } else {
                // 有任务未执行但 in_progress 为空 → 循环依赖
                return Err(Error::CyclicDependency);
            }
        }
    }
}
```

---

#### 2.4 长时程任务管理

**新增模块**：

```
src/planner/
└── state.rs
```

```rust
pub struct TaskStateManager {
    storage: Arc<dyn TaskStorage>,  // SQLite / 文件存储
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentTask {
    pub id: String,
    pub goal: String,
    pub status: TaskStatus,
    pub dag: TaskDag,           // 序列化后的 DAG
    pub completed_tasks: Vec<String>,
    pub failed_tasks: Vec<(String, String)>,  // (task_id, error)
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum TaskStatus {
    Pending,
    Running,
    Paused,      // 用户中断
    Completed,
    Failed,
}
```

**持久化策略**：
- 使用 SQLite 表 `persistent_tasks` 存储任务状态
- 系统启动时加载未完成的任务，询问用户是否恢复
- 每个子任务完成后立即 commit，崩溃后可恢复

---

### 9.4 Phase 3: 生态扩展（4-6 个月）

#### 3.1 无头模式 — `HeadlessComputerAdapter` ✅ 已实现

**代码位置**：`src/computer/headless.rs`

```rust
/// 无 GUI 环境的适配器（服务器、CI/CD）
pub struct HeadlessPhysicalAdapter {
    // 虚拟显示器
    virtual_display: Option<VirtualDisplay>,
}

impl HeadlessPhysicalAdapter {
    pub async fn new() -> Result<Self> {
        // Linux: 启动 Xvfb
        // macOS: 创建虚拟显示器（ Quartz Display Services）
        // Windows: 创建 RDP 会话
        let display = Self::create_virtual_display().await?;
        Ok(Self { virtual_display: Some(display) })
    }
}

impl PhysicalAiAdapter for HeadlessPhysicalAdapter {
    async fn screenshot(&self, region: Option<Rect>) -> Result<Screenshot> {
        // 从虚拟显示器捕获
        self.virtual_display.as_ref().unwrap().capture(region).await
    }
    // ... 其他方法委托给底层平台适配器
}
```

---

#### 3.2 移动端桥接

**新增模块**：

```
src/capabilities/mobile/
├── mod.rs
├── android.rs      # ADB 集成
└── ios.rs          # instruments / WebDriverAgent
```

```rust
// Android via ADB
pub struct AndroidSet;

impl CapabilitySet for AndroidSet {
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(AdbScreenshotTool::new()),
            Box::new(AdbUiTreeTool::new()),      // uiautomator dump
            Box::new(AdbInputTool::new()),       // tap / type / key
            Box::new(AdbAppManagerTool::new()),  // install / launch / force-stop
        ]
    }
}
```

---

#### 3.3 工作流录制回放 — `WorkflowRecorder`

**新增模块**：

```
src/planner/
└── workflow.rs      # ✅ WorkflowRecorder + WorkflowPlayer + FailureStrategy
```

```rust
pub struct WorkflowRecorder {
    actions: Vec<RecordedAction>,
    recording: bool,
}

pub enum RecordedAction {
    Desktop(DesktopAction, Duration),  // 动作 + 距上一动作的延迟
    Shell(String),
    Wait(Duration),
}

impl WorkflowRecorder {
    pub fn start_recording(&mut self) {
        self.recording = true;
        self.actions.clear();
    }

    pub fn record(&mut self, action: DesktopAction) {
        if self.recording {
            let delay = self.last_action_time.elapsed();
            self.actions.push(RecordedAction::Desktop(action, delay));
            self.last_action_time = Instant::now();
        }
    }

    pub fn to_workflow(&self) -> Workflow {
        Workflow {
            name: "recorded_workflow".to_string(),
            actions: self.actions.clone(),
            parameters: self.infer_parameters(),  // 将硬编码值转为变量
        }
    }
}
```

---

### 9.5 模块依赖图

```
src/planner/
  ├── 依赖 src/computer/（执行桌面动作）
  ├── 依赖 src/tools/（调用原子工具）
  ├── 依赖 src/providers/（LLM 调用）
  └── 依赖 src/memory/（经验存储）

src/computer/
  ├── 依赖 src/capabilities/（平台能力集）
  ├── 依赖 src/tools/（ToolRegistry 执行）
  └── 不依赖 src/planner/（单向依赖）

src/capabilities/
  ├── 各平台模块互不依赖
  ├── 统一依赖 src/tools/（Tool trait）
  └── 新增 accessibility 模块不影响现有 screenshot/desktop_control
```

---

### 9.6 实现顺序建议

| 周次 | 任务 | 产出 | 影响范围 |
|------|------|------|----------|
| 周次 | 任务 | 产出 | 状态 |
|------|------|------|------|
| W1-2 | `ComputerAdapter` trait + types | `src/computer/mod.rs` + `src/computer/types.rs` | ✅ 已实现（原设计为 `PhysicalAiAdapter`） |
| W3-4 | macOS/Windows/X11 `ComputerAdapter` 实现 | `src/computer/platform_*.rs` | ✅ 已实现 |
| W5-6 | Windows AccessibilityTool | `src/capabilities/windows/accessibility.rs` | ✅ 已实现 |
| W7-8 | Linux X11 AccessibilityTool | `src/capabilities/linux_desktop_x11/accessibility.rs` | ✅ 已实现 |
| W9-10 | `VerificationEngine` | `src/computer/verification.rs` | ✅ 已实现 |
| W11-12 | BrowserAutomation 扩展 | `src/tools/browser.rs` + `src/browser/` | ✅ 已完整实现（chromiumoxide CDP） |
| W13-16 | `GoalPlanner` + DAG 执行器 | `src/planner/` | ✅ 已实现 |
| W17-18 | SandboxInterceptor 强化 | `src/tools/sandbox_interceptor.rs` | ✅ 已实现（路径/网络/命令） |
| W19-20 | `RollbackManager` | `src/computer/rollback.rs` | ✅ 已实现（文件级 + 系统级 APFS/Btrfs/System Restore + 单步 undo） |
| W21-24 | TaskStateStore + 持久化队列 | `src/planner/state.rs` + `src/planner/persistent_queue.rs` | ✅ 已实现（SQLite） |
| W25-26 | 工具链推理 + 工具合成 + 工具学习 | `src/planner/tool_chain.rs` + `composite_tool.rs` + `tool_learning.rs` | ✅ 已实现 |
| W27-28 | 错误诊断 + 定时任务 | `src/planner/error_diagnosis.rs` + `src/planner/scheduled_tasks.rs` | ✅ 已实现 |

---

### 9.7 关键风险与规避

| 风险 | 影响 | 规避方案 |
|------|------|----------|
| Accessibility API 权限复杂（macOS 需用户授权） | 功能不可用 | 优雅降级到纯截图方案；首次启动引导用户授权 |
| Wayland 安全限制过严 | Linux Wayland 体验差 | 文档明确说明限制；推荐 X11 或 xdg-desktop-portal |
| Playwright 依赖大（~100MB） | 安装包膨胀 | 可选依赖，首次使用时自动下载；无头环境使用轻量方案 |
| LLM 分解目标 hallucination | 执行错误任务 | DAG 执行前人工确认；高权限操作强制审批 |
| 坐标系统跨平台不一致 | 点击位置偏移 | `ComputerAdapter` 内部统一为逻辑坐标，`DpiScale::detect()` 处理 DPI 转换 |

---

### 9.8 总结

当前 `CapabilitySet + ToolRegistry` 架构已经非常接近目标态。已实现的核心能力：

1. ✅ **Layer 1 平台层**：各平台 Accessibility API（macOS/Windows/Linux）已补齐
2. ✅ **Layer 3 统一抽象**：`ComputerAdapter` trait + 各平台适配器 + `ComputerUseLoop`
3. ✅ **Layer 4 规划层**：`GoalPlanner` + `DagScheduler` + `TaskExecutor` + `VerificationEngine`
4. ✅ **安全层**：`SandboxInterceptor`（路径白名单/黑名单、命令黑名单、域名白名单、资源配额）+ `RollbackManager`（文件级 + 系统级快照 + 单步 undo）+ `ContentFilter`（PII/Secret 检测）+ `SecurityAuditReport`
5. ✅ **移动端桥接**：`AndroidSet`（ADB 截图/点击/输入/应用管理）+ `IosSet`（libimobiledevice）

剩余重点工作（按优先级排序）：

1. **截图编码优化** — 根据网络状况自动调整分辨率/质量（本地运行时原图，远程运行时压缩）
2. **远程控制** — VNC/RDP 网关，通过网络控制远程物理机
3. **扩展层**：嵌入式与物联网（GPIO/Home Assistant/串口设备）、机器人接口（ROS2/机械臂控制）

现有代码中 `ToolContext` 的沙箱能力、`ToolRegistry` 的审批/熔断/缓存机制、`CapabilityRegistry` 的平台检测逻辑，都为上层建设提供了坚实基础，无需推倒重来。
