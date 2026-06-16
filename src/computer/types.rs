//! Cross-platform types for Physical AI desktop abstraction.
//!
//! These types hide platform differences (xdotool vs SendKeys vs AXUIElement)
//! behind a unified interface used by the Agent / Planner layers.

use serde::{Deserialize, Serialize};

/// A point on screen in logical coordinates (0-1920 range, DPI independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Scale this point by the given DPI factor (logical → physical).
    pub fn to_physical(&self, dpi_scale: f32) -> Self {
        Self {
            x: (self.x as f32 * dpi_scale).round() as i32,
            y: (self.y as f32 * dpi_scale).round() as i32,
        }
    }

    /// Scale this point by the given DPI factor (physical → logical).
    pub fn to_logical(&self, dpi_scale: f32) -> Self {
        Self {
            x: (self.x as f32 / dpi_scale).round() as i32,
            y: (self.y as f32 / dpi_scale).round() as i32,
        }
    }
}

/// Platform DPI scale factor for converting between logical and physical
/// coordinates.
///
/// Logical coordinates are what the Agent reasons about (e.g., "click at
/// 960, 540" on a 1920x1080 logical canvas). Physical coordinates are the
/// actual pixels on screen, which differ on HiDPI / Retina displays.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DpiScale {
    /// Scale factor: physical = logical × scale.
    /// 1.0 = standard DPI (96 on Windows, 72 on macOS).
    /// 2.0 = Retina / HiDPI.
    pub scale: f32,
}

impl DpiScale {
    /// Standard 1:1 scale (no HiDPI).
    pub const STANDARD: Self = Self { scale: 1.0 };

    pub fn new(scale: f32) -> Self {
        Self { scale }
    }

