//! Android device control via ADB.
//!
//! Provides screenshot, tap, swipe, text input, key events, app installation,
//! launch, force-stop, and UI tree dump.

use super::{has_adb, run_cmd};
use crate::capabilities::{CapabilitySet, OsControlScope, PlatformConstraints};
use crate::tools::{create_schema, Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use serde_json::Value;

// ── Android Capability Set ─────────────────────────────────────────────────

/// Android device capability set (requires `adb` on PATH).
pub struct AndroidSet;

impl Default for AndroidSet {
    fn default() -> Self {
        Self::new()
    }
}

impl AndroidSet {
    pub fn new() -> Self {
        Self
    }
}

impl CapabilitySet for AndroidSet {
    fn id(&self) -> &str {
        "android"
    }

    fn name(&self) -> &str {
        "Android Device Bridge"
    }

    fn description(&self) -> &str {
        "Control Android devices via ADB: screenshot, tap, type, install, launch apps"
    }

    fn constraints(&self) -> &PlatformConstraints {
        // Availability is determined by `has_adb()` at runtime.
        static CONSTRAINTS: std::sync::OnceLock<PlatformConstraints> = std::sync::OnceLock::new();
        CONSTRAINTS.get_or_init(|| PlatformConstraints {
            target_os: Vec::<String>::new(), // any OS
            requires_gui: false,
            requires_services: Vec::<String>::new(),
        })
    }

    fn scope(&self) -> OsControlScope {
        OsControlScope::UserSpace
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(AdbScreenshotTool::new()),
            Box::new(AdbInputTool::new()),
            Box::new(AdbAppManagerTool::new()),
            Box::new(AdbUiTreeTool::new()),
        ]
    }

    fn is_available(&self) -> bool {
        has_adb()
    }
}

// ── ADB Screenshot Tool ────────────────────────────────────────────────────

/// Capture device screen via `adb exec-out screencap -p`.
#[derive(Debug)]
pub struct AdbScreenshotTool {
    device: Option<String>,
}

impl Default for AdbScreenshotTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbScreenshotTool {
    pub fn new() -> Self {
        Self { device: None }
    }

    pub fn with_device(mut self, device: String) -> Self {
        self.device = Some(device);
        self
    }

    fn adb_args(&self, base: &[&str]) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(d) = &self.device {
            args.push("-s".to_string());
            args.push(d.clone());
        }
        for a in base {
            args.push(a.to_string());
        }
        args
    }
}

#[async_trait]
impl Tool for AdbScreenshotTool {
    fn name(&self) -> &str {
        "android_screenshot"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of the connected Android device and return it as base64 PNG."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Capture Android device screenshot",
            serde_json::json!({
                "device": {
                    "type": "string",
                    "description": "Optional device serial number",
                }
            }),
            Vec::<String>::new(),
        )
    }

    async fn execute(
        &self,
        _args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let adb_args = self.adb_args(&["exec-out", "screencap", "-p"]);
        let (status, stdout, stderr) = run_cmd("adb", &adb_args.iter().map(|s| s.as_str()).collect::<Vec<_>>()).await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "adb screencap failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !status.success() {
            return Ok(ToolExecutionResult::error(format!("adb screencap failed: {}", stderr)));
        }

        let base64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            stdout.as_bytes(),
        );

        Ok(ToolExecutionResult::success("Screenshot captured").with_data(serde_json::json!({
            "base64": base64,
            "format": "png",
        })))
    }
}

// ── ADB Input Tool ─────────────────────────────────────────────────────────

/// Tap, swipe, type, and press keys on the device.
#[derive(Debug)]
pub struct AdbInputTool {
    device: Option<String>,
}

impl Default for AdbInputTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbInputTool {
    pub fn new() -> Self {
        Self { device: None }
    }

    pub fn with_device(mut self, device: String) -> Self {
        self.device = Some(device);
        self
    }

    fn adb_args(&self, base: &[&str]) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(d) = &self.device {
            args.push("-s".to_string());
            args.push(d.clone());
        }
        for a in base {
            args.push(a.to_string());
        }
        args
    }
}

