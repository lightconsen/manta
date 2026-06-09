//! Windows Computer adapter — wraps Windows CapabilitySet tools.

use crate::computer::{
    ActionResult, ClickTarget, ComputerAdapter, ComputerError, CompressionFormat, DesktopAction,
    FileEntry, MouseButton, PackageManager, Rect, Result, Screenshot, UiElement, WaitCondition,
};
use crate::tools::ToolRegistry;
use std::sync::Arc;
use std::time::Duration;

/// Windows adapter backed by `windows_screenshot`, `windows_desktop_control`,
/// `windows_clipboard`, and `windows_powershell` tools.
pub struct WindowsComputerAdapter {
    registry: Arc<ToolRegistry>,
    file_watcher: tokio::sync::Mutex<Option<crate::computer::FileWatcher>>,
}

impl WindowsComputerAdapter {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            file_watcher: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl ComputerAdapter for WindowsComputerAdapter {
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
            .execute("windows_screenshot", args, &crate::tools::ToolContext::default())
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
            .execute("windows_accessibility", args, &crate::tools::ToolContext::default())
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
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
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
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
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
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
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
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
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
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::Type { text } => {
                let args = serde_json::json!({ "action": "type", "text": text });
                let result = self
                    .registry
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::KeyPress { keys } => {
                let args = serde_json::json!({ "action": "key", "keys": keys });
                let result = self
                    .registry
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardGet => {
                let args = serde_json::json!({ "action": "get" });
                let result = self
                    .registry
                    .execute("windows_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("clipboard tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::ClipboardSet { text } => {
                let args = serde_json::json!({ "action": "set", "text": text });
                let result = self
                    .registry
                    .execute("windows_clipboard", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("clipboard tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                Ok(ActionResult::success(result.output))
            }
            DesktopAction::LaunchApp { name, wait_for_ready, .. } => {
                let script = format!(
                    r#"Start-Process "{}""#,
                    name.replace('"', "\""")
                );
                let args = serde_json::json!({ "script": script });
                self.registry
                    .execute("windows_powershell", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("powershell tool not found".to_string()))?
                    .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;

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
            DesktopAction::ActivateWindow { title_pattern } => {
                let args = serde_json::json!({
                    "action": "activate_window",
                    "name": title_pattern,
                });
                let result = self
                    .registry
                    .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                    .await
                    .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
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
            DesktopAction::KeySequence { keys, delays_ms } => {
                for (i, key) in keys.iter().enumerate() {
                    let delay_ms = delays_ms.get(i).copied().unwrap_or(0);
                    if delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                    let args = serde_json::json!({ "action": "key", "keys": [key] });
                    self.registry
                        .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                        .await
                        .ok_or_else(|| ComputerError::ToolFailed("desktop control not found".to_string()))?
                        .map_err(|e| ComputerError::ToolFailed(e.to_string()))?;
                }
                Ok(ActionResult::success("Key sequence executed"))
            }
            DesktopAction::ListPorts {
                filter_protocol,
                filter_state,
            } => {
                let inspector = crate::computer::network::NetworkInspector::new();
                let filter_protocol = filter_protocol.clone();
                let filter_state = filter_state.clone();
                let ports = tokio::task::spawn_blocking(move || {
                    inspector.list_ports(filter_protocol.as_deref(), filter_state.as_deref())
                })
                .await
                .map_err(|e| ComputerError::Other(format!("list ports failed: {}", e)))?
                .map_err(|e| ComputerError::Other(format!("list ports failed: {}", e)))?;
                Ok(ActionResult::success(format!("Found {} ports", ports.len())).with_data(
                    serde_json::to_value(&ports).unwrap_or_default(),
                ))
            }
            DesktopAction::TestPing { target, count } => {
                let inspector = crate::computer::network::NetworkInspector::new();
                let result = inspector.test_ping(&target, count).await;
                Ok(ActionResult::success(result.message.clone()).with_data(
                    serde_json::to_value(&result).unwrap_or_default(),
                ))
            }
            DesktopAction::TestTcpConnect {
                target,
                port,
                timeout_ms,
            } => {
                let inspector = crate::computer::network::NetworkInspector::new();
                let timeout = timeout_ms.map(Duration::from_millis);
                let result = inspector.test_tcp_connect(&target, port, timeout).await;
                Ok(ActionResult::success(result.message.clone()).with_data(
                    serde_json::to_value(&result).unwrap_or_default(),
                ))
            }
            DesktopAction::ListFirewallRules => {
                let inspector = crate::computer::network::NetworkInspector::new();
                let rules = inspector
                    .list_firewall_rules()
                    .await
                    .map_err(|e| ComputerError::Other(e.to_string()))?;
                Ok(ActionResult::success(format!("Found {} firewall rules", rules.len())).with_data(
                    serde_json::to_value(&rules).unwrap_or_default(),
                ))
            }
            DesktopAction::BrowseFiles {
                path,
                filter_description,
                max_results,
            } => {
                let entries = browse_files(&path, filter_description.as_deref(), max_results)
                    .await?;
                Ok(ActionResult::success(format!("Found {} entries", entries.len())).with_data(
                    serde_json::to_value(&entries).unwrap_or_default(),
                ))
            }
            DesktopAction::ReadFileChunked {
                path,
                offset,
                limit_bytes,
            } => {
                let content = read_file_chunked(&path, offset, limit_bytes).await?;
                Ok(ActionResult::success(format!("Read {} bytes", content.len())).with_data(
                    serde_json::json!({ "content": content }),
                ))
            }
            DesktopAction::InstallPackage {
                manager,
                packages,
                timeout_secs,
            } => {
                install_package_windows(manager, &packages, timeout_secs).await
            }
            DesktopAction::Compress {
                sources,
                destination,
                format,
            } => {
                compress_files_windows(&sources, &destination, format).await
            }
            DesktopAction::Decompress {
                archive,
                destination,
            } => {
                decompress_archive_windows(&archive, &destination).await
            }
            DesktopAction::WatchDirectory { path } => {
                let mut guard = self.file_watcher.lock().await;
                if guard.is_none() {
                    let watcher = crate::computer::FileWatcher::new()
                        .map_err(|e| ComputerError::Other(format!("Failed to create file watcher: {}", e)))?;
                    *guard = Some(watcher);
                }
                guard
                    .as_mut()
                    .unwrap()
                    .watch_directory(&path)
                    .map_err(|e| ComputerError::Other(format!("Failed to watch directory: {}", e)))?;
                Ok(ActionResult::success(format!("Watching directory: {}", path)))
            }
            DesktopAction::UnwatchDirectory { path } => {
                let mut guard = self.file_watcher.lock().await;
                if let Some(ref mut watcher) = *guard {
                    watcher
                        .unwatch_directory(&path)
                        .map_err(|e| ComputerError::Other(format!("Failed to unwatch directory: {}", e)))?;
                }
                Ok(ActionResult::success(format!("Stopped watching directory: {}", path)))
            }
            DesktopAction::WatchFile { path } => {
                let mut guard = self.file_watcher.lock().await;
                if guard.is_none() {
                    let watcher = crate::computer::FileWatcher::new()
                        .map_err(|e| ComputerError::Other(format!("Failed to create file watcher: {}", e)))?;
                    *guard = Some(watcher);
                }
                guard
                    .as_mut()
                    .unwrap()
                    .watch_file(&path)
                    .map_err(|e| ComputerError::Other(format!("Failed to watch file: {}", e)))?;
                Ok(ActionResult::success(format!("Watching file: {}", path)))
            }
            DesktopAction::UnwatchFile { path } => {
                let mut guard = self.file_watcher.lock().await;
                if let Some(ref mut watcher) = *guard {
                    watcher
                        .unwatch_file(&path)
                        .map_err(|e| ComputerError::Other(format!("Failed to unwatch file: {}", e)))?;
                }
                Ok(ActionResult::success(format!("Stopped watching file: {}", path)))
            }
            DesktopAction::EditFile { path, search, replace } => {
                edit_file(&path, &search, &replace).await
            }
            DesktopAction::TransferFile {
                source,
                destination,
                method,
            } => {
                transfer_file_windows(&source, &destination, method).await
            }
            _ => Err(ComputerError::Other(
                "Action not yet implemented on Windows".to_string(),
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
                    let args = serde_json::json!({ "action": "list_windows" });
                    if let Some(Ok(result)) = self
                        .registry
                        .execute("windows_desktop_control", args, &crate::tools::ToolContext::default())
                        .await
                    {
                        result.output.contains(pattern)
                    } else {
                        false
                    }
                }
                WaitCondition::ProcessRunning { name } => {
                    let script = format!(
                        r#"Get-Process -Name "{}" -ErrorAction SilentlyContinue | Select-Object -First 1"#,
                        name
                    );
                    let args = serde_json::json!({ "script": script });
                    if let Some(Ok(result)) = self
                        .registry
                        .execute("windows_powershell", args, &crate::tools::ToolContext::default())
                        .await
                    {
                        !result.output.trim().is_empty()
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

impl WindowsComputerAdapter {
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
            entries.sort_by(|a, b| b.modified_secs.cmp(&a.modified_secs));
        } else if lower.contains("large") || lower.contains("big") || lower.contains("biggest") {
            entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
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

    String::from_utf8(buf).map_err(|e| {
        ComputerError::Other(format!("File {} contains non-UTF-8 bytes: {}", path, e))
    })
}

async fn install_package_windows(
    manager: PackageManager,
    packages: &[String],
    timeout_secs: u64,
) -> Result<ActionResult> {
    let (cmd, args): (&str, Vec<String>) = match manager {
        PackageManager::Winget => {
            let mut v = vec![
                "install".to_string(),
                "--accept-package-agreements".to_string(),
                "--accept-source-agreements".to_string(),
                "-e".to_string(),
            ];
            v.extend(packages.iter().cloned());
            ("winget", v)
        }
        PackageManager::Choco => {
            let mut v = vec!["install".to_string(), "-y".to_string()];
            v.extend(packages.iter().cloned());
            ("choco", v)
        }
        _ => {
            return Err(ComputerError::UnsupportedPlatform(
                format!("Package manager {:?} not supported on Windows", manager),
            ))
        }
    };

    let output = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::process::Command::new(cmd).args(&args).output(),
    )
    .await
    .map_err(|_| ComputerError::Timeout)?
    .map_err(|e| ComputerError::ToolFailed(format!("Failed to run {}: {}", cmd, e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(ActionResult::error(format!("{} install failed: {}", cmd, stderr)));
    }

    Ok(ActionResult::success(format!(
        "Installed {} with {}",
        packages.join(", "),
        cmd
    )))
}

async fn compress_files_windows(
    sources: &[String],
    destination: &str,
    format: CompressionFormat,
) -> Result<ActionResult> {
    match format {
        CompressionFormat::Zip => {
            let src_list = sources
                .iter()
                .map(|s| format!("'{}'", s.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",");
            let script = format!(
                "Compress-Archive -Path {} -DestinationPath '{}' -Force",
                src_list,
                destination.replace('\'', "''")
            );
            let args = serde_json::json!({ "script": script });
            let output = self_registry_execute_powershell(args).await?;
            if !output.status.success() {
                return Ok(ActionResult::error(format!(
                    "Compress-Archive failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            Ok(ActionResult::success(format!("Created {}", destination)))
        }
        CompressionFormat::Tar => {
            let mut args = vec!["-cvf".to_string(), destination.to_string()];
            args.extend(sources.iter().cloned());
            run_tar_windows(&args).await
        }
        CompressionFormat::TarGz => {
            let mut args = vec!["-czvf".to_string(), destination.to_string()];
            args.extend(sources.iter().cloned());
            run_tar_windows(&args).await
        }
        CompressionFormat::TarBz2 => {
            let mut args = vec!["-cjvf".to_string(), destination.to_string()];
            args.extend(sources.iter().cloned());
            run_tar_windows(&args).await
        }
        CompressionFormat::TarXz => {
            let mut args = vec!["-cJvf".to_string(), destination.to_string()];
            args.extend(sources.iter().cloned());
            run_tar_windows(&args).await
        }
        _ => Err(ComputerError::UnsupportedPlatform(
            format!("Compression format {:?} not supported on Windows", format),
        )),
    }
}

async fn run_tar_windows(args: &[String]) -> Result<ActionResult> {
    let output = tokio::process::Command::new("tar")
        .args(args)
        .output()
        .await
        .map_err(|e| ComputerError::ToolFailed(format!("Failed to run tar: {}", e)))?;
    if !output.status.success() {
        return Ok(ActionResult::error(format!(
            "tar failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(ActionResult::success("Archive created".to_string()))
}

async fn decompress_archive_windows(archive: &str, destination: &str) -> Result<ActionResult> {
    tokio::fs::create_dir_all(destination)
        .await
        .map_err(|e| ComputerError::Other(format!("Failed to create {}: {}", destination, e)))?;

    let lower = archive.to_lowercase();
    if lower.ends_with(".zip") {
        let script = format!(
            "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
            archive.replace('\'', "''"),
            destination.replace('\'', "''")
        );
        let args = serde_json::json!({ "script": script });
        let output = self_registry_execute_powershell(args).await?;
        if !output.status.success() {
            return Ok(ActionResult::error(format!(
                "Expand-Archive failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    } else {
        let args: Vec<String> = if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            vec!["-xzvf".to_string(), archive.to_string(), "-C".to_string(), destination.to_string()]
        } else if lower.ends_with(".tar.bz2") {
            vec!["-xjvf".to_string(), archive.to_string(), "-C".to_string(), destination.to_string()]
        } else if lower.ends_with(".tar.xz") {
            vec!["-xJvf".to_string(), archive.to_string(), "-C".to_string(), destination.to_string()]
        } else if lower.ends_with(".tar") {
            vec!["-xvf".to_string(), archive.to_string(), "-C".to_string(), destination.to_string()]
        } else {
            return Err(ComputerError::UnsupportedPlatform(
                format!("Cannot determine archive format for {}", archive),
            ));
        };
        let output = tokio::process::Command::new("tar")
            .args(&args)
            .output()
            .await
            .map_err(|e| ComputerError::ToolFailed(format!("Failed to run tar: {}", e)))?;
        if !output.status.success() {
            return Ok(ActionResult::error(format!(
                "tar failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }

    Ok(ActionResult::success(format!(
        "Extracted {} to {}",
        archive, destination
    )))
}

/// Invoke the windows_powershell tool via the adapter's registry.
///
/// This helper is standalone so compression helpers can call it without
/// holding `&self`.
async fn self_registry_execute_powershell(
    args: serde_json::Value,
) -> std::io::Result<std::process::Output> {
    // The tool registry requires `&self`, which we don't have in free functions.
    // Fallback to invoking `powershell` directly for compress/decompress scripts.
    let script = args
        .get("script")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .output()
        .await
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

async fn transfer_file_windows(
    source: &str,
    destination: &str,
    method: crate::computer::TransferMethod,
) -> Result<ActionResult> {
    match method {
        crate::computer::TransferMethod::Scp => {
            let output = tokio::process::Command::new("scp")
                .args(&[source, destination])
                .output()
                .await
                .map_err(|e| ComputerError::ToolFailed(format!("scp failed: {}", e)))?;
            if !output.status.success() {
                return Ok(ActionResult::error(format!(
                    "scp failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            Ok(ActionResult::success(format!(
                "Transferred {} to {} via scp",
                source, destination
            )))
        }
        crate::computer::TransferMethod::Rsync => {
            let output = tokio::process::Command::new("rsync")
                .args(&["-avz", source, destination])
                .output()
                .await
                .map_err(|e| ComputerError::ToolFailed(format!("rsync failed: {}", e)))?;
            if !output.status.success() {
                return Ok(ActionResult::error(format!(
                    "rsync failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            Ok(ActionResult::success(format!(
                "Transferred {} to {} via rsync",
                source, destination
            )))
        }
        crate::computer::TransferMethod::Smb => {
            // Use PowerShell Copy-Item with UNC path
            let script = format!(
                "Copy-Item -Path '{}' -Destination '{}' -Force -Recurse",
                source.replace('\'', "''"),
                destination.replace('\'', "''")
            );
            let output = tokio::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .output()
                .await
                .map_err(|e| ComputerError::ToolFailed(format!("SMB copy failed: {}", e)))?;
            if !output.status.success() {
                return Ok(ActionResult::error(format!(
                    "SMB copy failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            Ok(ActionResult::success(format!(
                "Transferred {} to {} via SMB",
                source, destination
            )))
        }
    }
}

/// Factory for Windows.
pub async fn create(registry: Arc<ToolRegistry>) -> Result<Box<dyn ComputerAdapter>> {
    Ok(Box::new(WindowsComputerAdapter::new(registry)))
}
