# OS Control — 让 Agent 完全掌握操作系统

这份文档描述如何让 Syscity Agent "感知并操作"它所在的操作系统（Linux、macOS、Windows），使 OS 看起来被 Agent 完全掌握和使用。

> **重要区分**：操作系统有**两种形态**，Agent 对它们的"掌控"方式完全不同：
> - **Desktop OS**（有 GUI）—— 通过 Accessibility API 获取 UI 树为主，截图 + VLM 为辅，精准操作控件
> - **Server OS**（Headless，无 GUI）—— 通过系统命令、结构化数据、日志分析来"像 sysadmin 一样管理"

---

## 1. 什么是"完全掌控"

"完全掌控"不是指 Agent 能执行某个特定命令，而是指 Agent 具备**系统管理员的完整能力闭环**：

| 维度 | 含义 | Desktop 示例 | Server 示例 |
|------|------|-------------|------------|
| **可观测** | 能获取系统当前完整状态 | 截图、窗口列表 | 进程、服务、日志、网络、磁盘 |
| **可理解** | 能把原始数据解读为语义 | "这是 Chrome 浏览器，当前在设置页" | "Nginx 服务已停止，日志显示端口冲突" |
| **可操作** | 能执行改变系统状态的动作 | 点击按钮、输入文字 | 重启服务、修改配置、安装软件 |
| **可诊断** | 能定位问题根因 | "这个弹窗是权限提示" | "OOM killer 终止了进程，因为内存不足" |
| **可规划** | 能把复杂任务分解为步骤序列 | "打开终端 → 输入命令 → 回车" | "备份配置 → 修改端口 → 测试启动 → 回滚如失败" |

---

## 2. 两种掌控模式

### 2.1 Desktop 控制 — GUI 环境

适用于有图形界面的桌面系统。Agent 通过 **Accessibility API 获取结构化 UI 树**为主，**截图 + VLM 为辅**，实现高效精准的 GUI 操作。

> **关键洞察**：纯截图方案效率低、成本高、易误判。各平台都提供了 Accessibility（无障碍）协议，可直接读取控件类型、文本、位置、状态等结构化信息。VLM 只用于"理解无法通过 API 获取的视觉内容"。

**核心循环（混合方案）**：

```
1. 获取 UI 树（Accessibility API）          → 结构化感知
2. 必要时截图 + VLM 补充理解               → 视觉验证
3. LLM 分析 UI 树 + 截图 → 决策           → 语义推理
4. 通过 Accessibility API 执行控件动作     → 精准操作
5. 等待状态变化 → 再次获取 UI 树验证        → 结构化验证
6. 必要时截图确认最终效果                  → 视觉兜底
```

**三种感知模式对比**：

| 维度 | 纯截图 + VLM | Accessibility API | **混合方案（推荐）** |
|------|------------|-------------------|---------------------|
| **精度** | 需 OCR/视觉理解，易误判 | 直接读取控件文本/坐标 | **结构化为主 + 视觉兜底** |
| **速度** | 慢（截图 + 上传 + 推理） | 快（本地 API 调用） | **毫秒级感知，秒级验证** |
| **结构化** | 无结构，纯图像 | 完整 UI 树 | **控件树 + 截图** |
| **可操作性** | 计算像素坐标 | 直接操作控件 ID | **控件级精准操作** |
| **成本** | 高（VLM token 消耗） | 零（本地 API） | **低成本，VLM 仅辅助** |

**能力矩阵**：

| 能力 | Linux | macOS | Windows |
|------|-------|-------|---------|
| **UI 树提取（主）** | `at-spi2` (D-Bus) | `AXUIElement` | `UI Automation` |
| **GUI 截图（辅）** | `grim` / `scrot` | `screencapture` | `BitBlt` |
| **控件操作（主）** | `at-spi2` 动作接口 | `AXUIElement` 动作 | `UIA_InvokePattern` |
| **键鼠模拟（兜底）** | `ydotool` / `xdotool` | `CGEvent` | `SendInput` |
| **窗口管理** | `wmctrl` / `swaymsg` | AppleScript / AX | `FindWindow` |
| **系统控制** | `systemctl` / D-Bus | `osascript` | PowerShell |

#### Accessibility API 详解

各平台的 Accessibility API 提供了标准化的 UI 元素访问能力：

