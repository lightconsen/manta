//! Tool wrapper around [`ComputerAdapter`] exposing desktop automation
//! operations as standard Tool trait implementations.
//!
//! The LLM calls `computer` with an `action` parameter to perform screenshots,
//! clicks, typing, key presses, window management, and other desktop ops.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::computer::{
    ActionResult, ClickTarget, ComputerAdapter, ComputerError, DesktopAction, MouseButton, Point,
    Rect, ScrollDirection,
};
use crate::tools::{
    approval::RiskLevel, create_schema, sdk::ToolCapabilities, Tool, ToolContext,
    ToolExecutionResult,
};

/// Tool that exposes all [`ComputerAdapter`] operations to the LLM.
///
/// Uses the `action` enum pattern (like [`TimeTool`]) — each action maps to a
/// [`DesktopAction`] variant.  The tool reports `is_available=false` when no
/// adapter is configured, preventing the LLM from calling it.
pub struct ComputerTool {
    adapter: Option<Arc<dyn ComputerAdapter>>,
}

impl ComputerTool {
    pub fn new(adapter: Option<Arc<dyn ComputerAdapter>>) -> Self {
        Self { adapter }
    }
}

#[async_trait]
impl Tool for ComputerTool {
    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        r#"Perform desktop automation operations: capture screenshots, move and click the mouse, type text, press keyboard shortcuts, scroll, drag, manage windows, launch applications, read UI accessibility trees, query system status, list/kill processes, test network connectivity, list firewall rules, manage file watchers, and browse files.

Use the `action` parameter to choose the operation. Each action uses its own set of parameters — see the action enum descriptions for details.

Common workflows:
- "screenshot" — capture the screen (optionally a region)
- "click" — click at coordinates or on a UI element
- "type" — type text at the current cursor position
- "key_press" — press keyboard shortcuts like ["ctrl","c"]
- "read_ui_tree" — inspect the accessibility tree of the active window"#
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Desktop automation operations",
            json!({
                "action": {
                    "type": "string",
                    "enum": [
                        "screenshot", "click", "double_click", "type", "key_press",
                        "scroll", "drag", "read_ui_tree", "launch_app",
                        "activate_window", "close_window", "wait",
                        "clipboard_get", "clipboard_set", "get_system_status",
                        "list_processes", "kill_process", "list_ports",
                        "test_ping", "test_tcp_connect", "list_firewall_rules",
                        "restart_process", "set_process_priority", "key_sequence",
                        "install_package", "browse_files"
                    ],
                    "description": "The desktop operation to perform"
                },
                // ── Common positional parameters ──────────────────────────
                "x": { "type": "integer", "description": "X coordinate (for click, double_click, scroll, drag_from, drag_to)" },
                "y": { "type": "integer", "description": "Y coordinate" },
                "text": { "type": "string", "description": "Text to type, or clipboard content to set" },
                "keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Key names to press (e.g. [\"ctrl\",\"c\"], [\"cmd\",\"space\"], [\"enter\"])"
                },
                "button": {
                    "type": "string",
                    "enum": ["left", "middle", "right"],
                    "description": "Mouse button for click/double_click"
                },
                // ── Region / scroll ───────────────────────────────────────
                "region_x": { "type": "integer", "description": "Screenshot region left coordinate" },
                "region_y": { "type": "integer", "description": "Screenshot region top coordinate" },
                "region_width": { "type": "integer", "description": "Screenshot region width" },
                "region_height": { "type": "integer", "description": "Screenshot region height" },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction"
                },
                "amount": { "type": "integer", "description": "Scroll amount (clicks or pixels)" },
                // ── Window / app ──────────────────────────────────────────
                "title_pattern": { "type": "string", "description": "Window title pattern to match (activate_window, close_window)" },
                "app_name": { "type": "string", "description": "Application name to launch" },
                "app_args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command-line arguments for the application"
                },
                "wait_for_ready": { "type": "boolean", "description": "Wait for the app window to appear before returning" },
                // ── Process ───────────────────────────────────────────────
                "filter": { "type": "string", "description": "Process name filter (list_processes)" },
                "pid": { "type": "integer", "description": "Process ID (kill_process, restart_process, set_process_priority)" },
                "name": { "type": "string", "description": "Process name (kill_process, restart_process, set_process_priority)" },
                "force": { "type": "boolean", "description": "Force kill (kill_process, restart_process)" },
                "priority": { "type": "integer", "description": "Priority value (set_process_priority): Unix nice -20..19, Windows 0..5" },
                // ── Network ───────────────────────────────────────────────
                "target": { "type": "string", "description": "Hostname or IP for network tests" },
                "port": { "type": "integer", "description": "TCP port (test_tcp_connect)" },
                "count": { "type": "integer", "description": "Ping count (test_ping)" },
                "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds" },
                "protocol": { "type": "string", "description": "Protocol filter for list_ports (e.g. tcp, udp)" },
                "state": { "type": "string", "description": "State filter for list_ports (e.g. listen, established)" },
                // ── Wait ──────────────────────────────────────────────────
                "milliseconds": { "type": "integer", "description": "Duration to wait in milliseconds" },
                // ── Key sequence ──────────────────────────────────────────
                "delays_ms": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "Per-key delays in milliseconds (key_sequence)"
                },
                // ── Package install ───────────────────────────────────────
                "packages": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Package names to install"
                },
                "package_manager": {
                    "type": "string",
                    "enum": ["brew", "apt", "dnf", "pacman", "apk", "winget", "choco", "macports"],
                    "description": "Package manager to use"
                },
                // ── Browse files ──────────────────────────────────────────
                "path": { "type": "string", "description": "Directory path for browse_files" },
                "max_results": { "type": "integer", "description": "Maximum number of file entries to return" },
                // ── Drag sub-params ───────────────────────────────────────
                "from_x": { "type": "integer", "description": "Drag start X" },
                "from_y": { "type": "integer", "description": "Drag start Y" },
                "to_x": { "type": "integer", "description": "Drag end X" },
                "to_y": { "type": "integer", "description": "Drag end Y" }
            }),
            vec!["action"],
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            risk_level: RiskLevel::Medium,
            categories: vec!["computer".to_string(), "desktop".to_string()],
            ..Default::default()
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        self.adapter.is_some()
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let adapter = self.adapter.as_ref().ok_or_else(|| {
            crate::error::SyscityError::Unsupported(
                "Computer adapter is not configured".to_string(),
            )
        })?;

        let action = args["action"]
            .as_str()
            .ok_or_else(|| crate::error::SyscityError::Validation("Missing action".to_string()))?;

        let desktop_action = action_to_desktop_action(action, &args)?;

        let result = adapter
            .execute(desktop_action)
            .await
            .map_err(to_syscity_err)?;
        Ok(action_result_to_tool_result(result))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn action_to_desktop_action(action: &str, args: &Value) -> crate::Result<DesktopAction> {
    match action {
        "screenshot" => {
            let region = parse_region(args);
            Ok(DesktopAction::Screenshot { region })
        }
        "click" => {
            let target = parse_click_target(args)?;
            let button = parse_button(args);
            Ok(DesktopAction::Click { target, button })
        }
        "double_click" => {
            let target = parse_click_target(args)?;
            let button = parse_button(args);
            Ok(DesktopAction::DoubleClick { target, button })
        }
        "type" => {
            let text = args["text"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'text' for type action".to_string(),
                    )
                })?
                .to_string();
            Ok(DesktopAction::Type { text })
        }
        "key_press" => {
            let keys = parse_string_array(args, "keys");
            Ok(DesktopAction::KeyPress { keys })
        }
        "scroll" => {
            let target = parse_click_target(args)?;
            let direction = match args["direction"].as_str() {
                Some("up") => ScrollDirection::Up,
                Some("down") => ScrollDirection::Down,
                Some("left") => ScrollDirection::Left,
                Some("right") => ScrollDirection::Right,
                _ => ScrollDirection::Down,
            };
            let amount = args["amount"].as_i64().unwrap_or(3) as i32;
            Ok(DesktopAction::Scroll { target, direction, amount })
        }
        "drag" => {
            let from_coords = (
                args["from_x"].as_i64().unwrap_or(0) as i32,
                args["from_y"].as_i64().unwrap_or(0) as i32,
            );
            let to_coords = (
                args["to_x"].as_i64().unwrap_or(0) as i32,
                args["to_y"].as_i64().unwrap_or(0) as i32,
            );
            let from = ClickTarget::Coordinate(Point::new(from_coords.0, from_coords.1));
            let to = ClickTarget::Coordinate(Point::new(to_coords.0, to_coords.1));
            Ok(DesktopAction::Drag { from, to })
        }
        "read_ui_tree" => {
            let app = args["app_name"].as_str().map(|s| s.to_string());
            Ok(DesktopAction::ReadUiTree { app })
        }
        "launch_app" => {
            let name = args["app_name"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'app_name' for launch_app action".to_string(),
                    )
                })?
                .to_string();
            let app_args = parse_string_array(args, "app_args");
            let wait_for_ready = args["wait_for_ready"].as_bool().unwrap_or(true);
            Ok(DesktopAction::LaunchApp {
                name,
                args: app_args,
                wait_for_ready,
            })
        }
        "activate_window" => {
            let title_pattern = args["title_pattern"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'title_pattern' for activate_window action".to_string(),
                    )
                })?
                .to_string();
            Ok(DesktopAction::ActivateWindow { title_pattern })
        }
        "close_window" => {
            let title_pattern = args["title_pattern"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'title_pattern' for close_window action".to_string(),
                    )
                })?
                .to_string();
            Ok(DesktopAction::CloseWindow { title_pattern })
        }
        "wait" => {
            let milliseconds = args["milliseconds"].as_u64().unwrap_or(1000);
            Ok(DesktopAction::Wait { milliseconds })
        }
        "clipboard_get" => Ok(DesktopAction::ClipboardGet),
        "clipboard_set" => {
            let text = args["text"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'text' for clipboard_set action".to_string(),
                    )
                })?
                .to_string();
            Ok(DesktopAction::ClipboardSet { text })
        }
        "get_system_status" => Ok(DesktopAction::GetSystemStatus),
        "list_processes" => {
            let filter = args["filter"].as_str().map(|s| s.to_string());
            let limit = args["max_results"].as_u64().map(|n| n as usize);
            Ok(DesktopAction::ListProcesses { filter, limit })
        }
        "kill_process" => {
            let pid = args["pid"].as_u64().map(|n| n as u32);
            let name = args["name"].as_str().map(|s| s.to_string());
            let force = args["force"].as_bool().unwrap_or(false);
            Ok(DesktopAction::KillProcess { pid, name, force })
        }
        "list_ports" => {
            let filter_protocol = args["protocol"].as_str().map(|s| s.to_string());
            let filter_state = args["state"].as_str().map(|s| s.to_string());
            Ok(DesktopAction::ListPorts { filter_protocol, filter_state })
        }
        "test_ping" => {
            let target = args["target"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'target' for test_ping action".to_string(),
                    )
                })?
                .to_string();
            let count = args["count"].as_u64().map(|n| n as u32);
            Ok(DesktopAction::TestPing { target, count })
        }
        "test_tcp_connect" => {
            let target = args["target"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'target' for test_tcp_connect action".to_string(),
                    )
                })?
                .to_string();
            let port = args["port"].as_u64().ok_or_else(|| {
                crate::error::SyscityError::Validation(
                    "Missing 'port' for test_tcp_connect action".to_string(),
                )
            })? as u16;
            let timeout_ms = args["timeout_ms"].as_u64();
            Ok(DesktopAction::TestTcpConnect { target, port, timeout_ms })
        }
        "list_firewall_rules" => Ok(DesktopAction::ListFirewallRules),
        "restart_process" => {
            let pid = args["pid"].as_u64().map(|n| n as u32);
            let name = args["name"].as_str().map(|s| s.to_string());
            let force = args["force"].as_bool().unwrap_or(false);
            Ok(DesktopAction::RestartProcess { pid, name, force })
        }
        "set_process_priority" => {
            let pid = args["pid"].as_u64().map(|n| n as u32);
            let name = args["name"].as_str().map(|s| s.to_string());
            let priority = args["priority"].as_i64().unwrap_or(0) as i32;
            Ok(DesktopAction::SetProcessPriority { pid, name, priority })
        }
        "key_sequence" => {
            let keys = parse_string_array(args, "keys");
            let delays_ms: Vec<u64> = args["delays_ms"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                .unwrap_or_default();
            Ok(DesktopAction::KeySequence { keys, delays_ms })
        }
        "install_package" => {
            use crate::computer::PackageManager;
            let manager = match args["package_manager"].as_str() {
                Some("brew") => PackageManager::Brew,
                Some("apt") => PackageManager::Apt,
                Some("dnf") => PackageManager::Dnf,
                Some("pacman") => PackageManager::Pacman,
                Some("apk") => PackageManager::Apk,
                Some("winget") => PackageManager::Winget,
                Some("choco") => PackageManager::Choco,
                Some("macports") => PackageManager::Macports,
                _ => {
                    // Auto-detect: default to apt on Linux, brew on macOS
                    #[cfg(target_os = "macos")]
                    {
                        PackageManager::Brew
                    }
                    #[cfg(target_os = "linux")]
                    {
                        PackageManager::Apt
                    }
                    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                    {
                        PackageManager::Apt
                    }
                }
            };
            let packages = parse_string_array(args, "packages");
            let timeout_secs = args["timeout_ms"].as_u64().unwrap_or(120);
            Ok(DesktopAction::InstallPackage {
                manager,
                packages,
                timeout_secs,
            })
        }
        "browse_files" => {
            let path = args["path"]
                .as_str()
                .ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "Missing 'path' for browse_files action".to_string(),
                    )
                })?
                .to_string();
            let filter_description = args["filter"].as_str().map(|s| s.to_string());
            let max_results = args["max_results"].as_u64().map(|n| n as usize);
            Ok(DesktopAction::BrowseFiles {
                path,
                filter_description,
                max_results,
            })
        }
        _ => Err(crate::error::SyscityError::Validation(format!(
            "Unknown computer action: {}",
            action
        ))),
    }
}

