//! Headless computer adapter — virtual display for CI/CD and server environments.
//!
//! When no physical display is available, this adapter spins up a virtual
//! framebuffer (Xvfb on Linux) so that desktop automation can still run.
//!
//! ```text
//! Agent → HeadlessComputerAdapter → VirtualDisplay → Xvfb
//!                                     │
//!                                     └── screenshot (x11grab / import)
//!                                     └── click/type (xdotool via DISPLAY)
//! ```

use crate::computer::{
    ActionResult, ClickTarget, ComputerAdapter, ComputerError, DesktopAction, MouseButton,
    Rect, Result, Screenshot, UiElement, WaitCondition,
};
use crate::tools::ToolRegistry;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

// ── Virtual Display abstraction ────────────────────────────────────────────

/// A virtual display that can host GUI applications without a physical monitor.
#[async_trait::async_trait]
pub trait VirtualDisplay: Send + Sync {
    /// The DISPLAY value (e.g. ":99") used by X11 clients.
    fn display(&self) -> &str;

    /// Capture a screenshot from the virtual framebuffer.
    async fn capture(&self, region: Option<Rect>) -> Result<Screenshot>;

    /// Gracefully shut down the virtual display.
    async fn shutdown(&mut self) -> Result<()>;
}

// ── Linux Xvfb implementation ──────────────────────────────────────────────

/// Xvfb-backed virtual display.
pub struct XvfbDisplay {
    #[allow(dead_code)]
    display_num: u16,
    display: String,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
    child: Option<tokio::process::Child>,
}

impl XvfbDisplay {
    /// Try to start an Xvfb instance.
    ///
    /// Tries display numbers starting from `:99` upward until one is free.
    pub async fn start(width: u32, height: u32) -> Result<Self> {
        for display_num in 99..=199u16 {
            let display = format!(":{}", display_num);
            match Self::try_start(display_num, width, height).await {
                Ok(child) => {
                    // Give Xvfb a moment to initialise.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    return Ok(Self {
                        display_num,
                        display,
                        width,
                        height,
                        child: Some(child),
                    });
                }
                Err(e) => {
                    tracing::debug!(
                        "Xvfb on display {} failed to start: {}",
                        display_num,
                        e
                    );
                    continue;
                }
            }
        }
        Err(ComputerError::Other(
            "Could not start Xvfb on any display 99-199".to_string(),
        ))
    }

    async fn try_start(
        display_num: u16,
        width: u32,
        height: u32,
    ) -> std::io::Result<tokio::process::Child> {
        tokio::process::Command::new("Xvfb")
            .arg(format!(":{}", display_num))
            .arg("-screen")
            .arg("0")
            .arg(format!("{}x{}x24", width, height))
            .arg("+extension")
            .arg("RANDR")
            .arg("-ac")
            .arg("-nolisten")
            .arg("tcp")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
    }

    /// Run a command with the correct DISPLAY environment set.
    async fn run_with_display(
        &self,
        cmd: &str,
        args: &[&str],
    ) -> std::io::Result<std::process::Output> {
        tokio::process::Command::new(cmd)
            .args(args)
            .env("DISPLAY", &self.display)
            .output()
            .await
    }
}

#[async_trait::async_trait]
impl VirtualDisplay for XvfbDisplay {
    fn display(&self) -> &str {
        &self.display
    }

