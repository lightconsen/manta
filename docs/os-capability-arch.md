# OS Capability Sets — 可扩展平台能力架构

## 核心问题

OS 控制功能需要按平台/场景组织为可插拔的集合：
- **Linux** — 无 GUI，systemd，命令行管理
- **Linux Desktop (X11)** — X11 环境，xdotool/xclip/xwd 截图+UI树
- **Linux Desktop (Wayland)** — Wayland 环境，xdg-desktop-portal/grim 截图+UI树
- **macOS** — AXUIElement，AppleScript
- **Windows** — UI Automation，PowerShell
- **未来扩展** — Android、iOS、嵌入式、机器人...

每个集合内部有相似的能力（感知、操作、诊断），但实现方式完全不同。

## 核心设计：三层架构

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 3: Agent (LLM 调用层)                                 │
│  ── 不关心底层是哪个平台，只关心 "我需要 system_inspect"       │
│                                                              │
│  Agent.complete(request)                                     │
│    → ToolRegistry.get_available() → 返回所有可用工具定义     │
│    → LLM 选择工具 → Agent 执行 → 结果回传                    │
└─────────────────────────────────────────────────────────────┘
                              ▲
┌─────────────────────────────┴───────────────────────────────┐
│  Layer 2: ToolRegistry (现有，无需改动)                       │
│  ── 管理单个 Tool 的生命周期、权限、熔断、审批                 │
│                                                              │
│  ToolRegistry { tools: HashMap<String, Box<dyn Tool>> }      │
│    register(tool) / get_available(context) / execute()       │
└─────────────────────────────────────────────────────────────┘
                              ▲
┌─────────────────────────────┴───────────────────────────────┐
│  Layer 1: CapabilityRegistry (新增)                          │
│  ── 按平台/场景组织工具集合，运行时检测，动态注册               │
│                                                              │
│  CapabilitySet ──→ 包含多个 Tool                              │
│    LinuxSet { system_inspect, service_manager, ... }   │
│    LinuxDesktopX11Set { desktop_control, accessibility, ... }   │
│    ...                                                       │
└─────────────────────────────────────────────────────────────┘
```

**关键原则**：
- `CapabilitySet` 是**组织单元**，不是执行单元
- 真正被 LLM 调用的仍然是单个 `Tool`
- `CapabilityRegistry` 负责"在什么环境下启用哪些工具"

## Layer 1: CapabilitySet

### 定义

```rust
// src/capabilities/mod.rs

/// 平台能力集合 — 一组针对特定平台/场景的工具
///
/// 每个 CapabilitySet 代表一个"环境"（如 Linux Server、macOS Desktop），
/// 包含该平台下所有相关的工具。
pub trait CapabilitySet: Send + Sync {
    /// 唯一标识，如 "linux"
    fn id(&self) -> &str;

    /// 人类可读名称
    fn name(&self) -> &str;

    /// 描述
    fn description(&self) -> &str;

    /// 平台约束 — 当前环境是否满足该集合的运行条件
    fn constraints(&self) -> &PlatformConstraints;

    /// OS 控制权限范围
    fn scope(&self) -> OsControlScope;

    /// 该集合提供的所有工具
    fn tools(&self) -> Vec<Box<dyn Tool>>;

    /// 运行时检测当前环境是否支持该集合
    fn is_available(&self) -> bool {
        self.constraints().check()
    }
}

/// 平台约束条件
#[derive(Debug, Clone)]
pub struct PlatformConstraints {
    /// 目标操作系统 (对应 cfg(target_os))
    pub target_os: Vec<String>,
    /// 是否需要 GUI 环境
    pub requires_gui: bool,
    /// 是否需要特定服务 (如 systemd)
    pub requires_services: Vec<String>,
    /// 自定义检测函数
    pub custom_check: Option<fn() -> bool>,
}

impl PlatformConstraints {
    /// 检测当前环境是否满足约束
    pub fn check(&self) -> bool {
        // 1. 检查目标 OS
        let current_os = std::env::consts::OS;
        if !self.target_os.iter().any(|os| os == current_os) {
            return false;
        }

        // 2. 检查 GUI 环境
        if self.requires_gui {
            if !has_display_server() {
                return false;
            }
        }

        // 3. 检查必需服务
        for service in &self.requires_services {
            if !is_service_available(service) {
                return false;
            }
        }

        // 4. 自定义检测
        if let Some(check) = self.custom_check {
            if !check() {
                return false;
            }
        }

        true
    }
}