/// Parse an optional screenshot region from args.
fn parse_region(args: &Value) -> Option<Rect> {
    let x = args["region_x"].as_i64()?;
    let y = args["region_y"].as_i64()?;
    let w = args["region_width"].as_u64()?;
    let h = args["region_height"].as_u64()?;
    Some(Rect {
        x: x as i32,
        y: y as i32,
        width: w as u32,
        height: h as u32,
    })
}

/// Parse a click target from coordinates (x, y) or element parameters.
fn parse_click_target(args: &Value) -> crate::Result<ClickTarget> {
    // If x/y are provided, use coordinate target.
    if args.get("x").and_then(Value::as_i64).is_some()
        && args.get("y").and_then(Value::as_i64).is_some()
    {
        return Ok(ClickTarget::Coordinate(Point::new(
            args["x"].as_i64().unwrap_or(0) as i32,
            args["y"].as_i64().unwrap_or(0) as i32,
        )));
    }
    // Otherwise use element label if available.
    if let Some(label) = args["text"].as_str() {
        return Ok(ClickTarget::ElementLabel(label.to_string()));
    }
    // Fall back to coordinate (0, 0) — let the adapter handle it.
    Ok(ClickTarget::Coordinate(Point::new(0, 0)))
}

/// Parse mouse button from args.
fn parse_button(args: &Value) -> MouseButton {
    match args["button"].as_str() {
        Some("middle") => MouseButton::Middle,
        Some("right") => MouseButton::Right,
        _ => MouseButton::Left,
    }
}

