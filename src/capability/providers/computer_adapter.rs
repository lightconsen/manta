//! Bridge [`DesktopAction`](crate::computer::DesktopAction) → [`Capability`](super::super::Capability).
//!
//! Each [`ComputerCapability`] wraps a single [`DesktopAction`] variant,
//! allowing it to be registered in a [`CapabilityRegistry`] and invoked
//! through the unified capability interface.

use crate::capability::{Capability, CapabilityResult};
use crate::computer::{ComputerAdapter, DesktopAction};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// A [`Capability`] backed by a specific [`DesktopAction`].
///
/// The adapter holds a reference to the [`ComputerAdapter`] and executes
/// the wrapped action through it.
pub struct ComputerCapability {
    name: String,
    action: DesktopAction,
    adapter: Arc<dyn ComputerAdapter>,
}

impl std::fmt::Debug for ComputerCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputerCapability")
            .field("name", &self.name)
            .field("action", &self.action)
            .finish()
    }
}

impl ComputerCapability {
    /// Create a new computer capability wrapping `action`.
    ///
    /// The capability name is derived from the action variant
    /// (e.g. `"screenshot"`, `"click"`, `"type"`).
    pub fn new(adapter: Arc<dyn ComputerAdapter>, action: DesktopAction) -> Self {
        let name = action_variant_name(&action);
        Self {
            name,
            action,
            adapter,
        }
    }

    /// Create a capability with an explicit name override.
    pub fn with_name(
        adapter: Arc<dyn ComputerAdapter>,
        action: DesktopAction,
        name: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            action,
            adapter,
        }
    }

    /// Returns a reference to the inner action.
    pub fn action(&self) -> &DesktopAction {
        &self.action
    }
}

#[async_trait]
impl Capability for ComputerCapability {
    fn name(&self) -> &str {
        &self.name
    }

