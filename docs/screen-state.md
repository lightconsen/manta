# Computer Use 发展方案

Syscity 的 Computer Use 能力现状与增强路线。

---

## 一、现状

### 当前 macOS Desktop Control 能力

| 工具 | 功能 | 局限 |
|------|------|------|
| `macos_accessibility` | 查询 UI 树（AppleScript/System Events） | 扁平的列表，丢了层级结构 |
| `macos_screenshot` | 全屏截图（screencapture） | 不支持区域截图、不支持窗口截图 |
| `macos_desktop_control` | 点击、输入、快捷键 | 盲操作——无视觉反馈 |
| `applescript` | 执行任意 AppleScript | 原始能力，需要 LLM 自己写脚本 |

### 当前流程：盲点盲操作

```
agent 拿到 accessibility tree (文本，扁平的)
  → 决定: click role="button" name="OK"
  → 发 AppleScript: click button "OK"
  → 结束，没有视觉确认
```

问题是：accessibility tree 没有视觉信息。不知道按钮是否灰色、有没有弹窗挡住、布局是否已变。

---

## 二、窗口管理增强

### 当前能力

- `activate_window` — 按进程名激活
- `close_window` — 发 Cmd+W
- 仅此而已，无 resize/move/minimize

### 目标能力

| 功能 | 实现方式 | 价值 |
|------|----------|------|
| **列举所有窗口** | `tell app "System Events" to get name of every process whose visible is true` + 遍历窗口 | agent 知道当前打开了什么 |
| **按窗口标题激活** | 改为按窗口标题模糊匹配激活（浏览器多窗口场景必要） | 在多个 Chrome 窗口间切换 |
| **获取窗口位置和大小** | `position of window 1 of app "X"` / `size of window 1` | agent 知道窗口布局 |
| **移动窗口** | `set position of window 1 of app "X" to {x, y}` | 布局桌面，截图前排列窗口 |
| **调整窗口大小** | `set size of window 1 of app "X" to {w, h}` | 确保截图包含完整内容 |
| **最小化/最大化** | `set minimized of window 1 to true` / `set zoomed of window 1 to true` | 整理桌面空间 |

**典型场景**：agent 在做桌面操作前，先排列好窗口 → 截图 → 分析 → 操作。

---

## 三、多桌面 / Spaces 支持

### 当前能力：完全不存在

### 目标能力

| 功能 | 实现方式 | 价值 |
|------|----------|------|
| **查询当前 Space** | `tell app "System Events" to get name of current desktop` | 知道自己在第几个桌面 |
| **列举所有 Space** | 通过 NSWorkspace API 或 Mission Control AppleScript | 知道系统有几个桌面 |
| **切换 Space** | `tell app "System Events" to key code 124 using control down` | agent 可以换桌面工作 |
| **将窗口移到另一个 Space** | Dock 菜单 → Options → Assign To | 按任务分配桌面 |
| **创建/删除 Space** | 触发 Mission Control | 动态管理工作区 |

**典型场景**：桌面 1 查资料，桌面 2 写代码，桌面 3 跑测试。agent 能在桌面间切换，按任务组织工作空间。

---

## 四、快捷键系统增强

### 当前能力

- `key_shortcut` — 支持 1 modifier + 1 key
- `key_sequence` — 多个快捷键序列
- 限制：无组合键叠加，无功能键，无媒体键

### 目标能力

| 功能 | 实现方式 | 价值 |
|------|----------|------|
| **复杂组合键** | `key down cmd` + `key down shift` + `keystroke "4"` + key up 序列 | Cmd+Shift+4 截图等系统级快捷键 |
| **功能键** | F1-F12 | IDE 快捷键（如 F5 调试） |
| **媒体键** | key code 方式（100=播放/暂停） | 控制音乐/视频 |
| **系统全局热键** | CGEvent API 注册全局快捷键 | 用户按某键唤醒 agent |
| **修饰键映射** | 读取/修改系统修饰键配置 | 适配非标准键盘布局 |

**典型场景**：agent 注册 Ctrl+F12 为唤醒键，或操作 IDE 时发送 Cmd+Shift+P。

---

## 五、从"盲操作"到"有视觉反馈"——核心架构

### ScreenState：三个数据源合并

```rust
struct ScreenState {
    // 1. 树形 UI（结构化）
    ui_tree: UiNode,                     // 根节点，递归 children

    // 2. 截图（原始像素，存内存用于对比）
    screenshot: Vec<u8>,                 // PNG bytes

    // 3. OCR 文字（从截图提取）
    ocr_text: String,                    // 全部文字
    ocr_regions: Vec<TextRegion>,        // 带位置的文字区域
}

struct TextRegion {
    text: String,
    bounding_box: Rect,
    confidence: f32,
}
```