**Linux — `at-spi2` (Assistive Technology Service Provider Interface)**
- 基于 D-Bus 的跨进程通信协议
- 可枚举应用窗口、控件、文本、菜单等
- 支持获取控件角色（Role）、状态（State）、文本内容
- 支持对控件执行默认动作（如点击按钮）

```bash
# 使用 Python + pyatspi 获取当前焦点应用的 UI 树
python3 -c "
import pyatspi
reg = pyatspi.Registry
desktop = reg.getDesktop(0)
for app in desktop:
    if app.getState().contains(pyatspi.STATE_ACTIVE):
        print(f'App: {app.name}')
        for child in app:
            print(f'  {child.getRoleName()}: {child.name}')
"
```

**macOS — `AXUIElement` (Accessibility API)**
- 系统级无障碍框架，通过 C API 调用
- 可枚举所有 UI 元素，获取属性（kAXTitleAttribute, kAXValueAttribute）
- 支持执行动作（kAXPressAction, kAXSetValueAction）
- 也可通过 AppleScript / `osascript` 间接调用

```bash
# AppleScript：获取 Safari 前窗口的 UI 树
osascript -e 'tell application "System Events" to tell process "Safari" to get entire contents of front window'

# 输出示例（简化）：
# window "GitHub - Safari"
#   group
#     text field "Search or enter address" (value: "github.com")
#     button "Reload" (enabled: true)
#     scroll area
#       web area
#         heading "Build software better, together"
#         button "Sign up"
```

**Windows — `UI Automation` (UIA)**
- COM 接口，支持所有 Windows 应用
- 提供 `IUIAutomationElement` 遍历控件树
- 支持 `IUIAutomationTreeWalker` 遍历、查找元素
- 支持 `InvokePattern`（点击）、`ValuePattern`（输入）等

```powershell
# PowerShell + UI Automation 获取计算器窗口的按钮
Add-Type -AssemblyName UIAutomationClient
$ui = New-Object System.Windows.Automation.AutomationElement
$calc = $ui.RootElement.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
    [System.Windows.Automation.Condition]::TrueCondition)
$calc.FindAll([System.Windows.Automation.TreeScope]::Descendants,
    [System.Windows.Automation.ControlTypeCondition]::CreateCondition([System.Windows.Automation.ControlType]::Button))
```

### 2.2 Server 控制 — Headless 环境

适用于无图形界面的服务器。Agent 像资深系统管理员一样通过命令行和结构化数据管理系统。

**核心循环**：

```
采集系统状态 → 结构化分析 → 决策 → 执行命令 → 采集验证 → 分析日志 ...
```

**关键洞察**：Server 不需要"看图"，需要**结构化信息采集 + 系统级命令执行**。

---

## 3. Server 完全掌控 — 能力全景

对 Linux Server 的"完全掌控"意味着 Agent 能在以下每个领域独立工作：

### 3.1 进程与资源

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 进程列表 | `ps aux`, `htop -n 1` | `kill`, `killall`, `nice`, `renice` |
| CPU/内存/磁盘 | `top -b -n 1`, `free`, `df` | `cgcreate`, `ulimit` |
| I/O 统计 | `iostat`, `iotop` | 调整 I/O 调度器 |
| 打开的文件 | `lsof`, `fuser` | 强制释放句柄 |

### 3.2 服务与守护进程

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 服务状态 | `systemctl list-units`, `service --status-all` | `systemctl start/stop/restart/enable/disable` |
| 定时任务 | `crontab -l`, `systemctl list-timers` | `crontab -e`, 编辑 `/etc/cron.d/*` |
| 开机服务 | `systemctl list-unit-files --state=enabled` | `systemctl enable/disable` |

### 3.3 网络

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 连接状态 | `ss -tulnp`, `netstat -tulnp` | `iptables`, `nftables`, `firewall-cmd` |
| 路由表 | `ip route`, `route -n` | `ip route add/del` |
| DNS 配置 | `cat /etc/resolv.conf`, `systemd-resolve --status` | 编辑配置文件 |
| 抓包分析 | `tcpdump -c 100 -w /tmp/capture.pcap` | `tcpdump`, `tshark` |
| 带宽/流量 | `iftop`, `nload`, `/proc/net/dev` | `tc` (traffic control) |