    fn param_schema(&self) -> Value {
        // Computer capabilities currently accept no additional parameters;
        // the action itself carries all configuration.
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(&self, _params: Value) -> CapabilityResult {
        let start = std::time::Instant::now();

        match self.adapter.execute(self.action.clone()).await {
            Ok(result) => CapabilityResult {
                success: result.success,
                output: Some(serde_json::json!({
                    "message": result.message,
                    "data": result.data,
                })),
                error: if result.success { None } else { Some(result.message) },
                duration_ms: start.elapsed().as_millis() as u64,
            },
            Err(err) => CapabilityResult {
                success: false,
                output: None,
                error: Some(err.to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
            },
        }
    }
}

/// Derive a human-readable name from a `DesktopAction` variant.
fn action_variant_name(action: &DesktopAction) -> String {
    match action {
        DesktopAction::Screenshot { .. } => "screenshot".into(),
        DesktopAction::Click { .. } => "click".into(),
        DesktopAction::DoubleClick { .. } => "double_click".into(),
        DesktopAction::Type { .. } => "type".into(),
        DesktopAction::KeyPress { .. } => "key_press".into(),
        DesktopAction::Scroll { .. } => "scroll".into(),
        DesktopAction::Drag { .. } => "drag".into(),
        DesktopAction::ReadUiTree { .. } => "read_ui_tree".into(),
        DesktopAction::LaunchApp { .. } => "launch_app".into(),
        DesktopAction::ActivateWindow { .. } => "activate_window".into(),
        DesktopAction::CloseWindow { .. } => "close_window".into(),
        DesktopAction::Wait { .. } => "wait".into(),
        DesktopAction::ClipboardGet => "clipboard_get".into(),
        DesktopAction::ClipboardSet { .. } => "clipboard_set".into(),
        DesktopAction::GetSystemStatus => "get_system_status".into(),
        DesktopAction::ListProcesses { .. } => "list_processes".into(),
        DesktopAction::KillProcess { .. } => "kill_process".into(),
        DesktopAction::WatchDirectory { .. } => "watch_directory".into(),
        DesktopAction::UnwatchDirectory { .. } => "unwatch_directory".into(),
        DesktopAction::WatchFile { .. } => "watch_file".into(),
        DesktopAction::UnwatchFile { .. } => "unwatch_file".into(),
        DesktopAction::ListPorts { .. } => "list_ports".into(),
        DesktopAction::TestPing { .. } => "test_ping".into(),
        DesktopAction::TestTcpConnect { .. } => "test_tcp_connect".into(),
        DesktopAction::ListFirewallRules => "list_firewall_rules".into(),
        DesktopAction::RestartProcess { .. } => "restart_process".into(),
        DesktopAction::SetProcessPriority { .. } => "set_process_priority".into(),
        DesktopAction::KeySequence { .. } => "key_sequence".into(),
        DesktopAction::InstallPackage { .. } => "install_package".into(),
        DesktopAction::BrowseFiles { .. } => "browse_files".into(),
        DesktopAction::ReadFileChunked { .. } => "read_file_chunked".into(),
        DesktopAction::EditFile { .. } => "edit_file".into(),
        DesktopAction::Compress { .. } => "compress".into(),
        DesktopAction::Decompress { .. } => "decompress".into(),
        DesktopAction::TransferFile { .. } => "transfer_file".into(),
    }
}

/// Generate a list of all standard [`ComputerCapability`] wrappers for an adapter.
///
/// This provides the "default set" of desktop capabilities — one per variant.
pub fn all_computer_capabilities(
    adapter: Arc<dyn ComputerAdapter>,
) -> Vec<ComputerCapability> {
    vec![
        ComputerCapability::new(adapter.clone(), DesktopAction::Screenshot { region: None }),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::Click {
                target: crate::computer::ClickTarget::Coordinate(
                    crate::computer::Point::new(0, 0),
                ),
                button: crate::computer::MouseButton::Left,
            },
        ),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::DoubleClick {
                target: crate::computer::ClickTarget::Coordinate(
                    crate::computer::Point::new(0, 0),
                ),
                button: crate::computer::MouseButton::Left,
            },
        ),
        ComputerCapability::new(adapter.clone(), DesktopAction::Type { text: String::new() }),
        ComputerCapability::new(adapter.clone(), DesktopAction::KeyPress { keys: vec![] }),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::Scroll {
                target: crate::computer::ClickTarget::Coordinate(
                    crate::computer::Point::new(0, 0),
                ),
                direction: crate::computer::ScrollDirection::Down,
                amount: 1,
            },
        ),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::Drag {
                from: crate::computer::ClickTarget::Coordinate(
                    crate::computer::Point::new(0, 0),
                ),
                to: crate::computer::ClickTarget::Coordinate(
                    crate::computer::Point::new(0, 0),
                ),
            },
        ),
        ComputerCapability::new(adapter.clone(), DesktopAction::ReadUiTree { app: None }),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::LaunchApp {
                name: String::new(),
                args: vec![],
                wait_for_ready: true,
            },
        ),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::ActivateWindow {
                title_pattern: String::new(),
            },
        ),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::CloseWindow {
                title_pattern: String::new(),
            },
        ),
        ComputerCapability::new(adapter.clone(), DesktopAction::Wait { milliseconds: 100 }),
        ComputerCapability::new(adapter.clone(), DesktopAction::ClipboardGet),
        ComputerCapability::new(
            adapter.clone(),
            DesktopAction::ClipboardSet { text: String::new() },
        ),
        ComputerCapability::new(adapter.clone(), DesktopAction::GetSystemStatus),
        ComputerCapability::new(adapter.clone(), DesktopAction::ListFirewallRules),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::{ActionResult, ComputerError};

    struct MockAdapter;

    #[async_trait]
    impl ComputerAdapter for MockAdapter {
        async fn screenshot(
            &self,
            _region: Option<crate::computer::Rect>,
        ) -> crate::computer::Result<crate::computer::Screenshot> {
            Err(ComputerError::Other("not implemented".into()))
        }
        async fn read_ui_tree(
            &self,
            _app: Option<&str>,
        ) -> crate::computer::Result<Vec<crate::computer::UiElement>> {
            Err(ComputerError::Other("not implemented".into()))
        }
        async fn execute(
            &self,
            action: DesktopAction,
        ) -> crate::computer::Result<ActionResult> {
            match action {
                DesktopAction::Screenshot { .. } => Ok(ActionResult::success("screenshot taken")),
                DesktopAction::ClipboardGet => Ok(ActionResult::success("clipboard content")),
                _ => Err(ComputerError::Other("mock error".into())),
            }
        }
        async fn wait_for(
            &self,
            _condition: crate::computer::WaitCondition,
            _timeout: std::time::Duration,
        ) -> crate::computer::Result<bool> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn test_computer_capability_name() {
        let adapter = Arc::new(MockAdapter);
        let cap = ComputerCapability::new(adapter, DesktopAction::Screenshot { region: None });
        assert_eq!(cap.name(), "screenshot");
    }

    #[tokio::test]
    async fn test_computer_capability_execute_success() {
        let adapter = Arc::new(MockAdapter);
        let cap = ComputerCapability::new(adapter, DesktopAction::Screenshot { region: None });
        let result = cap.execute(Value::Null).await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_computer_capability_execute_error() {
        let adapter = Arc::new(MockAdapter);
        let cap = ComputerCapability::new(
            adapter,
            DesktopAction::Click {
                target: crate::computer::ClickTarget::Coordinate(
                    crate::computer::Point::new(0, 0),
                ),
                button: crate::computer::MouseButton::Left,
            },
        );
        let result = cap.execute(Value::Null).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("mock error"));
    }

    #[test]
    fn test_action_variant_names() {
        assert_eq!(
            action_variant_name(&DesktopAction::Screenshot { region: None }),
            "screenshot"
        );
        assert_eq!(
            action_variant_name(&DesktopAction::ClipboardGet),
            "clipboard_get"
        );
        assert_eq!(
            action_variant_name(&DesktopAction::GetSystemStatus),
            "get_system_status"
        );
    }

    #[test]
    fn test_all_computer_capabilities_count() {
        let adapter = Arc::new(MockAdapter);
        let caps = all_computer_capabilities(adapter);
        assert!(caps.len() >= 14); // at least the explicitly listed caps
    }
}