/// OS 控制权限范围（沿用 os.md 中的设计）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OsControlScope {
    /// 只读观察
    ReadOnly = 0,
    /// 用户空间操作
    UserSpace = 1,
    /// 系统级操作
    System = 2,
    /// 完全控制
    Root = 3,
}
```

### 具体集合实现

```rust
// src/capabilities/linux.rs

/// Linux Server 能力集合
pub struct LinuxSet;

impl CapabilitySet for LinuxSet {
    fn id(&self) -> &str { "linux" }
    fn name(&self) -> &str { "Linux Server Control" }
    fn description(&self) -> &str {
        "Complete control over a headless Linux server via structured system commands"
    }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: PlatformConstraints = PlatformConstraints {
            target_os: vec!["linux".to_string()],
            requires_gui: false,
            requires_services: vec!["systemd".to_string()], // 可选
            custom_check: None,
        };
        &CONSTRAINTS
    }

    fn scope(&self) -> OsControlScope { OsControlScope::System }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SystemInspectTool::new()),
            Box::new(ServiceManagerTool::new()),
            Box::new(LogAnalyzerTool::new()),
            Box::new(NetworkDiagTool::new()),
            Box::new(PackageManagerTool::new()),
            Box::new(FirewallManagerTool::new()),
            Box::new(UserManagerTool::new()),
            Box::new(CronManagerTool::new()),
        ]
    }
}
```

```rust
// src/capabilities/linux_desktop_x11.rs

/// Linux Desktop 能力集合
pub struct LinuxDesktopX11Set;

impl CapabilitySet for LinuxDesktopX11Set {
    fn id(&self) -> &str { "linux-desktop-x11" }
    fn name(&self) -> &str { "Linux Desktop Control" }
    fn description(&self) -> &str {
        "Control a Linux desktop environment via Accessibility API + screenshot hybrid"
    }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: PlatformConstraints = PlatformConstraints {
            target_os: vec!["linux".to_string()],
            requires_gui: true,
            requires_services: vec![],
            custom_check: Some(|| has_at_spi2()), // 检测 at-spi2 是否可用
        };
        &CONSTRAINTS
    }

    fn scope(&self) -> OsControlScope { OsControlScope::UserSpace }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(DesktopControlTool::new(DesktopPerceptionMode::Hybrid)),
            Box::new(AccessibilityTool::new()),
            Box::new(ScreenshotTool::new()),
            Box::new(InputSimulatorTool::new()), // 兜底键鼠模拟
        ]
    }
}
```

```rust
// src/capabilities/macos.rs

/// macOS Desktop 能力集合
pub struct MacosSet;

impl CapabilitySet for MacosSet {
    fn id(&self) -> &str { "macos" }
    fn name(&self) -> &str { "macOS Desktop Control" }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: PlatformConstraints = PlatformConstraints {
            target_os: vec!["macos".to_string()],
            requires_gui: true,
            requires_services: vec![],
            custom_check: Some(|| has_ax_access()), // 检测辅助功能权限
        };
        &CONSTRAINTS
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(DesktopControlTool::new(DesktopPerceptionMode::Hybrid)),
            Box::new(AccessibilityTool::new()), // macOS AXUIElement 实现
            Box::new(ScreenshotTool::new()),    // screencapture
            Box::new(AppleScriptTool::new()),
        ]
    }
}
```

```rust
// src/capabilities/windows.rs

/// Windows Desktop 能力集合
pub struct WindowsSet;

impl CapabilitySet for WindowsSet {
    fn id(&self) -> &str { "windows" }
    fn name(&self) -> &str { "Windows Desktop Control" }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: PlatformConstraints = PlatformConstraints {
            target_os: vec!["windows".to_string()],
            requires_gui: true,
            requires_services: vec![],
            custom_check: None,
        };
        &CONSTRAINTS
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(DesktopControlTool::new(DesktopPerceptionMode::Hybrid)),
            Box::new(AccessibilityTool::new()), // Windows UI Automation 实现
            Box::new(ScreenshotTool::new()),
            Box::new(PowerShellTool::new()),
        ]
    }
}
```

## Layer 1: CapabilityRegistry

```rust
// src/capabilities/registry.rs

