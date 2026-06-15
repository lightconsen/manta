//! Windows Accessibility tool — query UI trees via PowerShell + UIAutomation.
//!
//! Uses the .NET `System.Windows.Automation` namespace to traverse the
//! foreground window's control hierarchy and output it as JSON.

use crate::tools::{create_schema, Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

/// Description of a UI element on Windows.
#[derive(Debug, Clone, Serialize)]
pub struct UiElement {
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub enabled: Option<bool>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub children: Vec<UiElement>,
}

/// Result of an accessibility query.
#[derive(Debug, Clone, Serialize)]
pub struct AccessibilityResult {
    pub success: bool,
    pub app: Option<String>,
    pub elements: Vec<UiElement>,
    pub raw_output: Option<String>,
    pub error: Option<String>,
}

/// Query the Windows accessibility tree using UIAutomation.
///
/// This tool requires PowerShell and the .NET Framework (both present
/// on standard Windows desktops).  On Windows Server Core or stripped
/// builds the UIAutomationClient assembly may be missing — the tool
/// degrades gracefully with an explanatory error.
#[derive(Debug)]
pub struct WindowsAccessibilityTool;

impl Default for WindowsAccessibilityTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsAccessibilityTool {
    pub fn new() -> Self {
        Self
    }

    /// Build a PowerShell script that traverses the foreground window.
    fn build_ui_tree_script() -> String {
        r#"
Add-Type -AssemblyName UIAutomationClient -ErrorAction Stop

function Export-Element {
    param($elem, $depth = 0)
    $r = $elem.Current
    $obj = @{
        role   = $r.ControlType.ProgrammaticName -replace '^ControlType\.',''
        name   = $r.Name
        value  = if ($r.Value -is [string]) { $r.Value } else { $null }
        enabled = $r.IsEnabled
        x      = [int]$r.BoundingRectangle.X
        y      = [int]$r.BoundingRectangle.Y
        width  = [int]$r.BoundingRectangle.Width
        height = [int]$r.BoundingRectangle.Height
        children = @()
    }
    if ($depth -lt 3) {
        $children = $elem.FindAll([System.Windows.Automation.TreeScope]::Children,
                                  [System.Windows.Automation.Condition]::TrueCondition)
        for ($i = 0; $i -lt $children.Count; $i++) {
            $obj.children += (Export-Element $children[$i] ($depth + 1))
        }
    }
    return $obj
}

$root = [System.Windows.Automation.AutomationElement]::RootElement
# Try to find the foreground (active) window
$cond = [System.Windows.Automation.ControlTypeCondition]::Window
$allWindows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)
$target = $null
for ($i = 0; $i -lt $allWindows.Count; $i++) {
    if ($allWindows[$i].Current.NativeWindowHandle -ne 0) {
        $target = $allWindows[$i]
        break
    }
}
if (-not $target -and $allWindows.Count -gt 0) {
    $target = $allWindows[0]
}

if ($target) {
    $tree = Export-Element $target
    $tree | ConvertTo-Json -Depth 10 -Compress
} else {
    '{"role":"desktop","name":"","children":[]}'
}
"#
        .to_string()
    }

    /// Parse the JSON returned by PowerShell into our Rust types.
    fn parse_json_tree(value: &Value) -> Vec<UiElement> {
        let mut result = Vec::new();
        Self::parse_element(value, &mut result);
        result
    }

    fn parse_element(value: &Value, out: &mut Vec<UiElement>) {
        if let Some(obj) = value.as_object() {
            let el = UiElement {
                role: obj
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                name: obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                value: obj.get("value").and_then(|v| v.as_str()).map(String::from),
                enabled: obj.get("enabled").and_then(|v| v.as_bool()),
                x: obj.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                y: obj.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                width: obj.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                height: obj.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                children: Vec::new(),
            };
            out.push(el);

            if let Some(children) = obj.get("children").and_then(|v| v.as_array()) {
                for child in children {
                    Self::parse_element(child, out);
                }
            }
        }
    }

    async fn run_ps(script: &str) -> crate::Result<(bool, String, String)> {
        let output = timeout(
            Duration::from_secs(15),
            Command::new("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
                .output(),
        )
        .await;
        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Ok((out.status.success(), stdout, stderr))
            }
            Ok(Err(e)) => Ok((false, String::new(), format!("PowerShell spawn error: {}", e))),
            Err(_) => Ok((false, String::new(), "PowerShell timed out".to_string())),
        }
    }
}

#[async_trait]
impl Tool for WindowsAccessibilityTool {
    fn name(&self) -> &str {
        "windows_accessibility"
    }

    fn description(&self) -> &str {
        "Query the Windows accessibility/UIAutomation tree for the active window. \
         Returns a structured list of buttons, text fields, menus, and other UI elements. \
         Use this before taking screenshots whenever possible for structured desktop perception."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Query UI tree via UIAutomation",
            serde_json::json!({
                "app": {
                    "type": "string",
                    "description": "Window title pattern to inspect (partial match). If omitted, inspects the foreground window."
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
        info!("Querying Windows accessibility tree");

        let (success, stdout, stderr) = Self::run_ps(&Self::build_ui_tree_script()).await?;

        let mut result = AccessibilityResult {
            success,
            app: None,
            elements: Vec::new(),
            raw_output: Some(stdout.clone()),
            error: if success { None } else { Some(stderr.clone()) },
        };

        if success {
            match serde_json::from_str::<Value>(&stdout) {
                Ok(json) => {
                    result.app = json.get("name").and_then(|v| v.as_str()).map(String::from);
                    result.elements = Self::parse_json_tree(&json);
                }
                Err(e) => {
                    warn!("Failed to parse UIAutomation JSON output: {}", e);
                    result.success = false;
                    result.error = Some(format!("JSON parse error: {}", e));
                }
            }
        } else {
            warn!("UIAutomation query failed: {}", stderr);
        }

        let json = serde_json::to_string_pretty(&result)
            .map_err(crate::error::SyscityError::Serialization)?;

        if result.success {
            Ok(ToolExecutionResult::success(json).with_data(serde_json::to_value(result)?))
        } else {
            Ok(ToolExecutionResult::error(result.error.clone().unwrap_or_default())
                .with_data(serde_json::to_value(result)?))
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        cfg!(target_os = "windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accessibility_tool_creation() {
        let tool = WindowsAccessibilityTool::new();
        assert_eq!(tool.name(), "windows_accessibility");
        assert!(tool.description().contains("UIAutomation"));
    }

    #[test]
    fn test_parse_json_tree() {
        let json = serde_json::json!({
            "role": "Window",
            "name": "Calculator",
            "enabled": true,
            "x": 0, "y": 0, "width": 400, "height": 600,
            "children": [
                {
                    "role": "Button",
                    "name": "7",
                    "enabled": true,
                    "x": 10, "y": 100, "width": 50, "height": 50,
                    "children": []
                },
                {
                    "role": "Edit",
                    "name": "Display",
                    "value": "0",
                    "enabled": true,
                    "x": 10, "y": 10, "width": 380, "height": 40,
                    "children": []
                }
            ]
        });

        let elements = WindowsAccessibilityTool::parse_json_tree(&json);
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0].role, "Window");
        assert_eq!(elements[1].role, "Button");
        assert_eq!(elements[2].role, "Edit");
        assert_eq!(elements[2].value.as_deref(), Some("0"));
    }
}