#[async_trait]
impl Tool for AdbInputTool {
    fn name(&self) -> &str {
        "android_input"
    }

    fn description(&self) -> &str {
        "Send tap, swipe, text, or key events to an Android device via ADB."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Send input to Android device",
            serde_json::json!({
                "action": {
                    "type": "string",
                    "description": "tap | swipe | text | key",
                    "enum": ["tap", "swipe", "text", "key"]
                },
                "x": { "type": "integer", "description": "X coordinate (tap/swipe)" },
                "y": { "type": "integer", "description": "Y coordinate (tap/swipe)" },
                "x2": { "type": "integer", "description": "End X (swipe)" },
                "y2": { "type": "integer", "description": "End Y (swipe)" },
                "text": { "type": "string", "description": "Text to type" },
                "keycode": { "type": "string", "description": "Android keycode name or number" },
                "device": { "type": "string", "description": "Optional device serial" }
            }),
            vec!["action"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("tap");

        let shell_cmd = match action {
            "tap" => {
                let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
                let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
                format!("input tap {} {}", x, y)
            }
            "swipe" => {
                let x1 = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
                let y1 = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
                let x2 = args.get("x2").and_then(|v| v.as_i64()).unwrap_or(0);
                let y2 = args.get("y2").and_then(|v| v.as_i64()).unwrap_or(0);
                format!("input swipe {} {} {} {}", x1, y1, x2, y2)
            }
            "text" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .replace(' ', "%s");
                format!("input text '{}'", text)
            }
            "key" => {
                let keycode = args.get("keycode").and_then(|v| v.as_str()).unwrap_or("HOME");
                format!("input keyevent {}", keycode)
            }
            _ => return Ok(ToolExecutionResult::error(format!("Unknown action: {}", action))),
        };

        let adb_args = self.adb_args(&["shell", &shell_cmd]);
        let (status, _stdout, stderr) = run_cmd(
            "adb",
            &adb_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        )
        .await
        .map_err(|e| crate::error::SyscityError::ExternalService {
            source: "adb input failed".to_string(),
            cause: Some(Box::new(e)),
        })?;

        if !status.success() {
            return Ok(ToolExecutionResult::error(format!("adb input failed: {}", stderr)));
        }

        Ok(ToolExecutionResult::success(format!("Input '{}' sent", action)))
    }
}

// ── ADB App Manager Tool ───────────────────────────────────────────────────

/// Install, launch, force-stop, and list Android apps.
#[derive(Debug)]
pub struct AdbAppManagerTool {
    device: Option<String>,
}

impl Default for AdbAppManagerTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbAppManagerTool {
    pub fn new() -> Self {
        Self { device: None }
    }

    pub fn with_device(mut self, device: String) -> Self {
        self.device = Some(device);
        self
    }

    fn adb_args(&self, base: &[&str]) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(d) = &self.device {
            args.push("-s".to_string());
            args.push(d.clone());
        }
        for a in base {
            args.push(a.to_string());
        }
        args
    }
}

#[async_trait]
impl Tool for AdbAppManagerTool {
    fn name(&self) -> &str {
        "android_app_manager"
    }