    /// Detect the current platform's DPI scale at runtime.
    ///
    /// Returns a best-effort estimate. On some platforms this may
    /// require display-server queries (X11, Wayland, macOS, Windows).
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            // macOS: try via CoreGraphics display scale
            Self::detect_macos()
        }
        #[cfg(target_os = "windows")]
        {
            Self::detect_windows()
        }
        #[cfg(target_os = "linux")]
        {
            Self::detect_linux()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Self::STANDARD
        }
    }

    #[cfg(target_os = "macos")]
    fn detect_macos() -> Self {
        // macOS Retina: typically 2.0, can be 1.0 on external displays.
        // Without linking CoreGraphics, we shell out to `system_profiler`.
        if let Ok(output) = std::process::Command::new("system_profiler")
            .args(["SPDisplaysDataType", "-json"])
            .output()
        {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(displays) = json
                    .get("SPDisplaysDataType")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("spdisplays_ndrvs"))
                    .and_then(|v| v.as_array())
                {
                    if let Some(first) = displays.first() {
                        if let Some(res) = first.get("_spdisplays_resolution") {
                            if let Some(res_str) = res.as_str() {
                                // "Retina" in the resolution string indicates HiDPI
                                if res_str.contains("Retina") {
                                    return Self { scale: 2.0 };
                                }
                            }
                        }
                        if let Some(pixels) = first.get("_spdisplays_pixels") {
                            if let Some(px_str) = pixels.as_str() {
                                // e.g. "3840 x 2160" vs "1920 x 1080"
                                let parts: Vec<&str> = px_str.split(" x ").collect();
                                if parts.len() == 2 {
                                    if let (Ok(px_w), Ok(px_h)) = (
                                        parts[0].trim().parse::<u32>(),
                                        parts[1].trim().parse::<u32>(),
                                    ) {
                                        // Rough heuristic: if pixel dimensions are
                                        // double common logical sizes, assume 2x
                                        if (px_w >= 3840 && px_h >= 2160)
                                            || (px_w >= 2880 && px_h >= 1800)
                                        {
                                            return Self { scale: 2.0 };
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Self::STANDARD
    }

    #[cfg(target_os = "windows")]
    fn detect_windows() -> Self {
        // Windows: try via PowerShell Get-CimInstance
        if let Ok(output) = std::process::Command::new("powershell")
            .args([
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Screen]::PrimaryScreen.DeviceDpi",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(dpi) = text.trim().parse::<f32>() {
                if dpi > 0.0 {
                    return Self {
                        scale: dpi / 96.0,
                    };
                }
            }
        }
        Self::STANDARD
    }

    #[cfg(target_os = "linux")]
    fn detect_linux() -> Self {
        // Try X11 via xdpyinfo
        if let Ok(output) = std::process::Command::new("xdpyinfo").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut screen_w_mm = 0f32;
            let mut screen_w_px = 0f32;
            let mut found = false;
            for line in text.lines() {
                if line.contains("dimensions:") {
                    // "  dimensions:    3840x2160 pixels (1016x572 millimeters)"
                    if let Some(px_part) = line.split("pixels").next() {
                        if let Some(x) = px_part.split('x').next() {
                            if let Ok(w) = x.trim().split_whitespace().last().unwrap_or("0").parse::<f32>() {
                                screen_w_px = w;
                            }
                        }
                    }
                    if let Some(mm_part) = line.split('(').nth(1) {
                        if let Some(x) = mm_part.split('x').next() {
                            if let Ok(w) = x.trim().parse::<f32>() {
                                screen_w_mm = w;
                                found = true;
                            }
                        }
                    }
                }
            }
            if found && screen_w_mm > 0.0 {
                // DPI = pixels / inches, inches = mm / 25.4
                let dpi = (screen_w_px / (screen_w_mm / 25.4)).round();
                if dpi > 96.0 {
                    return Self {
                        scale: (dpi / 96.0).max(1.0).min(4.0),
                    };
                }
            }
        }

        // Try Wayland via gsettings (GNOME)
        if let Ok(output) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "scaling-factor"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(scale) = text.trim().replace("uint32 ", "").parse::<f32>() {
                if scale >= 1.0 {
                    return Self { scale };
                }
            }
        }

        Self::STANDARD
    }
}

/// A rectangle on screen in logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Scale this rect by the given DPI factor (logical → physical).
    pub fn to_physical(&self, dpi_scale: f32) -> Self {
        Self {
            x: (self.x as f32 * dpi_scale).round() as i32,
            y: (self.y as f32 * dpi_scale).round() as i32,
            width: (self.width as f32 * dpi_scale).round() as u32,
            height: (self.height as f32 * dpi_scale).round() as u32,
        }
    }

    /// Scale this rect by the given DPI factor (physical → logical).
    pub fn to_logical(&self, dpi_scale: f32) -> Self {
        Self {
            x: (self.x as f32 / dpi_scale).round() as i32,
            y: (self.y as f32 / dpi_scale).round() as i32,
            width: (self.width as f32 / dpi_scale).round() as u32,
            height: (self.height as f32 / dpi_scale).round() as u32,
        }
    }
}

/// A UI element as seen by the accessibility / UI automation layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiElement {
    /// Platform-specific unique identifier (may be empty if unavailable).
    pub id: String,
    /// Role: "button", "text_field", "window", "menu", "checkbox", etc.
    pub role: String,
    /// Human-readable label / text.
    pub label: Option<String>,
    /// Current value (for inputs, checkboxes, sliders).
    pub value: Option<String>,
    /// Bounding box in logical coordinates.
    pub bounds: Rect,
    /// Whether the element is enabled for interaction.
    pub enabled: bool,
    /// Whether the element currently has keyboard focus.
    pub focused: bool,
    /// Child elements (tree structure).
    pub children: Vec<UiElement>,
}

/// Parse a UI element from accessibility tool JSON output.
/// Handles the format produced by platform accessibility tools:
/// `{ role, name, value, enabled, x, y, width, height, children }`.
pub fn ui_element_from_accessibility_json(value: &serde_json::Value) -> Option<UiElement> {
    let obj = value.as_object()?;
    let role = obj.get("role")?.as_str()?.to_string();
    let name = obj.get("name")?.as_str().unwrap_or("").to_string();
    let x = obj.get("x")?.as_i64()? as i32;
    let y = obj.get("y")?.as_i64()? as i32;
    let width = obj.get("width")?.as_i64()? as u32;
    let height = obj.get("height")?.as_i64()? as u32;
    let enabled = obj.get("enabled")?.as_bool().unwrap_or(true);
    let value = obj.get("value").and_then(|v| v.as_str()).map(String::from);

    let children = obj
        .get("children")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(ui_element_from_accessibility_json).collect())
        .unwrap_or_default();

    Some(UiElement {
        id: String::new(),
        role,
        label: if name.is_empty() { None } else { Some(name) },
        value,
        bounds: Rect::new(x, y, width, height),
        enabled,
        focused: false,
        children,
    })
}