### 3.4 存储与文件系统

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 磁盘使用 | `df -h`, `lsblk` | `mount`, `umount`, `resize2fs` |
| 目录大小 | `du -sh /path` | 清理日志、归档旧文件 |
| 文件系统类型 | `findmnt`, `blkid` | `mkfs`, `fsck` |
| LVM/RAID | `lvs`, `vgs`, `pvs`, `cat /proc/mdstat` | `lvcreate`, `vgextend`, `mdadm` |
| 磁盘健康 | `smartctl -a /dev/sda` | — |

### 3.5 用户与权限

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 用户列表 | `cat /etc/passwd`, `getent passwd` | `useradd`, `usermod`, `userdel` |
| 组与权限 | `cat /etc/group`, `id username` | `groupadd`, `gpasswd`, `chmod`, `chown` |
| sudo 权限 | `cat /etc/sudoers`, `visudo -c` | 编辑 `/etc/sudoers.d/*` |
| 登录记录 | `last`, `lastb`, `who` | — |
| SSH 密钥 | `~/.ssh/authorized_keys` | `ssh-keygen`, 添加/移除密钥 |

### 3.6 日志与审计

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 系统日志 | `journalctl`, `tail -f /var/log/syslog` | `journalctl --rotate`, 清理旧日志 |
| 应用日志 | `tail`, `grep` | 日志轮转配置 (`logrotate`) |
| 审计日志 | `ausearch`, `aureport` | `auditctl` 规则管理 |
| 安全事件 | `/var/log/auth.log`, `/var/log/secure` | `fail2ban` 配置 |
| 内核消息 | `dmesg`, `journalctl -k` | — |

### 3.7 软件与包管理

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 已安装包 | `dpkg -l`, `rpm -qa`, `pacman -Q` | `apt`, `yum`, `pacman` |
| 包更新 | `apt list --upgradable` | `apt upgrade`, `yum update` |
| 仓库源 | `cat /etc/apt/sources.list` | 编辑源列表 |
| 容器镜像 | `docker images`, `podman images` | `docker pull/run/stop/rm` |

### 3.8 内核与系统配置

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 内核参数 | `sysctl -a`, `cat /proc/sys/*` | `sysctl -w`, 编辑 `/etc/sysctl.conf` |
| 内核模块 | `lsmod`, `modinfo` | `modprobe`, `rmmod` |
| 启动参数 | `cat /proc/cmdline` | 编辑 GRUB 配置 |
| 环境变量 | `env`, `cat /etc/environment` | 编辑 shell profile |
| 主机信息 | `hostnamectl`, `uname -a` | `hostnamectl set-hostname` |

### 3.9 安全与合规

| 能力 | 采集方式 | 操作方式 |
|------|---------|---------|
| 防火墙规则 | `iptables -L -v -n`, `nft list ruleset` | `iptables`, `nft` |
| SELinux/AppArmor | `getenforce`, `aa-status` | `setenforce`, `aa-enforce` |
| 文件完整性 | `aide --check` | `aide --init` |
| 开放端口 | `nmap -sT localhost`, `ss -tulnp` | 关闭不需要的服务 |
| 漏洞扫描 | `lynis audit system` | 按建议修复 |

---

## 4. 在 Syscity 中落地的代码方案

### 4.1 模块结构

```
src/os/
├── mod.rs              # 平台检测、能力声明、统一入口
├── platform.rs         # Platform trait + 条件编译分发
├── desktop.rs          # Desktop 控制 — 仅 GUI 环境
│   ├── mod.rs          # DesktopOperator 主循环
│   ├── accessibility.rs # Accessibility API 抽象层
│   ├── screenshot.rs   # 截图实现
│   └── input.rs        # 键鼠模拟（兜底方案）
├── server.rs           # Server 控制 (系统信息采集/命令执行) — Headless
├── system.rs           # 通用系统状态 (CPU/内存/磁盘/网络)
└── perception.rs       # 感知流水线
    ├── desktop.rs      # UI 树 + 截图 混合感知
    └── server.rs       # 结构化数据采集 + LLM 分析
```

### 4.2 新增工具集

#### Desktop 工具（GUI 环境）