    fn description(&self) -> &str {
        "Install, launch, force-stop, or list apps on an Android device."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Manage Android apps",
            serde_json::json!({
                "action": {
                    "type": "string",
                    "description": "install | launch | force_stop | list_packages",
                    "enum": ["install", "launch", "force_stop", "list_packages"]
                },
                "package": { "type": "string", "description": "Package name (e.g. com.example.app)" },
                "activity": { "type": "string", "description": "Activity class (launch)" },
                "apk_path": { "type": "string", "description": "Local path to APK (install)" },
                "device": { "type": "string", "description": "Optional device serial" }
            }),
            vec!["action"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let adb_args = match action {
            "install" => {
                let path = args.get("apk_path").and_then(|v| v.as_str()).unwrap_or("");
                self.adb_args(&["install", "-r", path])
            }
            "launch" => {
                let pkg = args.get("package").and_then(|v| v.as_str()).unwrap_or("");
                let activity = args.get("activity").and_then(|v| v.as_str());
                let component = match activity {
                    Some(a) => format!("{}/{}", pkg, a),
                    None => format!("{}/.MainActivity", pkg),
                };
                self.adb_args(&["shell", "am", "start", "-n", &component])
            }
            "force_stop" => {
                let pkg = args.get("package").and_then(|v| v.as_str()).unwrap_or("");
                self.adb_args(&["shell", "am", "force-stop", pkg])
            }
            "list_packages" => self.adb_args(&["shell", "pm", "list", "packages"]),
            _ => {
                return Ok(ToolExecutionResult::error(format!(
                    "Unknown action: {}",
                    action
                )))
            }
        };

        let (status, stdout, stderr) = run_cmd(
            "adb",
            &adb_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        )
        .await
        .map_err(|e| crate::error::SyscityError::ExternalService {
            source: "adb app manager failed".to_string(),
            cause: Some(Box::new(e)),
        })?;

        if !status.success() {
            return Ok(ToolExecutionResult::error(format!(
                "adb app manager failed: {}",
                stderr
            )));
        }

        Ok(ToolExecutionResult::success(format!("Action '{}' completed\n{}", action, stdout)))
    }
}

// ── ADB UI Tree Tool ───────────────────────────────────────────────────────

/// Dump the Android accessibility UI tree via `uiautomator`.
#[derive(Debug)]
pub struct AdbUiTreeTool {
    device: Option<String>,
}

impl Default for AdbUiTreeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbUiTreeTool {
    pub fn new() -> Self {
        Self { device: None }
    }

    pub fn with_device(mut self, device: String) -> Self {
        self.device = Some(device);
        self
    }

    fn adb_args(&self, base: &[&str]) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(d) = &self.device {
            args.push("-s".to_string());
            args.push(d.clone());
        }
        for a in base {
            args.push(a.to_string());
        }
        args
    }
}

#[async_trait]
impl Tool for AdbUiTreeTool {
    fn name(&self) -> &str {
        "android_ui_tree"
    }

    fn description(&self) -> &str {
        "Dump the Android device UI hierarchy as XML via uiautomator."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Get Android UI tree",
            serde_json::json!({
                "device": { "type": "string", "description": "Optional device serial" }
            }),
            Vec::<String>::new(),
        )
    }

    async fn execute(
        &self,
        _args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        // Dump to device /sdcard/window_dump.xml, then pull it.
        let dump_args = self.adb_args(&["shell", "uiautomator", "dump", "/sdcard/window_dump.xml"]);
        let (status, _, stderr) = run_cmd(
            "adb",
            &dump_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        )
        .await
        .map_err(|e| crate::error::SyscityError::ExternalService {
            source: "adb uiautomator dump failed".to_string(),
            cause: Some(Box::new(e)),
        })?;

        if !status.success() {
            return Ok(ToolExecutionResult::error(format!(
                "uiautomator dump failed: {}",
                stderr
            )));
        }

        let pull_args = self.adb_args(&["pull", "/sdcard/window_dump.xml", "-"]);
        let (status, stdout, stderr) = run_cmd(
            "adb",
            &pull_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        )
        .await
        .map_err(|e| crate::error::SyscityError::ExternalService {
            source: "adb pull failed".to_string(),
            cause: Some(Box::new(e)),
        })?;

        if !status.success() {
            return Ok(ToolExecutionResult::error(format!("adb pull failed: {}", stderr)));
        }

        Ok(ToolExecutionResult::success(format!("UI tree dumped\n{}", stdout)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adb_screenshot_tool_name() {
        let tool = AdbScreenshotTool::new();
        assert_eq!(tool.name(), "android_screenshot");
    }

    #[test]
    fn test_adb_input_tool_schema() {
        let tool = AdbInputTool::new();
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
    }

    #[test]
    fn test_adb_app_manager_tool_name() {
        let tool = AdbAppManagerTool::new();
        assert_eq!(tool.name(), "android_app_manager");
    }

    #[test]
    fn test_android_set_id() {
        let set = AndroidSet::new();
        assert_eq!(set.id(), "android");
    }
}