### Verification Loop：操作验证闭环

```
         ┌───────────────────────────────────────────┐
         │           VerificationLoop                  │
         │                                             │
action ──→  pre_snapshot()                             │
         │    ├─ 截图（快，<100ms）                    │
         │    ├─ 取 UI Tree（快，<200ms）              │
         │    ├─ OCR（不做——太重，不适合每次操作）      │
         │                                             │
         ├─  execute(action)                           │
         │                                             │
         ├─  post_snapshot()                           │
         │    ├─ 截图 + tree                           │
         │                                             │
         ├─  compare(pre, post)                        │
         │    ├─ tree_diff: 哪些元素变了               │
         │    └─ pixel_diff: 哪些区域变了              │
         │                                             │
         └─  diff 附在 tool result 返回给 LLM ──→ 决定 │
                    │                                 │
                    └── LLM 判断是否需要重试 ──────────┘
```

**核心原则**：
- 验证循环不做 OCR（太慢，5-10 秒一次）
- 验证循环只做截图 diff + tree diff（<1 秒）
- OCR 作为独立工具让 LLM 按需调用

### 三个入口：显式 + 隐式 + 按需

#### ① tool: `screen_state`（显式感知）

```json
// LLM 想知道当前屏幕状态
screen_state()
→ {
    screenshot: "base64...",
    ui_tree: { root: { role: "window", children: [...] } },
    ocr_text: "Are you sure?\nOK  Cancel",
  }
```

用途：**初始感知**。agent 第一次操作前、遇到意外结果时，主动调用完整了解屏幕。

#### ② 透明层：`execute_and_verify`（隐式验证）

```
LLM 发 click("OK")
  → 自动做 pre_snapshot（只截图 + tree，不做 OCR）
  → 执行 click
  → 自动做 post_snapshot
  → tree_diff + pixel_diff
  → LLM 拿到的 tool result 末尾附带 diff summary
```

用途：**操作验证**。不需要额外 token，每次操作自然知道有没有生效。

#### ③ tool: `screen_ocr`（按需读取）

```json
// LLM 在截图里看到疑似文字，但 UI tree 没有（如 PDF 里的文字）
screen_ocr(region: { x: 100, y: 200, w: 400, h: 50 })
→ { text: "Server Error: Connection refused" }
```

用途：**精准读取**。OCR 开销大，只在需要的时候按区域跑，不做全屏识别。

### 发给 LLM 的 Tool Result 格式

```
Action: click(button "OK")
Result: executed
Verification:
  - pixel diff: dialog region changed (area 100,140-300,300)
  - tree diff:
      · button "OK" → enabled: true → false
      · staticText "Are you sure?" → REMOVED
  - OCR: dialog text is no longer visible on screen
→ Action appears to have succeeded (confirmation dialog dismissed)
```

LLM 只看紧凑的 diff summary 就能判断。如果 diff 显示没生效，LLM 自然尝试其他方案。

### 为什么不在工具内部自动重试

```rust
// ❌ 不推荐
for i in 0..3 {
    execute(action);
    if verify() { break; }
}
```

这样 LLM 不知道发生了什么，也学不到经验。应该**把验证信息返回给 LLM**，让 LLM 学到"原来这种弹窗需要先关掉才能点 OK"。下一次遇到类似情况，LLM 可以直接处理，不需要重试。

验证循环的目的是**提供反馈**，不是**代劳决策**。

---

## 六、UI 树增强

### 当前（扁平的）

```
role=button, name="OK", position={100,200}, size={80,30}
role=button, name="Cancel", position={190,200}, size={80,30}
role=staticText, name="Are you sure?", position={100,150}, size={150,20}
```

### 目标（树形）

```
role=dialog, name="Confirm", position={90,140}
  ├─ staticText, "Are you sure?"
  ├─ button, "OK", enabled=true, highlighted=false, hasFocus=false
  └─ button, "Cancel", enabled=true, highlighted=false, hasFocus=true
```

LLM 从树的层级结构就能理解"这是一个确认弹窗，Cancel 有焦点，说明按回车会取消"，不需要截图。

macOS System Events 的 AppleScript 可以递归取子元素，改动主要在 `accessibility.rs` 的 parser 部分，将扁平存储改为树结构存储 + 序列化时保留缩进层级。

---

## 七、代码改动估算