    async fn capture(&self, region: Option<Rect>) -> Result<Screenshot> {
        // Strategy 1: ImageMagick import (best quality / simplest)
        let mut import_cmd = tokio::process::Command::new("import");
        import_cmd
            .env("DISPLAY", &self.display)
            .arg("-window")
            .arg("root")
            .arg("png:-");

        if let Some(r) = region {
            import_cmd
                .arg("-crop")
                .arg(format!("{}x{}+{}+{}", r.width, r.height, r.x, r.y));
        }

        match import_cmd.output().await {
            Ok(output) if output.status.success() => {
                let base64 = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &output.stdout,
                );
                return Ok(Screenshot {
                    base64,
                    width: self.width,
                    height: self.height,
                });
            }
            Ok(output) => {
                tracing::warn!(
                    "ImageMagick import failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                tracing::warn!("ImageMagick import not available: {}", e);
            }
        }

        // Strategy 2: xwd + convert (fallback)
        let xwd_output = self
            .run_with_display("xwd", &["-root", "-silent"])
            .await
            .map_err(|e| ComputerError::ScreenshotFailed(format!("xwd failed: {}", e)))?;

        if !xwd_output.status.success() {
            return Err(ComputerError::ScreenshotFailed(
                "xwd capture failed".to_string(),
            ));
        }

        let mut child = tokio::process::Command::new("convert")
            .arg("xwd:-")
            .arg("png:-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ComputerError::ScreenshotFailed(format!("convert failed: {}", e)))?;

        {
            use tokio::io::AsyncWriteExt;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(&xwd_output.stdout)
                    .await
                    .map_err(|e| ComputerError::ScreenshotFailed(format!("convert stdin: {}", e)))?;
            }
        }

        let convert_output = child
            .wait_with_output()
            .await
            .map_err(|e| ComputerError::ScreenshotFailed(format!("convert failed: {}", e)))?;

        if !convert_output.status.success() {
            return Err(ComputerError::ScreenshotFailed(
                "xwd→png conversion failed".to_string(),
            ));
        }

        let base64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &convert_output.stdout,
        );

        Ok(Screenshot {
            base64,
            width: self.width,
            height: self.height,
        })
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        Ok(())
    }
}

impl Drop for XvfbDisplay {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

// ── Headless Adapter ───────────────────────────────────────────────────────

/// Headless computer adapter that optionally uses a virtual display.
///
/// When `virtual_display` is `Some`, GUI actions (screenshot, click, type)
/// are forwarded to the virtual framebuffer.  When `None`, only non-GUI
/// actions (shell, wait, file) are supported.
pub struct HeadlessComputerAdapter {
    #[allow(dead_code)]
    registry: Arc<ToolRegistry>,
    virtual_display: Option<Box<dyn VirtualDisplay>>,
}

impl HeadlessComputerAdapter {
    /// Create a new headless adapter **without** a virtual display.
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            virtual_display: None,
        }
    }

    /// Create a new headless adapter **with** an Xvfb virtual display.
    ///
    /// Only available on Linux.  On other platforms this falls back to
    /// `new()`.
    pub async fn with_xvfb(registry: Arc<ToolRegistry>) -> Self {
        #[cfg(target_os = "linux")]
        {
            match XvfbDisplay::start(1920, 1080).await {
                Ok(display) => {
                    tracing::info!(
                        "Xvfb virtual display started on {}",
                        display.display()
                    );
                    return Self {
                        registry,
                        virtual_display: Some(Box::new(display)),
                    };
                }
                Err(e) => {
                    tracing::warn!("Failed to start Xvfb: {}", e);
                }
            }
        }
        Self::new(registry)
    }

    fn display(&self) -> Option<&str> {
        self.virtual_display.as_ref().map(|d| d.display())
    }
}

#[async_trait::async_trait]
impl ComputerAdapter for HeadlessComputerAdapter {
    async fn screenshot(&self, region: Option<Rect>) -> Result<Screenshot> {
        if let Some(display) = &self.virtual_display {
            display.capture(region).await
        } else {
            Err(ComputerError::NoDisplay)
        }
    }

