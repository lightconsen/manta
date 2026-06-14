# Computer Module

Cross-platform unified desktop interface for computer use and desktop automation.

## Design

This layer sits above `CapabilitySet` + `ToolRegistry` and provides a platform-agnostic API for screenshots, UI-tree reading, and desktop actions. The LLM still interacts with individual `Tool`s via `ToolRegistry`; `ComputerAdapter` is consumed by the higher-level `GoalPlanner` and `ComputerUseLoop`.

```
Agent / Planner
      │
ComputerAdapter  ←── Unified interface
      │
ToolRegistry ──→ CapabilitySet ──→ xdotool / SendKeys / AXUIElement
```

- **`ComputerAdapter` trait** — Cross-platform adapter for desktop perception and action
- **`types.rs`** — Core types: `DesktopAction`, `Screenshot`, `UiElement`, `Rect`, `Point`, etc.
- **`system.rs`** — System information gathering
- **`verification.rs`** — `VerificationEngine` with `VerificationCriteria` for task success checking
- **`use_loop.rs`** — `ComputerUseLoop` with `LoopDecision` parsing for iterative desktop interaction
- **`rollback.rs`** — `RollbackManager` with snapshot-based state restoration
- **`headless.rs`** — `HeadlessComputerAdapter` with virtual display support
- **`fs_watch.rs`** — `FileWatcher` for directory and file change monitoring
- **`network.rs`** — `NetworkInspector` for port scanning and firewall inspection
- **`log_aggregator.rs`** — `LogAggregator` for system log collection and alerting
- **`audio.rs`** — `AudioCapture` for audio event detection
- **`screen_recorder.rs`** — `ScreenRecorder` for video capture
- **`screenshot_encoder.rs`** — Screenshot encoding utilities
- **`sensitive_ui.rs`** — Sensitive UI element detection and masking
- **`remote_control.rs`** — `RemoteControlAdapter` for SSH/VNC/RDP remote machines
- **`reflection.rs`** — Self-reflection and state introspection
- **`vision.rs`** — Computer vision integration (behind `vision` feature)

### Platform Adapters

| Platform | Module | Backend |
|----------|--------|---------|
| macOS | `platform_macos.rs` | AXUIElement, AppleScript |
| Windows | `platform_windows.rs` | UIAutomation, SendKeys |
| Linux | `platform_linux.rs` | xdotool, AT-SPI |

## Key Types

```rust
#[async_trait]
pub trait ComputerAdapter: Send + Sync {
    async fn screenshot(&self, region: Option<Rect>) -> Result<Screenshot>;
    async fn read_ui_tree(&self, app: Option<&str>) -> Result<Vec<UiElement>>;
    async fn execute(&self, action: DesktopAction) -> Result<ActionResult>;
    async fn wait_for(&self, condition: WaitCondition, timeout: Duration) -> Result<bool>;
    async fn click_at(&self, point: Point, button: MouseButton) -> Result<ActionResult>;
    async fn type_text(&self, text: &str) -> Result<ActionResult>;
    async fn key_press(&self, keys: Vec<String>) -> Result<ActionResult>;
    async fn clipboard_get(&self) -> Result<String>;
    async fn clipboard_set(&self, text: &str) -> Result<ActionResult>;
    async fn watch_directory(&self, path: &str) -> Result<ActionResult>;
    async fn unwatch_directory(&self, path: &str) -> Result<ActionResult>;
    async fn list_ports(&self, filter_protocol: Option<&str>, filter_state: Option<&str>) -> Result<ActionResult>;
}

pub enum DesktopAction {
    Screenshot { region: Option<Rect> },
    Click { target: ClickTarget, button: MouseButton },
    Type { text: String },
    KeyPress { keys: Vec<String> },
    Scroll { direction: ScrollDirection, amount: i32 },
    Wait { milliseconds: u64 },
    ClipboardGet,
    ClipboardSet { text: String },
    WatchDirectory { path: String },
    UnwatchDirectory { path: String },
    WatchFile { path: String },
    UnwatchFile { path: String },
}

pub enum LoopDecision {
    Done { message: String },
    NeedHelp { reason: String },
    Action(DesktopAction),
}
```

## Data Flow

```
User Request (desktop task)
    │
    ▼
is_desktop_task() heuristic
    │
    ▼
ComputerUseLoop::run()
    │
    ├──▶ screenshot() ──▶ LLM analysis
    ├──▶ read_ui_tree() ──▶ LLM analysis
    ├──▶ execute(action) ──▶ Platform adapter
    │       │
    │       ├──▶ macOS → AXUIElement
    │       ├──▶ Windows → UIAutomation
    │       └──▶ Linux → xdotool
    │
    └──▶ verification engine ──▶ success check
```

## Implemented Features

- Cross-platform `ComputerAdapter` trait (macOS, Windows, Linux)
- Screenshot capture with optional region selection
- Accessibility UI tree reading
- Desktop actions: click, type, key press, scroll, wait, clipboard
- File and directory watching
- Network port listing and inspection
- Computer use loop with iterative LLM-driven desktop interaction
- Loop decision parsing (`DONE:`, `HELP:`, `ACTION:` prefixes)
- Verification engine with configurable criteria
- Rollback manager with snapshot-based restoration
- Headless mode with virtual display support
- Remote control via SSH/VNC/RDP
- System log aggregation with alert rules
- Audio capture and event detection
- Screen recording with video frame capture
- Sensitive UI element detection and masking
- Computer vision integration (feature-gated)