| 新工具 | 能力 | 实现方式 |
|--------|------|----------|
| `desktop_control` | UI 树提取、控件操作、截图验证、键鼠模拟（兜底） | Accessibility API + 截图 + 键鼠模拟 |
| `accessibility` | 获取应用 UI 树、枚举控件、读取控件属性 | `at-spi2` / `AXUIElement` / `UI Automation` |
| `notification` | 发送/监听系统通知 | shell + OS API |
| `applescript` | macOS 自动化 | shell (`osascript`) |

##### `desktop_control` 工具设计（混合方案）

```rust
/// 桌面感知模式
pub enum DesktopPerceptionMode {
    /// 纯 Accessibility API（最快，零成本）
    AccessibilityOnly,
    /// 纯截图 + VLM（最通用，成本高）
    ScreenshotOnly,
    /// 混合方案：UI 树为主，截图验证为辅（推荐默认）
    Hybrid,
}

/// Desktop 控制入口
pub struct DesktopControl {
    perception: DesktopPerceptionMode,
    /// 各平台 Accessibility 客户端
    accessibility: Option<Box<dyn AccessibilityClient>>,
    /// 截图工具
    screenshot: Option<ScreenshotTool>,
    /// 键鼠模拟（兜底方案）
    input: Option<InputSimulator>,
}

impl DesktopControl {
    /// 感知当前桌面状态
    pub async fn perceive(&self) -> Result<DesktopPerception> {
        match self.perception {
            DesktopPerceptionMode::AccessibilityOnly => {
                let ui_tree = self.accessibility.as_ref()
                    .ok_or("Accessibility not available")?
                    .get_ui_tree().await?;
                Ok(DesktopPerception::UiTree(ui_tree))
            }
            DesktopPerceptionMode::ScreenshotOnly => {
                let image = self.screenshot.as_ref()
                    .ok_or("Screenshot not available")?
                    .capture().await?;
                Ok(DesktopPerception::Screenshot(image))
            }
            DesktopPerceptionMode::Hybrid => {
                // 1. 先获取 UI 树
                let ui_tree = self.accessibility.as_ref()
                    .ok_or("Accessibility not available")?
                    .get_ui_tree().await?;
                // 2. 同时截图（用于 VLM 视觉验证）
                let screenshot = self.screenshot.as_ref()
                    .map(|s| s.capture().await.ok())
                    .flatten();
                Ok(DesktopPerception::Hybrid { ui_tree, screenshot })
            }
        }
    }

    /// 执行桌面操作
    pub async fn act(&self, action: DesktopAction) -> Result<()> {
        match action {
            // 优先使用 Accessibility API 操作控件
            DesktopAction::ClickElement { element_id } => {
                if let Some(acc) = &self.accessibility {
                    if let Ok(()) = acc.click(element_id).await {
                        return Ok(());
                    }
                }
                // 兜底：计算坐标后用键鼠模拟
                self.fallback_click(element_id).await
            }
            DesktopAction::TypeText { element_id, text } => {
                if let Some(acc) = &self.accessibility {
                    if let Ok(()) = acc.set_value(element_id, &text).await {
                        return Ok(());
                    }
                }
                self.fallback_type(element_id, &text).await
            }
            // ...
        }
    }
}
```

#### Server 工具（Headless 环境）

| 新工具 | 能力 | 实现方式 |
|--------|------|----------|
| `system_inspect` | 采集系统完整快照（进程/服务/网络/磁盘/日志） | 封装系统命令，输出结构化 JSON |
| `service_manager` | 管理 systemd 服务 | `systemctl` 命令封装 |
| `log_analyzer` | 检索和分析日志 | `journalctl` / `grep` / `awk` 封装 |
| `network_diag` | 网络诊断（ping、traceroute、端口检测） | `ping`, `ss`, `curl`, `dig` 封装 |
| `package_manager` | 包管理（查询/安装/更新/卸载） | `apt` / `yum` / `pacman` 封装 |
| `user_manager` | 用户和权限管理 | `useradd`, `chmod`, `chown` 封装 |
| `firewall_manager` | 防火墙规则管理 | `iptables` / `nftables` / `firewall-cmd` 封装 |
| `cron_manager` | 定时任务管理 | `crontab` 封装 |

### 4.3 Server 感知循环（Server Perception Loop）

对 Server 而言，"掌握"不是看图，而是**持续采集结构化状态 + LLM 推理分析**：