/// Parse a list of UI elements from accessibility tool result data.
pub fn parse_accessibility_elements(data: Option<&serde_json::Value>) -> Vec<UiElement> {
    data.and_then(|d| d.get("elements"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(ui_element_from_accessibility_json).collect())
        .unwrap_or_default()
}

impl UiElement {
    /// Find the first element matching the given role (depth-first).
    pub fn find_by_role(&self, role: &str) -> Option<&UiElement> {
        if self.role == role {
            return Some(self);
        }
        for child in &self.children {
            if let found @ Some(_) = child.find_by_role(role) {
                return found;
            }
        }
        None
    }

    /// Find the first element whose label contains the given text.
    pub fn find_by_label_contains(&self, text: &str) -> Option<&UiElement> {
        if self
            .label
            .as_ref()
            .map(|l| l.contains(text))
            .unwrap_or(false)
        {
            return Some(self);
        }
        for child in &self.children {
            if let found @ Some(_) = child.find_by_label_contains(text) {
                return found;
            }
        }
        None
    }

    /// Compute the center point of this element's bounds.
    pub fn center(&self) -> Point {
        Point::new(
            self.bounds.x + self.bounds.width as i32 / 2,
            self.bounds.y + self.bounds.height as i32 / 2,
        )
    }
}

/// A captured screenshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screenshot {
    /// Base64-encoded PNG image.
    pub base64: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Wall-clock capture timestamp. Skipped in serialization since Instant is not
    /// natively supported by serde.
    #[serde(skip, default = "instant_now")]
    pub timestamp: std::time::Instant,
}

/// Serde helper: default value for `#[serde(skip)]` timestamp fields.
fn instant_now() -> std::time::Instant {
    std::time::Instant::now()
}

/// Mouse button for click / drag actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Scroll direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

/// How to locate a click target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickTarget {
    /// Absolute logical coordinate.
    Coordinate(Point),
    /// Platform-specific accessibility element ID.
    ElementId(String),
    /// Find element by label text (partial match).
    ElementLabel(String),
    /// Find element by role, then by label.
    ElementRoleLabel { role: String, label: String },
}

/// A desktop action the agent wants to perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopAction {
    /// Capture a screenshot (optionally of a sub-region).
    Screenshot {
        region: Option<Rect>,
    },
    /// Click somewhere on screen.
    Click {
        target: ClickTarget,
        button: MouseButton,
    },
    /// Double-click.
    DoubleClick {
        target: ClickTarget,
        button: MouseButton,
    },
    /// Type text (as if typed by keyboard).
    Type {
        text: String,
    },
    /// Press key combination (e.g. ["ctrl", "c"]).
    KeyPress {
        keys: Vec<String>,
    },
    /// Scroll at a location.
    Scroll {
        target: ClickTarget,
        direction: ScrollDirection,
        amount: i32,
    },
    /// Drag from one point to another.
    Drag {
        from: ClickTarget,
        to: ClickTarget,
    },
    /// Read the accessibility UI tree of the active (or named) app.
    ReadUiTree {
        app: Option<String>,
    },
    /// Launch an application.
    LaunchApp {
        name: String,
        args: Vec<String>,
        /// Wait for the app to be ready (window appears).
        wait_for_ready: bool,
    },
    /// Activate / focus a window by title pattern.
    ActivateWindow {
        title_pattern: String,
    },
    /// Close a window by title pattern.
    CloseWindow {
        title_pattern: String,
    },
    /// Wait for a duration.
    Wait {
        milliseconds: u64,
    },
    /// Read clipboard.
    ClipboardGet,
    /// Write to clipboard.
    ClipboardSet {
        text: String,
    },
    /// Query system resource status (CPU, memory, disk, network, uptime).
    GetSystemStatus,
    /// List running processes, optionally filtered by name.
    ListProcesses {
        filter: Option<String>,
        limit: Option<usize>,
    },
    /// Kill a process by PID or name.
    KillProcess {
        pid: Option<u32>,
        name: Option<String>,
        force: bool,
    },
    /// Watch a directory for changes.
    WatchDirectory {
        path: String,
    },
    /// Stop watching a directory.
    UnwatchDirectory {
        path: String,
    },
    /// Watch a single file for changes.
    WatchFile {
        path: String,
    },
    /// Stop watching a single file.
    UnwatchFile {
        path: String,
    },
    /// List network sockets (ports) with optional filters.
    ListPorts {
        filter_protocol: Option<String>,
        filter_state: Option<String>,
    },
    /// Test ICMP connectivity to a host.
    TestPing {
        target: String,
        count: Option<u32>,
    },
    /// Test TCP connectivity to a host:port.
    TestTcpConnect {
        target: String,
        port: u16,
        timeout_ms: Option<u64>,
    },
    /// List firewall rules.
    ListFirewallRules,
    /// Restart a process by PID or name.
    RestartProcess {
        pid: Option<u32>,
        name: Option<String>,
        force: bool,
    },
    /// Set process priority (nice value on Unix, priority class on Windows).
    SetProcessPriority {
        pid: Option<u32>,
        name: Option<String>,
        /// Unix nice value: -20 (highest) to 19 (lowest).
        /// Windows priority class: 0=Idle, 1=BelowNormal, 2=Normal, 3=AboveNormal, 4=High, 5=Realtime.
        priority: i32,
    },
    /// Press a sequence of keys with configurable delays between them.
    ///
    /// Useful for IDE multi-step shortcuts or timed interactions.
    KeySequence {
        keys: Vec<String>,
        /// Delay in milliseconds before each key (first element = delay before first key).
        /// If shorter than keys, the last delay is repeated.
        delays_ms: Vec<u64>,
    },
    /// Install software packages using the platform's package manager.
    InstallPackage {
        manager: PackageManager,
        packages: Vec<String>,
        timeout_secs: u64,
    },
    /// Browse files in a directory with optional natural-language filtering.
    BrowseFiles {
        path: String,
        /// Optional filter description (e.g. "recently modified logs", "largest files").
        filter_description: Option<String>,
        max_results: Option<usize>,
    },
    /// Read a chunk of a potentially large file.
    ReadFileChunked {
        path: String,
        /// Byte offset to start reading from.
        offset: u64,
        /// Maximum bytes to read.
        limit_bytes: u64,
    },
    /// Search and replace text in a file.
    EditFile {
        path: String,
        search: String,
        replace: String,
    },
    /// Compress files/directories into an archive.
    Compress {
        sources: Vec<String>,
        destination: String,
        format: CompressionFormat,
    },
    /// Decompress an archive.
    Decompress {
        archive: String,
        destination: String,
    },
    /// Transfer a file to/from a remote host.
    TransferFile {
        source: String,
        destination: String,
        method: TransferMethod,
    },
    /// Call a tool by name in the ToolRegistry (e.g. device tools).
    ///
    /// Used by the GoalPlanner executor to dispatch device operations
    /// and other tool-registered capabilities as plan steps.
    ToolCall {
        tool_name: String,
        args: serde_json::Value,
    },
}