    async fn read_ui_tree(&self, _app: Option<&str>) -> Result<Vec<UiElement>> {
        // Accessibility tree is not available in headless mode.
        Ok(Vec::new())
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
                let display = self
                    .display()
                    .ok_or_else(|| ComputerError::NoDisplay)?;
                let (x, y) = match target {
                    ClickTarget::Coordinate(p) => (p.x, p.y),
                    _ => {
                        return Err(ComputerError::Other(
                            "Headless adapter only supports coordinate clicks".to_string(),
                        ))
                    }
                };
                let btn = match button {
                    MouseButton::Left => "1",
                    MouseButton::Middle => "2",
                    MouseButton::Right => "3",
                };
                let output = tokio::process::Command::new("xdotool")
                    .env("DISPLAY", display)
                    .args(["mousemove", &x.to_string(), &y.to_string()])
                    .output()
                    .await
                    .map_err(|e| ComputerError::ToolFailed(format!("xdotool: {}", e)))?;
                if !output.status.success() {
                    return Err(ComputerError::ToolFailed(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ));
                }
                let output = tokio::process::Command::new("xdotool")
                    .env("DISPLAY", display)
                    .args(["click", btn])
                    .output()
                    .await
                    .map_err(|e| ComputerError::ToolFailed(format!("xdotool: {}", e)))?;
                if !output.status.success() {
                    return Err(ComputerError::ToolFailed(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ));
                }
                Ok(ActionResult::success(format!("Clicked at {}, {}", x, y)))
            }
            DesktopAction::Type { text } => {
                let display = self
                    .display()
                    .ok_or_else(|| ComputerError::NoDisplay)?;
                let output = tokio::process::Command::new("xdotool")
                    .env("DISPLAY", display)
                    .args(["type", &text])
                    .output()
                    .await
                    .map_err(|e| ComputerError::ToolFailed(format!("xdotool: {}", e)))?;
                if !output.status.success() {
                    return Err(ComputerError::ToolFailed(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ));
                }
                Ok(ActionResult::success(format!("Typed: {}", text)))
            }
            DesktopAction::KeyPress { keys } => {
                let display = self
                    .display()
                    .ok_or_else(|| ComputerError::NoDisplay)?;
                let key_str = keys.join("+");
                let output = tokio::process::Command::new("xdotool")
                    .env("DISPLAY", display)
                    .args(["key", &key_str])
                    .output()
                    .await
                    .map_err(|e| ComputerError::ToolFailed(format!("xdotool: {}", e)))?;
                if !output.status.success() {
                    return Err(ComputerError::ToolFailed(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ));
                }
                Ok(ActionResult::success(format!("Pressed: {}", key_str)))
            }
            DesktopAction::ClipboardGet | DesktopAction::ClipboardSet { .. } => {
                // Clipboard requires a real X11/Wayland connection
                Err(ComputerError::NoDisplay)
            }
            DesktopAction::LaunchApp { name, args, wait_for_ready } => {
                let mut cmd = tokio::process::Command::new(&name);
                cmd.args(&args);
                if let Some(d) = self.display() {
                    cmd.env("DISPLAY", d);
                }
                let child = cmd
                    .spawn()
                    .map_err(|e| {
                        ComputerError::ToolFailed(format!("Failed to launch {}: {}", name, e))
                    })?;
                drop(child);
                if wait_for_ready {
                    let ready = self
                        .wait_for(
                            WaitCondition::ProcessRunning {
                                name: name.clone(),
                            },
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
            DesktopAction::ActivateWindow { .. } => {
                Err(ComputerError::Other(
                    "Window activation not available in headless mode".to_string(),
                ))
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
            DesktopAction::WatchDirectory { path } => {
                Ok(ActionResult::success(format!(
                    "Watch directory request accepted for {} (headless adapter does not persist watchers)",
                    path
                )))
            }
            DesktopAction::UnwatchDirectory { path } => {
                Ok(ActionResult::success(format!(
                    "Unwatch directory request accepted for {} (headless adapter does not persist watchers)",
                    path
                )))
            }
            DesktopAction::WatchFile { path } => {
                Ok(ActionResult::success(format!(
                    "Watch file request accepted for {} (headless adapter does not persist watchers)",
                    path
                )))
            }
            DesktopAction::UnwatchFile { path } => {
                Ok(ActionResult::success(format!(
                    "Unwatch file request accepted for {} (headless adapter does not persist watchers)",
                    path
                )))
            }
            DesktopAction::ListPorts {
                filter_protocol,
                filter_state,
            } => {
                let inspector = crate::computer::network::NetworkInspector::new();
                let protocol_ref = filter_protocol.as_deref();
                let state_ref = filter_state.as_deref();
                match inspector.list_ports(protocol_ref, state_ref) {
                    Ok(entries) => Ok(ActionResult::success(format!(
                        "Found {} socket entries",
                        entries.len()
                    ))
                    .with_data(serde_json::to_value(&entries).unwrap_or_default())),
                    Err(e) => Err(ComputerError::Other(e.to_string())),
                }
            }
            DesktopAction::TestPing { target, count } => {
                let inspector = crate::computer::network::NetworkInspector::new();
                let result = inspector.test_ping(&target, count).await;
                let success = result.success;
                let message = result.message.clone();
                let mut ar = ActionResult {
                    success,
                    message,
                    screenshot_after: None,
                    data: Some(serde_json::to_value(&result).unwrap_or_default()),
                };
                if !success {
                    ar.success = false;
                }
                Ok(ar)
            }
            DesktopAction::TestTcpConnect {
                target,
                port,
                timeout_ms,
            } => {
                let inspector = crate::computer::network::NetworkInspector::new();
                let timeout = timeout_ms.map(std::time::Duration::from_millis);
                let result = inspector.test_tcp_connect(&target, port, timeout).await;
                let success = result.success;
                let message = result.message.clone();
                let mut ar = ActionResult {
                    success,
                    message,
                    screenshot_after: None,
                    data: Some(serde_json::to_value(&result).unwrap_or_default()),
                };
                if !success {
                    ar.success = false;
                }
                Ok(ar)
            }
            DesktopAction::ListFirewallRules => {
                let inspector = crate::computer::network::NetworkInspector::new();
                match inspector.list_firewall_rules().await {
                    Ok(rules) => Ok(ActionResult::success(format!(
                        "Found {} firewall rules",
                        rules.len()
                    ))
                    .with_data(serde_json::to_value(&rules).unwrap_or_default())),
                    Err(e) => Err(ComputerError::Other(e.to_string())),
                }
            }
            DesktopAction::RestartProcess { pid, name, force } => {
                let new_pid = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.restart_process(pid, name.as_deref(), force)
                })
                .await
                .map_err(|e| ComputerError::Other(format!("Restart failed: {}", e)))??;
                Ok(ActionResult::success(format!(
                    "Process restarted, new PID: {}",
                    new_pid
                )))
            }
            DesktopAction::SetProcessPriority {
                pid,
                name,
                priority,
            } => {
                let updated_pid = tokio::task::spawn_blocking(move || {
                    let mut monitor = crate::computer::system::SystemMonitor::new();
                    monitor.set_process_priority(pid, name.as_deref(), priority)
                })
                .await
                .map_err(|e| {
                    ComputerError::Other(format!("Priority change failed: {}", e))
                })??;
                Ok(ActionResult::success(format!(
                    "Priority set for PID {}",
                    updated_pid
                )))
            }
            _ => Err(ComputerError::Other(
                "Action not available in headless mode".to_string(),
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
                WaitCondition::ProcessRunning { name } => {
                    let output = tokio::process::Command::new("pgrep")
                        .arg(name)
                        .output()
                        .await;
                    matches!(output, Ok(out) if out.status.success())
                }
                WaitCondition::ProcessExited { name } => {
                    let output = tokio::process::Command::new("pgrep")
                        .arg(name)
                        .output()
                        .await;
                    !matches!(output, Ok(out) if out.status.success())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_headless_adapter_without_display() {
        let adapter = HeadlessComputerAdapter::new(Arc::new(ToolRegistry::default()));
        assert!(adapter.display().is_none());
    }
}