```rust
// src/os/server.rs
pub struct ServerOperator {
    inspector: Arc<dyn SystemInspector>,   // 系统信息采集器
}

impl ServerOperator {
    /// 采集系统完整快照
    pub async fn inspect(&self) -> Result<SystemSnapshot> {
        Ok(SystemSnapshot {
            processes: self.inspector.processes().await?,
            services: self.inspector.services().await?,
            network: self.inspector.network().await?,
            storage: self.inspector.storage().await?,
            users: self.inspector.users().await?,
            logs: self.inspector.recent_logs(100).await?,
            packages: self.inspector.packages().await?,
            kernel: self.inspector.kernel_params().await?,
            security: self.inspector.security_status().await?,
            timestamp: Utc::now(),
        })
    }

    /// 分析系统快照，生成人类可读的诊断报告
    pub async fn diagnose(&self, snapshot: &SystemSnapshot) -> Result<Diagnosis> {
        let prompt = format!(
            "作为资深 Linux 系统管理员，分析以下系统快照，指出异常和潜在问题：\n\n{}",
            serde_json::to_string_pretty(snapshot)?
        );
        // 调用 LLM 分析
        let analysis = self.llm.analyze(&prompt).await?;
        Ok(Diagnosis { analysis, snapshot: snapshot.clone() })
    }
}
```

**Server 感知循环**：

```
1. system_inspect 采集完整系统快照
2. LLM 分析快照，识别异常/问题/优化点
3. Agent 决策：需要执行什么操作？
4. 执行命令（service_manager / package_manager / shell 等）
5. 再次采集验证
6. 如有错误，读取日志分析原因
```

### 4.4 系统快照结构

```rust
#[derive(Debug, Clone, Serialize)]
pub struct SystemSnapshot {
    pub hostname: String,
    pub uptime: Duration,
    pub load_average: [f64; 3],
    pub memory: MemoryInfo,
    pub cpu: CpuInfo,
    pub disks: Vec<DiskInfo>,
    pub network: NetworkInfo,
    pub processes: Vec<ProcessInfo>,
    pub services: Vec<ServiceInfo>,
    pub users: Vec<UserInfo>,
    pub listening_ports: Vec<PortInfo>,
    pub recent_logs: Vec<LogEntry>,
    pub pending_updates: Vec<String>,
    pub security_alerts: Vec<String>,
    pub timestamp: DateTime<Utc>,
}
```

---

## 5. 安全边界调整

现有 `ToolContext` 有 `sandboxed` + `workspace_only` + `allowed_paths`。控制 OS 必须**分级授权**：

```rust
pub enum OsControlScope {
    /// 只读观察（查看进程、日志、配置）
    ReadOnly,
    /// 用户空间操作（管理用户进程、用户级配置）
    UserSpace,
    /// 系统级操作（启停服务、修改系统配置、包管理）
    System,
    /// 完全控制（内核参数、防火墙、用户管理）
    Root,
}

pub struct OsControlLevel {
    pub scope: OsControlScope,
    /// 文件系统范围
    pub fs_scope: FsScope,
    /// 允许执行的命令白名单（空 = 全部允许）
    pub allowed_commands: Vec<String>,
    /// 是否允许修改系统配置
    pub can_modify_system_config: bool,
    /// 是否允许管理用户
    pub can_manage_users: bool,
    /// 是否允许修改网络/防火墙
    pub can_manage_network: bool,
    /// 是否允许安装/卸载软件
    pub can_manage_packages: bool,
}
```

**审批策略**：

| 操作 | 所需 scope | 是否需审批 |
|------|-----------|-----------|
| `ps aux` | ReadOnly | 否 |
| `tail /var/log/syslog` | ReadOnly | 否 |
| `systemctl restart nginx` | System | 是 |
| `apt install package` | System | 是 |
| `useradd newuser` | Root | 是 |
| `iptables -F` | Root | 是（高危险） |
| `sysctl -w vm.swappiness=10` | Root | 是 |
| `rm -rf /` | — | **自动拒绝** |

---

## 6. 最小可行方案 (MVP)

### 方案 A：Shell 万能胶（立即可用）

给 `shell` 工具足够权限，Agent 直接用命令管理系统：

```bash
# 诊断 Nginx 问题
systemctl status nginx
journalctl -u nginx --since "1 hour ago" -n 50
ss -tulnp | grep :80
cat /var/log/nginx/error.log | tail -20

# 修复：端口冲突
lsof -i :80          # 找出占用端口的进程
kill -9 <pid>        # 终止冲突进程
systemctl restart nginx

# 安装软件
apt update && apt install -y htop
```