/// Parse a string array from args key.
fn parse_string_array(args: &Value, key: &str) -> Vec<String> {
    args[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Convert an [`ActionResult`] to a [`ToolExecutionResult`].
fn action_result_to_tool_result(result: ActionResult) -> ToolExecutionResult {
    let mut output = result.message.clone();
    let mut data = result.data.clone();

    // If there's a screenshot, include base64 data and dimensions.
    if let Some(screenshot) = result.screenshot_after {
        let screenshot_info = format!(
            "\n[Screenshot: {}x{} (base64 length: {})]",
            screenshot.width,
            screenshot.height,
            screenshot.base64.len()
        );
        output.push_str(&screenshot_info);

        let mut data_obj = data.unwrap_or_else(|| json!({}));
        if let Some(obj) = data_obj.as_object_mut() {
            obj.insert("screenshot_base64".to_string(), json!(screenshot.base64));
            obj.insert("screenshot_width".to_string(), json!(screenshot.width));
            obj.insert("screenshot_height".to_string(), json!(screenshot.height));
        }
        data = Some(data_obj);
    }

    if result.success {
        ToolExecutionResult::success(output).with_data(data.unwrap_or(json!({})))
    } else {
        ToolExecutionResult::error(output)
    }
}

/// Map [`ComputerError`] to [`SyscityError`].
fn to_syscity_err(e: ComputerError) -> crate::error::SyscityError {
    match e {
        ComputerError::UnsupportedPlatform(msg) => crate::error::SyscityError::Unsupported(msg),
        ComputerError::NoDisplay => {
            crate::error::SyscityError::Unsupported("No display server available".to_string())
        }
        ComputerError::AccessibilityDenied => crate::error::SyscityError::Unsupported(
            "Accessibility permission not granted".to_string(),
        ),
        other => crate::error::SyscityError::Internal(other.to_string()),
    }
}