| 模块 | 文件 | 行数 | 依赖 |
|------|------|------|------|
| ScreenState | `src/computer/vision/screen_state.rs` | ~80 | 复用 accessibility + screenshot |
| Tree diff | 同上 | ~60 | 已有 UiNode，递归比较 |
| Pixel diff | 同上 | ~40 | `image` crate 已依赖 |
| OCR | `src/computer/vision/ocr.rs` | ~300 | `ort` 已依赖 |
| Verify wrap | `platform_macos.rs` 修改 | ~50 | 在 execute() 加分支 |
| 树形 UI | `accessibility.rs` 修改 | ~80 | 无新增依赖 |
| **合计** | | **~610** | |

---

## 八、实现优先级

| 优先级 | 功能 | 行数 | 效果 |
|--------|------|------|------|
| P0 | 树形 UI（结构感知） | ~80 | LLM 理解 UI 结构更准，无新增依赖 |
| P0 | 验证循环（操作反馈） | ~230 | 操作可靠性大幅提升，已有依赖 |
| P1 | 窗口管理增强 | ~150 | agent 能整理桌面，截图更有意义 |
| P1 | 按需 OCR | ~300 | agent 能看到截图里的文字 |
| P2 | 快捷键系统增强 | ~100 | 操作 IDE 和系统级快捷键 |
| P3 | 多桌面 / Spaces | ~120 | 多工作区管理 |

---

## 九、跨平台分析

ScreenState 各组件在不同平台的通用性：

```
┌─────────────────────────────────────┐
│   ScreenState + Verification Loop    │ ← 完全通用，写一次
│   (截屏→对比→返回 diff)              │
├─────────────────────────────────────┤
│   OCR + Pixel Diff                  │ ← 通用（image crate + ONNX）
│   (图片处理、文字识别、逐块比较)      │
├─────────────────────────────────────┤
│   截图 + UI Tree 采集                │ ← 平台相关，各平台不同
│   (platform_macos / windows / linux) │
└─────────────────────────────────────┘
```

### 第一层：完全通用（0 平台依赖）

| 组件 | 说明 | 实现方式 |
|------|------|----------|
| **Verification Loop** | pre/post 编排逻辑 | 纯 Rust 控制流，写一次 |
| **ScreenDiff** | tree diff + pixel diff 生成 | 无平台依赖 |
| **Pixel diff** | 两张图片逐块比较 | `image` crate，跨平台 |
| **Diff summary 格式化** | 转成 LLM 易读的文本 | 纯字符串处理 |

这些写一次，所有平台自动受益。

### 第二层：通用但依赖平台数据

| 组件 | 说明 | 跨平台情况 |
|------|------|------------|
| **OCR** | 截图上跑文字识别 | **完全通用**。ONNX Runtime (`ort` crate) 是跨平台的，模型文件同一份。macOS/Linux/Windows 用同一个模型 |
| **树形 UI Tree 结构** | 扁平→树形转换 | **半通用**。每个平台 accessibility API 返回格式不同，但转成统一 `UiNode` 结构后，上层的 diff 逻辑是通用的 |

### 第三层：平台相关（各平台单独写）

| 平台 | 截图 | UI Tree 采集 |
|------|------|-------------|
| **macOS** | `screencapture` 命令行（已有） | AXUIElement via AppleScript（已有） |
| **Windows** | 已有实现 | UIAutomation（已有） |
| **Linux** | 已有实现 | AT-SPI / xdotool（已有） |

每个平台的 `ComputerAdapter` 已经实现了 `screenshot()` 和 `read_ui_tree()`。ScreenState 只需要调用这两个接口，不需要关心底层实现。

### 跨平台代码量汇总

| 模块 | 行数 | 跨平台情况 |
|------|------|-----------|
| OCR（`src/computer/vision/ocr.rs`） | ~300 | **一次编写**，ONNX 跨平台 |
| Tree diff + Pixel diff | ~100 | **一次编写** |
| ScreenState capture | ~80 | **一次编写**，底层调 `ComputerAdapter` |
| Verification Loop 编排 | ~50 | **一次编写** |
| 树形 UI: macOS | ~40 | 只改 `accessibility.rs` parser |
| 树形 UI: Windows | ~40 | 只改 `platform_windows.rs` |
| 树形 UI: Linux | ~40 | 只改 `platform_linux.rs` |
| **合计** | **~650** | **~530 行通用 + ~120 行三平台分摊** |

核心功能（ScreenState + Verification Loop + OCR）一次编写全平台可用。只有 UI Tree 采集需要少量平台适配，而这些已经在 `ComputerAdapter` trait 里抽象好了。