**优点**：零开发，立即可用
**缺点**：Agent 需要自己拼凑命令，没有结构化反馈

### 方案 B：system_inspect 工具（推荐 MVP）

新增一个 `system_inspect` 工具，一键采集系统快照并返回结构化 JSON：

```json
{
  "tool": "system_inspect",
  "args": {
    "sections": ["processes", "services", "network", "logs"],
    "log_lines": 50,
    "since": "1 hour ago"
  }
}
```

返回：

```json
{
  "hostname": "web-server-01",
  "uptime": "45 days",
  "load": [0.5, 0.3, 0.2],
  "memory": { "total": "16G", "used": "8.2G", "free": "7.8G" },
  "disks": [
    { "mount": "/", "size": "100G", "used": "78G", "available": "22G" }
  ],
  "services": [
    { "name": "nginx", "status": "active", "enabled": true },
    { "name": "mysql", "status": "failed", "enabled": true }
  ],
  "processes": [ ... ],
  "logs": [
    { "time": "2024-01-15T10:23:00Z", "service": "mysql", "level": "ERROR", "message": "..." }
  ]
}
```

**优点**：Agent 获得结构化数据，分析能力大幅提升
**工作量**：2-3 天封装常用命令

### 方案 C：Server 感知循环（完全体）

```
system_inspect → LLM 分析 → 决策 → 执行命令 → system_inspect 验证
     ↑_____________________________________________________________↓
```

Agent 能自主完成复杂运维任务：
- "排查为什么网站无法访问" → 检查服务/端口/日志/防火墙 → 定位问题 → 修复
- "安装并配置 Redis" → 安装包 → 修改配置 → 启动服务 → 验证
- "清理磁盘空间" → 分析磁盘使用 → 清理日志/缓存 → 验证

---

## 7. 实现优先级

### Desktop 控制（混合方案）

| 优先级 | 平台 | 方案 | 工作量 |
|--------|------|------|--------|
| **P1** | Linux | `at-spi2` UI 树提取 + `scrot` 截图 | 2-3 天 |
| **P1** | macOS | `AXUIElement` / AppleScript + `screencapture` | 2-3 天 |
| P2 | All | `desktop_control` 工具（Hybrid 模式）+ LLM 决策循环 | 1-2 周 |
| P2 | Windows | `UI Automation` + `BitBlt` 截图 | 1-2 周 |
| P3 | All | 纯 VLM 视觉方案（截图 + 键鼠模拟）| 1-2 周 |

### Server 控制

| 优先级 | 能力 | 方案 | 工作量 |
|--------|------|------|--------|
| **P0** | 系统快照 | `system_inspect` 工具 | 2-3 天 |
| **P0** | 服务管理 | `service_manager` 工具 | 1 天 |
| **P0** | 日志分析 | `log_analyzer` 工具 | 1-2 天 |
| P1 | 网络诊断 | `network_diag` 工具 | 1-2 天 |
| P1 | 包管理 | `package_manager` 工具 | 2-3 天 |
| P1 | 感知循环 | ServerOperator + LLM 诊断 | 3-5 天 |
| P2 | 用户/权限 | `user_manager` 工具 | 2-3 天 |
| P2 | 防火墙 | `firewall_manager` 工具 | 2-3 天 |
| P2 | 安全扫描 | 集成 `lynis` / 审计规则 | 3-5 天 |

---

## 8. 总结

| 场景 | 核心能力 | 关键工具 |
|------|---------|---------|
| **Desktop** | **结构化 UI 树感知** + 控件精准操作 + 截图验证 | `desktop_control` (Hybrid), `accessibility`, `screenshot` |
| **Server** | 结构化信息采集 + 系统命令执行 | `system_inspect`, `service_manager`, `log_analyzer` |

对 Linux Server 来说，"完全掌控" = Agent 能像**7x24 值班的资深 SRE**一样：
1. 随时知道系统发生了什么（可观测）
2. 能读懂日志和指标的含义（可理解）
3. 能执行修复和优化操作（可操作）
4. 能追踪根因，不只是治标（可诊断）
5. 能规划多步复杂任务并安全执行（可规划）
