//! Linux Computer adapter — wraps X11, Wayland, or headless tools.

use crate::computer::{
    ActionResult, ClickTarget, ComputerAdapter, ComputerError, DesktopAction, MouseButton,
    Rect, Result, Screenshot, UiElement, WaitCondition,
};
use crate::tools::ToolRegistry;
use std::sync::Arc;
use std::time::Duration;

// ── X11 Adapter ─────────────────────────────────────────────────────────────

pub struct X11ComputerAdapter {
    registry: Arc<ToolRegistry>,
}

impl X11ComputerAdapter {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl ComputerAdapter for X11ComputerAdapter {
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
            .execute("linux_x11_screenshot", args, &crate::tools::ToolContext::default())
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

    async fn read_ui_tree(&self, app: Option<&str>) -> Result<Vec<UiElement>> {
        let args = if let Some(a) = app {
            serde_json::json!({ "app": a })
        } else {
            serde_json::json!({})
        };

        let result = self
            .registry
            .execute("linux_x11_accessibility", args, &crate::tools::ToolContext::default())
            .await
            .ok_or_else(|| ComputerError::ToolFailed("accessibility tool not found".to_string()))?
            .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;

        if !result.success {
            return Err(ComputerError::AccessibilityDenied);
        }

        let elements = crate::computer::parse_accessibility_elements(result.data.as_ref());
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
            DesktopAction::Click { target, button } => {
                let (x, y) = match target {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "X11 adapter only supports coordinate clicks for now".to_string(),
                    )),
                };
                let btn_num = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                };
                let args = serde_json::json!({
                    "action": "click", "x": x, "y": y, "button": btn_num,
                });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::DoubleClick { target, button } => {
                let (x, y) = match target {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "X11 adapter only supports coordinate double-clicks for now".to_string(),
                    )),
                };
                let btn_num = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                };
                let args = serde_json::json!({
                    "action": "double_click", "x": x, "y": y, "button": btn_num,
                });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Scroll { target, direction, amount } => {
                let (x, y) = match target {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "X11 adapter only supports coordinate scroll for now".to_string(),
                    )),
                };
                let dir_str = match direction {
                    crate::computer::ScrollDirection::Up => "up",
                    crate::computer::ScrollDirection::Down => "down",
                    crate::computer::ScrollDirection::Left => "left",
                    crate::computer::ScrollDirection::Right => "right",
                };
                let args = serde_json::json!({
                    "action": "scroll", "x": x, "y": y, "direction": dir_str, "amount": amount,
                });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Drag { from, to } => {
                let (from_x, from_y) = match from {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "X11 adapter only supports coordinate drag for now".to_string(),
                    )),
                };
                let (to_x, to_y) = match to {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "X11 adapter only supports coordinate drag for now".to_string(),
                    )),
                };
                let args = serde_json::json!({
                    "action": "drag", "from_x": from_x, "from_y": from_y, "to_x": to_x, "to_y": to_y,
                });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::CloseWindow { title_pattern } => {
                let args = serde_json::json!({
                    "action": "close_window", "name": title_pattern,
                });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Type { text } => {
                let args = serde_json::json!({ "action": "type", "text": text });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::KeyPress { keys } => {
                let args = serde_json::json!({ "action": "key", "keys": keys });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardGet => {
                let args = serde_json::json!({ "action": "get" });
                let result = self
                    .registry
                    .execute("linux_x11_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("clipboard tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardSet { text } => {
                let args = serde_json::json!({ "action": "set", "text": text });
                let result = self
                    .registry
                    .execute("linux_x11_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("clipboard tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::LaunchApp { name, wait_for_ready, .. } => {
                let result = tokio::process::Command::new(&name)
                    .spawn()
                    .map_err(|e| ComputerError::ToolFailed(format!("Failed to launch {}: {}", name, e)))?;
                drop(result);
                if wait_for_ready {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Ok(ActionResult::success(format!("Launched {}", name)))
            }
            DesktopAction::ActivateWindow { title_pattern } => {
                let args = serde_json::json!({
                    "action": "activate_window",
                    "name": title_pattern,
                });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Wait { milliseconds } => {
                tokio::time::sleep(Duration::from_millis(milliseconds)).await;
                Ok(ActionResult::success(format!("Waited {}ms", milliseconds)))
            }
            _ => Err(ComputerError::Other(
                "Action not yet implemented on X11".to_string(),
            )),
        }
    }

    async fn wait_for(
        &self,
        _condition: WaitCondition,
        timeout: Duration,
    ) -> Result<bool> {
        // Minimal implementation: just wait the timeout
        tokio::time::sleep(timeout).await;
        Ok(false)
    }
}

// ── Wayland Adapter ────────────────────────────────────────────────────────

pub struct WaylandComputerAdapter {
    registry: Arc<ToolRegistry>,
}

impl WaylandComputerAdapter {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl ComputerAdapter for WaylandComputerAdapter {
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
            .execute("linux_wayland_screenshot", args, &crate::tools::ToolContext::default())
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

    async fn read_ui_tree(&self, app: Option<&str>) -> Result<Vec<UiElement>> {
        let args = if let Some(a) = app {
            serde_json::json!({ "app": a })
        } else {
            serde_json::json!({})
        };

        let result = self
            .registry
            .execute("linux_wayland_accessibility", args, &crate::tools::ToolContext::default())
            .await
            .ok_or_else(|| ComputerError::ToolFailed("accessibility tool not found".to_string()))?
            .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;

        // Wayland tool always returns success with explanation; parse whatever we got
        let elements = crate::computer::parse_accessibility_elements(result.data.as_ref());
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
            DesktopAction::Click { target, button } => {
                let (x, y) = match target {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "Wayland adapter only supports coordinate clicks for now".to_string(),
                    )),
                };
                let btn_num = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                };
                let args = serde_json::json!({
                    "action": "click", "x": x, "y": y, "button": btn_num,
                });
                let result = self
                    .registry
                    .execute("linux_wayland_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::DoubleClick { target, button } => {
                let (x, y) = match target {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "Wayland adapter only supports coordinate double-clicks for now".to_string(),
                    )),
                };
                let btn_num = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                };
                let args = serde_json::json!({
                    "action": "double_click", "x": x, "y": y, "button": btn_num,
                });
                let result = self
                    .registry
                    .execute("linux_wayland_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Scroll { target, direction, amount } => {
                let dir_str = match direction {
                    crate::computer::ScrollDirection::Up => "up",
                    crate::computer::ScrollDirection::Down => "down",
                    crate::computer::ScrollDirection::Left => "left",
                    crate::computer::ScrollDirection::Right => "right",
                };
                let args = serde_json::json!({
                    "action": "scroll", "direction": dir_str, "amount": amount,
                });
                let result = self
                    .registry
                    .execute("linux_wayland_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Drag { from, to } => {
                let (from_x, from_y) = match from {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "Wayland adapter only supports coordinate drag for now".to_string(),
                    )),
                };
                let (to_x, to_y) = match to {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => return Err(ComputerError::Other(
                        "Wayland adapter only supports coordinate drag for now".to_string(),
                    )),
                };
                let args = serde_json::json!({
                    "action": "drag", "from_x": from_x, "from_y": from_y, "to_x": to_x, "to_y": to_y,
                });
                let result = self
                    .registry
                    .execute("linux_wayland_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::CloseWindow { title_pattern } => {
                let args = serde_json::json!({
                    "action": "close_window", "name": title_pattern,
                });
                let result = self
                    .registry
                    .execute("linux_wayland_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Type { text } => {
                let args = serde_json::json!({ "action": "type", "text": text });
                let result = self
                    .registry
                    .execute("linux_wayland_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::KeyPress { keys } => {
                let args = serde_json::json!({ "action": "key", "keys": keys });
                let result = self
                    .registry
                    .execute("linux_wayland_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Wait { milliseconds } => {
                tokio::time::sleep(Duration::from_millis(milliseconds)).await;
                Ok(ActionResult::success(format!("Waited {}ms", milliseconds)))
            }
            _ => Err(ComputerError::Other(
                "Action not yet implemented on Wayland".to_string(),
            )),
        }
    }

    async fn wait_for(
        &self,
        _condition: WaitCondition,
        timeout: Duration,
    ) -> Result<bool> {
        tokio::time::sleep(timeout).await;
        Ok(false)
    }
}

// ── Headless Adapter ───────────────────────────────────────────────────────
//
// The HeadlessComputerAdapter is now defined in `headless.rs` and re-exported
// from `computer::mod`.  This module only provides the Linux-specific factory.

// ── Factory ────────────────────────────────────────────────────────────────

/// Detect X11 at runtime.
fn has_x11() -> bool {
    std::env::var("DISPLAY").is_ok() && std::env::var("WAYLAND_DISPLAY").is_err()
}

/// Detect Wayland at runtime.
fn has_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
}

/// Create the appropriate Linux adapter.
pub async fn create(registry: Arc<ToolRegistry>) -> Result<Box<dyn ComputerAdapter>> {
    if has_wayland() {
        Ok(Box::new(WaylandComputerAdapter::new(registry)))
    } else if has_x11() {
        Ok(Box::new(X11ComputerAdapter::new(registry)))
    } else {
        Ok(Box::new(super::HeadlessComputerAdapter::new(registry)))
    }
}
