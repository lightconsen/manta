//! macOS Computer adapter — wraps macOS CapabilitySet tools.

use crate::computer::{
    ActionResult, ClickTarget, ComputerAdapter, ComputerError, DesktopAction,
    Rect, Result, Screenshot, UiElement, WaitCondition,
};
use crate::tools::ToolRegistry;
use std::sync::Arc;
use std::time::Duration;

/// macOS adapter backed by `macos_screenshot`, `macos_accessibility`,
/// `macos_desktop_control`, and `macos_applescript` tools.
pub struct MacosComputerAdapter {
    registry: Arc<ToolRegistry>,
}

impl MacosComputerAdapter {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl ComputerAdapter for MacosComputerAdapter {
    async fn screenshot(&self, region: Option<Rect>) -> Result<Screenshot> {
        let args = if let Some(r) = region {
            serde_json::json!({
                "region": { "x": r.x, "y": r.y, "width": r.width, "height": r.height }
            })
        } else {
            serde_json::json!({})
        };

        let result = self
            .registry
            .execute("macos_screenshot", args, &crate::tools::ToolContext::default())
            .await
            .ok_or_else(|| ComputerError::ToolFailed("screenshot tool not found".to_string()))?
            .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;

        if !result.success {
            return Err(ComputerError::ScreenshotFailed(
                result.error.unwrap_or_default(),
            ));
        }

        let data = result.data.as_ref();

        let base64 = data
            .and_then(|d| d.get("base64").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_default();

        let width = data
            .and_then(|d| d.get("width").and_then(|v| v.as_u64()))
            .unwrap_or(0) as u32;

        let height = data
            .and_then(|d| d.get("height").and_then(|v| v.as_u64()))
            .unwrap_or(0) as u32;

        Ok(Screenshot {
            base64,
            width,
            height,
        })
    }

    async fn read_ui_tree(
        &self,
        app: Option<&str>,
    ) -> Result<Vec<UiElement>> {
        let args = if let Some(a) = app {
            serde_json::json!({ "action": "tree", "app": a })
        } else {
            serde_json::json!({ "action": "tree" })
        };

        let result = self
            .registry
            .execute("macos_accessibility", args, &crate::tools::ToolContext::default())
            .await
            .ok_or_else(|| ComputerError::ToolFailed("accessibility tool not found".to_string()))?
            .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;

        if !result.success {
            return Err(ComputerError::AccessibilityDenied);
        }

        let elements: Vec<UiElement> = result
            .data
            .and_then(|d| serde_json::from_value(d.get("elements")?.clone()).ok())
            .unwrap_or_default();

        Ok(elements)
    }

    async fn execute(&self, action: DesktopAction) -> Result<ActionResult> {
        match action {
            DesktopAction::Screenshot { region } => {
                let ss = self.screenshot(region).await?;
                Ok(ActionResult::success("screenshot captured").with_data(
                    serde_json::to_value(&ss).unwrap_or_default(),
                ))
            }
            DesktopAction::Click { target, button: _ } => {
                let (x, y) = self.resolve_click_target(target).await?;
                let script = format!(
                    r#"tell application "System Events" to click at {{ {}, {} }}"#,
                    x, y
                );
                let apple_args = serde_json::json!({ "script": script });
                let result = self
                    .registry
                    .execute("macos_applescript", apple_args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("applescript tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::DoubleClick { target, button: _ } => {
                let (x, y) = self.resolve_click_target(target).await?;
                let script = format!(
                    r#"tell application "System Events"
    click at {{ {}, {} }}
    delay 0.05
    click at {{ {}, {} }}
end tell"#,
                    x, y, x, y
                );
                let apple_args = serde_json::json!({ "script": script });
                let result = self
                    .registry
                    .execute("macos_applescript", apple_args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("applescript tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Scroll { target: _, direction, amount } => {
                let key_code = match direction {
                    crate::computer::ScrollDirection::Up => "116",
                    crate::computer::ScrollDirection::Down => "121",
                    crate::computer::ScrollDirection::Left => "123",
                    crate::computer::ScrollDirection::Right => "124",
                };
                let mut script = String::new();
                for _ in 0..amount {
                    script.push_str(&format!("tell application \"System Events\" to key code {}\ndelay 0.05\n", key_code));
                }
                let apple_args = serde_json::json!({ "script": script });
                let result = self
                    .registry
                    .execute("macos_applescript", apple_args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("applescript tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Drag { from, to } => {
                let (from_x, from_y) = self.resolve_click_target(from).await?;
                let (to_x, to_y) = self.resolve_click_target(to).await?;
                let script = format!(
                    r#"tell application "System Events"
    click at {{ {}, {} }}
    delay 0.1
    key down option
    click at {{ {}, {} }}
    key up option
end tell"#,
                    from_x, from_y, to_x, to_y
                );
                let apple_args = serde_json::json!({ "script": script });
                let result = self
                    .registry
                    .execute("macos_applescript", apple_args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("applescript tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::CloseWindow { title_pattern } => {
                let args = serde_json::json!({ "action": "close_window", "app": title_pattern });
                let result = self
                    .registry
                    .execute("macos_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Type { text } => {
                let args = serde_json::json!({ "action": "type", "text": text });
                let result = self
                    .registry
                    .execute("macos_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::KeyPress { keys } => {
                let args = serde_json::json!({ "action": "key", "keys": keys });
                let result = self
                    .registry
                    .execute("macos_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardGet => {
                let args = serde_json::json!({ "action": "get" });
                let result = self
                    .registry
                    .execute("macos_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("clipboard tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardSet { text } => {
                let args = serde_json::json!({ "action": "set", "text": text });
                let result = self
                    .registry
                    .execute("macos_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("clipboard tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::LaunchApp { name, args: _app_args, wait_for_ready } => {
                let script = format!(r#"tell application "{}" to activate"#, name);
                let apple_args = serde_json::json!({ "script": script });
                self.registry
                    .execute("macos_applescript", apple_args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("applescript tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;

                if wait_for_ready {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let _ = self.read_ui_tree(Some(&name)).await?;
                }
                Ok(ActionResult::success(format!("Launched {}", name)))
            }
            DesktopAction::ActivateWindow { title_pattern } => {
                let script = format!(
                    r#"tell application "System Events"
    set frontmost of (first process whose name contains "{}") to true
end tell"#,
                    title_pattern
                );
                let apple_args = serde_json::json!({ "script": script });
                let result = self
                    .registry
                    .execute("macos_applescript", apple_args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("applescript tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Wait { milliseconds } => {
                tokio::time::sleep(Duration::from_millis(milliseconds)).await;
                Ok(ActionResult::success(format!("Waited {}ms", milliseconds)))
            }
            DesktopAction::GetSystemStatus => {
                let status = tokio::task::spawn_blocking(|| {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.get_status()
                })
                .await
                .map_err(|e| ComputerError::Other(format!("System monitor failed: {}", e)))?;
                Ok(ActionResult::success("System status retrieved").with_data(
                    serde_json::to_value(&status).unwrap_or_default(),
                ))
            }
            DesktopAction::ListProcesses { filter, limit } => {
                let procs = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.list_processes(filter.as_deref(), limit)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Process list failed: {}", e)))?;
                Ok(ActionResult::success(format!("Found {} processes", procs.len())).with_data(
                    serde_json::to_value(&procs).unwrap_or_default(),
                ))
            }
            DesktopAction::KillProcess { pid, name, force } => {
                let killed_pid = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.kill_process(pid, name.as_deref(), force)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Kill failed: {}", e)))??;
                Ok(ActionResult::success(format!("Killed process {}", killed_pid)))
            }
            _ => Err(ComputerError::Other(
                "Action not yet implemented on macOS".to_string(),
            )),
        }
    }

    async fn wait_for(
        &self,
        condition: WaitCondition,
        timeout: Duration,
    ) -> Result<bool> {
        let deadline = std::time::Instant::now() + timeout;
        let poll_interval = Duration::from_millis(500);

        while std::time::Instant::now() < deadline {
            let matched = match &condition {
                WaitCondition::UiTreeContains { role, label } => {
                    let tree = self.read_ui_tree(None).await?;
                    tree.iter().any(|e| {
                        e.role == *role
                            && label
                                .as_ref()
                                .map(|l| e.label.as_ref().map(|el| el.contains(l)).unwrap_or(false))
                                .unwrap_or(true)
                    })
                }
                WaitCondition::WindowTitleContains { pattern } => {
                    let script = r#"tell application "System Events" to return name of first process whose frontmost is true"#;
                    let args = serde_json::json!({ "script": script });
                    if let Some(Ok(result)) = self
                        .registry
                        .execute("macos_applescript", args, &crate::tools::ToolContext::default())
                        .await
                    {
                        result.output.contains(pattern)
                    } else {
                        false
                    }
                }
                WaitCondition::ProcessRunning { name } => {
                    let script = format!(
                        r#"tell application "System Events" to return (name of processes) contains "{}""#,
                        name
                    );
                    let args = serde_json::json!({ "script": script });
                    if let Some(Ok(result)) = self
                        .registry
                        .execute("macos_applescript", args, &crate::tools::ToolContext::default())
                        .await
                    {
                        result.output == "true"
                    } else {
                        false
                    }
                }
                WaitCondition::FileExists { path } => {
                    tokio::fs::try_exists(path).await.unwrap_or(false)
                }
                _ => false,
            };

            if matched {
                return Ok(true);
            }
            tokio::time::sleep(poll_interval).await;
        }
        Ok(false)
    }
}

impl MacosComputerAdapter {
    async fn resolve_click_target(&self, target: ClickTarget) -> Result<(i32, i32)> {
        match target {
            ClickTarget::Coordinate(p) => Ok((p.x, p.y)),
            ClickTarget::ElementId(id) => {
                let tree = self.read_ui_tree(None).await?;
                let el = tree
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or_else(|| ComputerError::ElementNotFound(id.clone()))?;
                let center = el.center();
                Ok((center.x, center.y))
            }
            ClickTarget::ElementLabel(label) => {
                let tree = self.read_ui_tree(None).await?;
                let el = tree
                    .iter()
                    .find(|e| e.label.as_ref().map(|l| l.contains(&label)).unwrap_or(false))
                    .ok_or_else(|| ComputerError::ElementNotFound(label.clone()))?;
                let center = el.center();
                Ok((center.x, center.y))
            }
            ClickTarget::ElementRoleLabel { role, label } => {
                let tree = self.read_ui_tree(None).await?;
                let el = tree
                    .iter()
                    .find(|e| {
                        e.role == role
                            && e.label.as_ref().map(|l| l.contains(&label)).unwrap_or(false)
                    })
                    .ok_or_else(|| ComputerError::ElementNotFound(format!("{}:{}", role, label)))?;
                let center = el.center();
                Ok((center.x, center.y))
            }
        }
    }
}

/// Factory for macOS.
pub async fn create(registry: Arc<ToolRegistry>) -> Result<Box<dyn ComputerAdapter>> {
    Ok(Box::new(MacosComputerAdapter::new(registry)))
}