/// 能力集合注册表 — 管理所有平台能力集合
pub struct CapabilityRegistry {
    sets: Vec<Box<dyn CapabilitySet>>,
    /// 被禁用的集合 ID
    disabled: HashSet<String>,
    /// 环境检测结果缓存
    availability_cache: std::sync::RwLock<HashMap<String, bool>>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self {
            sets: Vec::new(),
            disabled: HashSet::new(),
            availability_cache: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// 注册一个能力集合
    pub fn register(&mut self, set: Box<dyn CapabilitySet>) {
        self.sets.push(set);
    }

    /// 禁用某个集合（用户手动关闭）
    pub fn disable(&mut self, set_id: &str) {
        self.disabled.insert(set_id.to_string());
    }

    /// 启用某个集合
    pub fn enable(&mut self, set_id: &str) {
        self.disabled.remove(set_id);
    }

    /// 获取当前环境可用的所有集合
    pub fn available_sets(&self) -> Vec<&dyn CapabilitySet> {
        self.sets
            .iter()
            .filter(|s| !self.disabled.contains(s.id()))
            .filter(|s| self.check_availability(s.id(), s))
            .map(|s| s.as_ref())
            .collect()
    }

    /// 获取所有集合（不分是否可用）
    pub fn all_sets(&self) -> Vec<&dyn CapabilitySet> {
        self.sets.iter().map(|s| s.as_ref()).collect()
    }

    /// 获取特定集合
    pub fn get(&self, id: &str) -> Option<&dyn CapabilitySet> {
        self.sets.iter().find(|s| s.id() == id).map(|s| s.as_ref())
    }

    /// 检查集合在当前环境是否可用（带缓存）
    fn check_availability(&self, id: &str, set: &dyn CapabilitySet) -> bool {
        if let Ok(cache) = self.availability_cache.read() {
            if let Some(cached) = cache.get(id) {
                return *cached;
            }
        }

        let available = set.is_available();

        if let Ok(mut cache) = self.availability_cache.write() {
            cache.insert(id.to_string(), available);
        }

        available
    }

    /// 刷新环境检测缓存（环境变化时调用）
    pub fn refresh_cache(&self) {
        if let Ok(mut cache) = self.availability_cache.write() {
            cache.clear();
        }
    }

    /// 导出所有可用工具到 ToolRegistry
    pub fn export_to_tool_registry(&self, registry: &mut ToolRegistry) {
        for set in self.available_sets() {
            for tool in set.tools() {
                registry.register(tool);
            }
        }
    }

    /// 导出特定权限范围内的工具（用于权限升级场景）
    pub fn export_with_scope(
        &self,
        registry: &mut ToolRegistry,
        max_scope: OsControlScope,
    ) {
        for set in self.available_sets() {
            if set.scope() <= max_scope {
                for tool in set.tools() {
                    registry.register(tool);
                }
            }
        }
    }
}
```

## 组合与冲突处理

### 多 Set 共存是默认行为

同一台机器可以同时启用多个 CapabilitySet。`CapabilityRegistry` 会合并所有可用集合的工具，注册到 `ToolRegistry`。

```
Linux 开发机（有 GUI + systemd）
  ├─ LinuxSet     → system_inspect, service_manager, log_analyzer...
  ├─ LinuxDesktopX11Set    → desktop_control, accessibility, screenshot...
  └─ 合并注册到 ToolRegistry
        → LLM 同时看到 Server 和 Desktop 工具
```

**典型场景**：
- **Linux 开发机**：Server + Desktop 同时启用。用 `system_inspect` 查看系统状态，用 `desktop_control` 操作 IDE
- **Linux 服务器**：只有 Server Set 通过检测（`requires_gui: false`），Desktop Set 自动禁用
- **WSL（Windows Subsystem for Linux）**：Linux Server Set 可用，但 Desktop Set 可能不可用（取决于是否配置了图形转发）

### 工具名称冲突处理

如果两个 Set 注册了同名工具，需要明确的冲突解决策略：

```rust
pub enum ToolConflictStrategy {
    /// 报错，拒绝注册（默认）
    Reject,
    /// 后注册的覆盖先注册的
    Override,
    /// 保留两个，给 LLM 足够的上下文让它选择
    KeepBoth,
}

impl CapabilityRegistry {
    pub fn export_to_tool_registry(
        &self,
        registry: &mut ToolRegistry,
        strategy: ToolConflictStrategy,
    ) {
        let mut seen = HashSet::new();

        for set in self.available_sets() {
            for tool in set.tools() {
                let name = tool.name().to_string();

                if seen.contains(&name) {
                    match strategy {
                        ToolConflictStrategy::Reject => {
                            panic!("Tool '{}' conflicts between capability sets", name);
                        }
                        ToolConflictStrategy::Override => {
                            registry.register(tool); // 覆盖
                        }
                        ToolConflictStrategy::KeepBoth => {
                            // 给工具加前缀，如 "linux_desktop_screenshot"
                            // 或者保持原样，依赖 description 区分
                            registry.register(tool);
                        }
                    }
                } else {
                    seen.insert(name.clone());
                    registry.register(tool);
                }
            }
        }
    }
}
```

**实践中**，不同 Set 的工具名称天然不冲突：
- Server 工具：`system_inspect`, `service_manager`, `log_analyzer`...
- Desktop 工具：`desktop_control`, `accessibility`, `screenshot`...
- 即使有同名（如两个 Set 都有 `shell`），基础 `shell` 工具已经在基础层注册，OS 层的 Set 不会再提供

### CapabilityProfile：预定义组合

让用户可以切换预设的"能力配置"，而不是手动开关每个 Set：

```rust
/// 预定义的能力配置档案
pub enum CapabilityProfile {
    /// 最小化 — 只有基础工具，无 OS 控制
    Minimal,
    /// 观察者 — 只读系统信息，不执行操作
    Observer,
    /// 服务器模式 — 基础 + Server 集合
    Server,
    /// 桌面模式 — 基础 + Desktop 集合
    Desktop,
    /// 全功能 — 基础 + 所有可用集合
    Full,
    /// 自定义 — 指定集合 ID 列表
    Custom(Vec<String>),
}

impl CapabilityProfile {
    /// 应用配置到 Registry
    pub fn apply(&self, registry: &mut CapabilityRegistry) {
        match self {
            CapabilityProfile::Minimal => {
                // 禁用所有 OS 控制集合
                for set in registry.all_sets() {
                    registry.disable(set.id());
                }
            }
            CapabilityProfile::Observer => {
                // 只启用 ReadOnly scope 的集合
                for set in registry.all_sets() {
                    if set.scope() != OsControlScope::ReadOnly {
                        registry.disable(set.id());
                    }
                }
            }
            CapabilityProfile::Server => {
                // 只启用非 GUI 的 Server 集合
                for set in registry.all_sets() {
                    if set.constraints().requires_gui {
                        registry.disable(set.id());
                    }
                }
            }
            CapabilityProfile::Desktop => {
                // 只启用 GUI 集合
                for set in registry.all_sets() {
                    if !set.constraints().requires_gui {
                        registry.disable(set.id());
                    }
                }
            }
            CapabilityProfile::Full => {
                // 启用所有（默认行为）
            }
            CapabilityProfile::Custom(ids) => {
                // 禁用不在列表中的集合
                for set in registry.all_sets() {
                    if !ids.contains(&set.id().to_string()) {
                        registry.disable(set.id());
                    }
                }
            }
        }
    }
}
```

**配置文件对应**：

```toml
# syscity.toml

[capabilities]
# 预设档案: minimal | observer | server | desktop | full
profile = "server"

# 或者自定义组合
# profile = "custom"
# custom_sets = ["linux", "linux-desktop-x11"]

# 权限上限
max_scope = "System"
```

## 初始化流程

```rust
// src/main.rs 或应用启动逻辑

fn init_capabilities() -> CapabilityRegistry {
    let mut cap_reg = CapabilityRegistry::new();

    // ── 条件编译注册 ──
    // 只在 Linux 编译 Linux 相关集合
    #[cfg(target_os = "linux")]
    {
        cap_reg.register(Box::new(LinuxSet));

        // Desktop 只在有 GUI 时可用（由运行时检测决定）
        cap_reg.register(Box::new(LinuxDesktopX11Set));
    }

    #[cfg(target_os = "macos")]
    {
        cap_reg.register(Box::new(MacosSet));
    }

    #[cfg(target_os = "windows")]
    {
        cap_reg.register(Box::new(WindowsSet));
    }

    // ── 未来扩展：动态加载第三方集合 ──
    // cap_reg.register(Box::new(AndroidMobileSet));
    // cap_reg.register(Box::new(RobotArmSet));

    cap_reg
}

fn init_tools(cap_reg: &CapabilityRegistry) -> ToolRegistry {
    let mut tool_reg = ToolRegistry::new();

    // 基础工具（所有平台都有）
    tool_reg.register(Box::new(ShellTool::new()));
    tool_reg.register(Box::new(FileReadTool::new()));
    tool_reg.register(Box::new(FileWriteTool::new()));
    // ...

    // 从 CapabilityRegistry 导入 OS 控制工具（多 Set 合并）
    cap_reg.export_to_tool_registry(&mut tool_reg, ToolConflictStrategy::Reject);

    // 标记特权工具
    tool_reg.mark_privileged("system_inspect");
    tool_reg.mark_privileged("service_manager");
    tool_reg.mark_privileged("firewall_manager");
    tool_reg.mark_privileged("user_manager");

    tool_reg
}
```

## 配置控制

用户可以通过配置文件启用/禁用特定集合：

```toml
# syscity.toml

[capabilities]
# 默认启用所有可用的集合
auto_detect = true

# 手动控制特定集合
disabled = ["linux-desktop-x11"]  # 在服务器上禁用 desktop 控制

# 权限范围上限（防止 Agent 越权）
max_scope = "System"  # 可选: ReadOnly, UserSpace, System, Root
```

```rust
// 配置加载
pub fn apply_config(&mut self, config: &CapabilitiesConfig) {
    if !config.auto_detect {
        // 关闭自动检测，只启用用户指定的集合
        // ...
    }

    for id in &config.disabled {
        self.disable(id);
    }
}
```

## 模块结构

```
src/
├── capabilities/
│   ├── mod.rs              # CapabilitySet trait, PlatformConstraints, OsControlScope
│   ├── registry.rs         # CapabilityRegistry
│   ├── config.rs           # CapabilitiesConfig
│   ├── linux.rs     # LinuxSet
│   ├── linux_desktop_x11.rs    # LinuxDesktopX11Set
│   ├── macos.rs    # MacosSet
│   ├── windows.rs  # WindowsSet
│   └── common/             # 跨平台共享的 OS 工具实现
│       ├── mod.rs
│       ├── system_inspect.rs
│       ├── service_manager.rs
│       ├── log_analyzer.rs
│       ├── network_diag.rs
│       ├── package_manager.rs
│       ├── firewall_manager.rs
│       ├── user_manager.rs
│       └── cron_manager.rs
├── os/
│   ├── mod.rs              # 平台抽象层（被 capabilities 使用）
│   ├── platform.rs
│   ├── desktop/
│   │   ├── mod.rs
│   │   ├── accessibility.rs   # 各平台 Accessibility 抽象
│   │   ├── screenshot.rs
│   │   └── input.rs
│   └── server/
│       ├── mod.rs
│       └── inspector.rs
└── tools/
    ├── mod.rs              # 现有 Tool trait, ToolRegistry（不变）
    ├── shell.rs
    ├── file.rs
    └── ...
```

## 为什么这样设计

| 设计点 | 原因 |
|-------|------|
| **CapabilitySet 不是 Tool** | 保持与现有架构兼容，LLM 调用的仍然是单个 Tool |
| **运行时检测** | 同一台 Linux 机器可能是 Server 也可能是 Desktop，需要动态判断 |
| **条件编译 + 动态检测** | 条件编译减少二进制体积，动态检测处理同一 OS 的不同形态 |
| **权限范围分级** | 用户可以在配置中限制 Agent 最大权限，安全可控 |
| **可扩展** | 新增平台只需新增一个 `CapabilitySet` 实现，零侵入现有代码 |

## 扩展示例：新增 Android 支持

```rust
// src/capabilities/android_mobile.rs

pub struct AndroidMobileSet;

impl CapabilitySet for AndroidMobileSet {
    fn id(&self) -> &str { "android-mobile" }
    fn name(&self) -> &str { "Android Mobile Control" }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: PlatformConstraints = PlatformConstraints {
            target_os: vec!["android".to_string()],
            requires_gui: true,
            requires_services: vec![],
            custom_check: Some(|| has_adb_connection()),
        };
        &CONSTRAINTS
    }

    fn scope(&self) -> OsControlScope { OsControlScope::UserSpace }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(AndroidUiAutomatorTool::new()),  // uiautomator
            Box::new(AdbShellTool::new()),            // adb shell
            Box::new(AndroidScreenshotTool::new()),   // adb screencap
        ]
    }
}

// 注册时只需加一行
cap_reg.register(Box::new(AndroidMobileSet));
```

零改动现有代码。
