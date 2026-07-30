//! Linux Computer adapter — wraps X11, Wayland, or headless tools.

use std::sync::Arc;
use std::time::Duration;

use crate::computer::{
    ActionResult, ClickTarget, ComputerAdapter, ComputerError, DesktopAction,
    FileEntry, MouseButton, Rect, Result, Screenshot, UiElement, WaitCondition,
};
use crate::tools::ToolRegistry;

// ── X11 Adapter ─────────────────────────────────────────────────────────────

pub struct X11ComputerAdapter {
    registry: Arc<ToolRegistry>,
    file_watcher: tokio::sync::Mutex<Option<crate::computer::FileWatcher>>,
}

impl X11ComputerAdapter {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            file_watcher: tokio::sync::Mutex::new(None),
        }
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
            return Err(ComputerError::ScreenshotFailed(result.error.unwrap_or_default()));
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
            file_path: None,
            timestamp: std::time::Instant::now(),
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
                Ok(ActionResult::success("screenshot captured")
                    .with_data(serde_json::to_value(&ss).unwrap_or_default()))
            }
            DesktopAction::Click { target, button } => {
                let (x, y) = self.resolve_click_target(target).await?;
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
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::DoubleClick { target, button } => {
                let (x, y) = self.resolve_click_target(target).await?;
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
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Scroll { target, direction, amount } => {
                let (x, y) = self.resolve_click_target(target).await?;
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
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Drag { from, to } => {
                let (from_x, from_y) = self.resolve_click_target(from).await?;
                let (to_x, to_y) = self.resolve_click_target(to).await?;
                let args = serde_json::json!({
                    "action": "drag", "from_x": from_x, "from_y": from_y, "to_x": to_x, "to_y": to_y,
                });
                let result = self
                    .registry
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::CloseWindow { title_pattern } => {
                let args = serde_json::json!({
                    "action": "close_window", "name": title_pattern,
                });
                let result = self
                    .registry
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Type { text } => {
                let args = serde_json::json!({ "action": "type", "text": text });
                let result = self
                    .registry
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::KeyPress { keys } => {
                let args = serde_json::json!({ "action": "key", "keys": keys });
                let result = self
                    .registry
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardGet => {
                let args = serde_json::json!({ "action": "get" });
                let result = self
                    .registry
                    .execute("linux_x11_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("clipboard tool not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardSet { text } => {
                let args = serde_json::json!({ "action": "set", "text": text });
                let result = self
                    .registry
                    .execute("linux_x11_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("clipboard tool not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::LaunchApp { name, wait_for_ready, .. } => {
                let result = tokio::process::Command::new(&name).spawn().map_err(|e| {
                    ComputerError::ToolFailed(format!("Failed to launch {}: {}", name, e))
                })?;
                drop(result);
                if wait_for_ready {
                    let ready = self
                        .wait_for(
                            WaitCondition::ProcessRunning { name: name.clone() },
                            Duration::from_secs(10),
                        )
                        .await?;
                    if !ready {
                        return Ok(ActionResult::error(format!(
                            "Launched {} but it did not appear within 10s",
                            name
                        )));
                    }
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
                    .execute(
                        "linux_x11_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
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
                Ok(ActionResult::success("System status retrieved")
                    .with_data(serde_json::to_value(&status).unwrap_or_default()))
            }
            DesktopAction::ListProcesses { filter, limit } => {
                let procs = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.list_processes(filter.as_deref(), limit)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Process list failed: {}", e)))?;
                Ok(ActionResult::success(format!("Found {} processes", procs.len()))
                    .with_data(serde_json::to_value(&procs).unwrap_or_default()))
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
            DesktopAction::RestartProcess { pid, name, force } => {
                let new_pid = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.restart_process(pid, name.as_deref(), force)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Restart failed: {}", e)))??;
                Ok(ActionResult::success(format!("Process restarted, new PID: {}", new_pid)))
            }
            DesktopAction::SetProcessPriority { pid, name, priority } => {
                let updated_pid = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.set_process_priority(pid, name.as_deref(), priority)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Priority change failed: {}", e)))??;
                Ok(ActionResult::success(format!("Priority set for PID {}", updated_pid)))
            }
            // ── Window management ──────────────────────────────────────────
            DesktopAction::ListWindows => {
                let args = serde_json::json!({ "action": "list_windows" });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::GetWindowGeometry { title_pattern, .. } => {
                let args = serde_json::json!({ "action": "get_window_geometry", "name": title_pattern });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::MoveWindow { title_pattern, x, y } => {
                let args = serde_json::json!({ "action": "move_window", "name": title_pattern, "x": x, "y": y });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ResizeWindow { title_pattern, width, height } => {
                let args = serde_json::json!({ "action": "resize_window", "name": title_pattern, "width": width, "height": height });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::MinimizeWindow { title_pattern } => {
                let args = serde_json::json!({ "action": "minimize_window", "name": title_pattern });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::MaximizeWindow { title_pattern } => {
                let args = serde_json::json!({ "action": "maximize_window", "name": title_pattern });
                let result = self
                    .registry
                    .execute("linux_x11_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
        }
    }

    async fn wait_for(&self, condition: WaitCondition, timeout: Duration) -> Result<bool> {
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
                    let args = serde_json::json!({ "action": "list_windows" });
                    if let Some(Ok(result)) = self
                        .registry
                        .execute(
                            "linux_x11_desktop_control",
                            args,
                            &crate::tools::ToolContext::default(),
                        )
                        .await
                    {
                        result.output.contains(pattern)
                    } else {
                        false
                    }
                }
                WaitCondition::ProcessRunning { name } => {
                    let output = tokio::process::Command::new("pidof")
                        .arg(name)
                        .output()
                        .await;
                    match output {
                        Ok(out) => !out.stdout.is_empty(),
                        _ => false,
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

impl X11ComputerAdapter {
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
                    .find(|e| {
                        e.label
                            .as_ref()
                            .map(|l| l.contains(&label))
                            .unwrap_or(false)
                    })
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
                            && e.label
                                .as_ref()
                                .map(|l| l.contains(&label))
                                .unwrap_or(false)
                    })
                    .ok_or_else(|| ComputerError::ElementNotFound(format!("{}:{}", role, label)))?;
                let center = el.center();
                Ok((center.x, center.y))
            }
        }
    }
}

// ── Wayland Adapter ────────────────────────────────────────────────────────

pub struct WaylandComputerAdapter {
    registry: Arc<ToolRegistry>,
    file_watcher: tokio::sync::Mutex<Option<crate::computer::FileWatcher>>,
}

impl WaylandComputerAdapter {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            file_watcher: tokio::sync::Mutex::new(None),
        }
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
            return Err(ComputerError::ScreenshotFailed(result.error.unwrap_or_default()));
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
            file_path: None,
            timestamp: std::time::Instant::now(),
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
                Ok(ActionResult::success("screenshot captured")
                    .with_data(serde_json::to_value(&ss).unwrap_or_default()))
            }
            DesktopAction::Click { target, button } => {
                let (x, y) = self.resolve_click_target(target).await?;
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
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::DoubleClick { target, button } => {
                let (x, y) = self.resolve_click_target(target).await?;
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
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Scroll { target, direction, amount } => {
                let (x, y) = self.resolve_click_target(target).await?;
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
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Drag { from, to } => {
                let (from_x, from_y) = self.resolve_click_target(from).await?;
                let (to_x, to_y) = self.resolve_click_target(to).await?;
                let args = serde_json::json!({
                    "action": "drag", "from_x": from_x, "from_y": from_y, "to_x": to_x, "to_y": to_y,
                });
                let result = self
                    .registry
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::CloseWindow { title_pattern } => {
                let args = serde_json::json!({
                    "action": "close_window", "name": title_pattern,
                });
                let result = self
                    .registry
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Type { text } => {
                let args = serde_json::json!({ "action": "type", "text": text });
                let result = self
                    .registry
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::KeyPress { keys } => {
                let args = serde_json::json!({ "action": "key", "keys": keys });
                let result = self
                    .registry
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardGet => {
                let args = serde_json::json!({ "action": "get" });
                let result = self
                    .registry
                    .execute("linux_wayland_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("clipboard tool not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardSet { text } => {
                let args = serde_json::json!({ "action": "set", "text": text });
                let result = self
                    .registry
                    .execute("linux_wayland_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("clipboard tool not found".to_string())
                    })?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::LaunchApp { name, wait_for_ready, .. } => {
                let result = tokio::process::Command::new(&name).spawn().map_err(|e| {
                    ComputerError::ToolFailed(format!("Failed to launch {}: {}", name, e))
                })?;
                drop(result);
                if wait_for_ready {
                    let ready = self
                        .wait_for(
                            WaitCondition::ProcessRunning { name: name.clone() },
                            Duration::from_secs(10),
                        )
                        .await?;
                    if !ready {
                        return Ok(ActionResult::error(format!(
                            "Launched {} but it did not appear within 10s",
                            name
                        )));
                    }
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
                    .execute(
                        "linux_wayland_desktop_control",
                        args,
                        &crate::tools::ToolContext::default(),
                    )
                    .await
                    .ok_or_else(|| {
                        ComputerError::ToolFailed("desktop control not found".to_string())
                    })?
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
                Ok(ActionResult::success("System status retrieved")
                    .with_data(serde_json::to_value(&status).unwrap_or_default()))
            }
            DesktopAction::ListProcesses { filter, limit } => {
                let procs = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.list_processes(filter.as_deref(), limit)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Process list failed: {}", e)))?;
                Ok(ActionResult::success(format!("Found {} processes", procs.len()))
                    .with_data(serde_json::to_value(&procs).unwrap_or_default()))
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
            DesktopAction::RestartProcess { pid, name, force } => {
                let new_pid = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.restart_process(pid, name.as_deref(), force)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Restart failed: {}", e)))??;
                Ok(ActionResult::success(format!("Process restarted, new PID: {}", new_pid)))
            }
            DesktopAction::SetProcessPriority { pid, name, priority } => {
                let updated_pid = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.set_process_priority(pid, name.as_deref(), priority)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Priority change failed: {}", e)))??;
                Ok(ActionResult::success(format!("Priority set for PID {}", updated_pid)))
            }
            // ── Window management ──────────────────────────────────────────
            DesktopAction::ListWindows
            | DesktopAction::GetWindowGeometry { .. }
            | DesktopAction::MoveWindow { .. }
            | DesktopAction::ResizeWindow { .. }
            | DesktopAction::MinimizeWindow { .. }
            | DesktopAction::MaximizeWindow { .. } => {
                Err(ComputerError::UnsupportedPlatform(
                    "Window management not available on Wayland".to_string(),
                ))
            }
        }
    }

    async fn wait_for(&self, condition: WaitCondition, timeout: Duration) -> Result<bool> {
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
                    // Wayland restricts window title introspection; fall back to
                    // checking whether a process with that name is running.
                    let output = tokio::process::Command::new("pidof")
                        .arg(pattern)
                        .output()
                        .await;
                    match output {
                        Ok(out) => !out.stdout.is_empty(),
                        _ => false,
                    }
                }
                WaitCondition::ProcessRunning { name } => {
                    let output = tokio::process::Command::new("pidof")
                        .arg(name)
                        .output()
                        .await;
                    match output {
                        Ok(out) => !out.stdout.is_empty(),
                        _ => false,
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

impl WaylandComputerAdapter {
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
                    .find(|e| {
                        e.label
                            .as_ref()
                            .map(|l| l.contains(&label))
                            .unwrap_or(false)
                    })
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
                            && e.label
                                .as_ref()
                                .map(|l| l.contains(&label))
                                .unwrap_or(false)
                    })
                    .ok_or_else(|| ComputerError::ElementNotFound(format!("{}:{}", role, label)))?;
                let center = el.center();
                Ok((center.x, center.y))
            }
        }
    }
}

// ── Headless Adapter ───────────────────────────────────────────────────────
//
// The HeadlessComputerAdapter is now defined in `headless.rs` and re-exported
// from `computer::mod`.  This module only provides the Linux-specific factory.

// ── Shared action helpers ──────────────────────────────────────────────────

async fn browse_files(
    path: &str,
    filter_description: Option<&str>,
    max_results: Option<usize>,
) -> Result<Vec<FileEntry>> {
    use tokio::fs;

    let mut entries = Vec::new();
    let mut reader = fs::read_dir(path)
        .await
        .map_err(|e| ComputerError::Other(format!("Failed to read directory {}: {}", path, e)))?;

    while let Ok(Some(entry)) = reader.next_entry().await {
        let meta = entry.metadata().await.ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        entries.push(FileEntry {
            path: entry.path().to_string_lossy().to_string(),
            name: entry.file_name().to_string_lossy().to_string(),
            size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified_secs: modified,
            is_directory: meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
        });
    }

    if let Some(filter) = filter_description {
        let lower = filter.to_lowercase();
        if lower.contains("recent") {
            entries.sort_by_key(|a| std::cmp::Reverse(a.modified_secs));
        } else if lower.contains("large") || lower.contains("big") || lower.contains("biggest") {
            entries.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
        } else if lower.contains("directory") || lower.contains("dir") || lower.contains("folder") {
            entries.retain(|e| e.is_directory);
        } else if lower.contains("file") {
            entries.retain(|e| !e.is_directory);
        } else {
            entries.retain(|e| e.name.to_lowercase().contains(&lower));
        }
    }

    if let Some(limit) = max_results {
        entries.truncate(limit);
    }

    Ok(entries)
}

async fn read_file_chunked(path: &str, offset: u64, limit_bytes: u64) -> Result<String> {
    use tokio::fs::File;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = File::open(path)
        .await
        .map_err(|e| ComputerError::Other(format!("Failed to open {}: {}", path, e)))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| ComputerError::Other(format!("Failed to seek {}: {}", path, e)))?;

    let mut buf = vec![0u8; limit_bytes.min(10 * 1024 * 1024) as usize];
    let n = file
        .read(&mut buf)
        .await
        .map_err(|e| ComputerError::Other(format!("Failed to read {}: {}", path, e)))?;
    buf.truncate(n);

    String::from_utf8(buf)
        .map_err(|e| ComputerError::Other(format!("File {} contains non-UTF-8 bytes: {}", path, e)))
}


async fn edit_file(path: &str, search: &str, replace: &str) -> Result<ActionResult> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ComputerError::Other(format!("Failed to read {}: {}", path, e)))?;
    let new_content = content.replace(search, replace);
    tokio::fs::write(path, new_content)
        .await
        .map_err(|e| ComputerError::Other(format!("Failed to write {}: {}", path, e)))?;
    Ok(ActionResult::success(format!("Edited {}", path)))
}


// ── Factory ────────────────────────────────────────────────────────────────

/// Create the appropriate Linux adapter.
pub async fn create(registry: Arc<ToolRegistry>) -> Result<Box<dyn ComputerAdapter>> {
    if crate::computer::has_wayland() {
        Ok(Box::new(WaylandComputerAdapter::new(registry)))
    } else if crate::computer::has_x11() {
        Ok(Box::new(X11ComputerAdapter::new(registry)))
    } else {
        Ok(Box::new(super::HeadlessComputerAdapter::new()))
    }
}
