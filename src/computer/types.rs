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