/// Package manager supported for software installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    /// macOS Homebrew.
    Brew,
    /// Debian/Ubuntu APT.
    Apt,
    /// Fedora DNF.
    Dnf,
    /// Arch pacman.
    Pacman,
    /// Alpine apk.
    Apk,
    /// Windows winget.
    Winget,
    /// Windows Chocolatey.
    Choco,
    /// macOS MacPorts.
    Macports,
}

/// Archive compression format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionFormat {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    SevenZ,
}

/// File transfer method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferMethod {
    Scp,
    Rsync,
    Smb,
}

/// A file entry returned by BrowseFiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub modified_secs: u64,
    pub is_directory: bool,
}

/// Result of executing a desktop action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub success: bool,
    pub message: String,
    /// Screenshot taken after the action (for verification).
    pub screenshot_after: Option<Screenshot>,
    /// Structured data (e.g. UI tree after the action).
    pub data: Option<serde_json::Value>,
}

impl ActionResult {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            screenshot_after: None,
            data: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            screenshot_after: None,
            data: None,
        }
    }

    pub fn with_screenshot(mut self, screenshot: Screenshot) -> Self {
        self.screenshot_after = Some(screenshot);
        self
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// Condition to wait for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitCondition {
    /// Wait for an element with the given role to appear in the UI tree.
    UiTreeContains { role: String, label: Option<String> },
    /// Wait for the window title to contain a pattern.
    WindowTitleContains { pattern: String },
    /// Wait for a process with the given name to be running.
    ProcessRunning { name: String },
    /// Wait for a process with the given name to exit.
    ProcessExited { name: String },
    /// Wait for the screenshot to differ from the baseline by less than a threshold.
    ScreenshotStable { max_pixel_diff: u32, timeout_ms: u64 },
    /// Wait for a file to appear.
    FileExists { path: String },
}

/// Cross-platform system resource snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub uptime_seconds: u64,
    pub cpu_usage_percent: f32,
    pub cpu_count: usize,
    pub memory_total_mb: u64,
    pub memory_used_mb: u64,
    pub memory_available_mb: u64,
    pub swap_total_mb: u64,
    pub swap_used_mb: u64,
    pub disks: Vec<DiskStatus>,
    pub networks: Vec<NetworkStatus>,
    /// Wall-clock snapshot timestamp. Skipped in serialization since Instant is not
    /// natively supported by serde.
    #[serde(skip, default = "instant_now")]
    pub timestamp: std::time::Instant,
}

/// Disk usage information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskStatus {
    pub name: String,
    pub mount_point: String,
    pub total_gb: f64,
    pub used_gb: f64,
    pub available_gb: f64,
}

/// Network interface statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStatus {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// A single running process entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_mb: u64,
    pub status: String,
    pub start_time: Option<String>,
}
