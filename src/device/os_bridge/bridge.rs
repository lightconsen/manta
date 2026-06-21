//! OS device bridge event loop.
//!
//! Listens to OS device plug/unplug events, matches them against
//! registered [`DeviceMatcher`] entries, and auto-probes/connects/disconnects
//! devices through the [`DeviceRegistry`].
//!
//! # Architecture
//!
//! ```text
//! create_os_monitor()
//!    │  broadcast::Receiver<OsDeviceEvent>
//!    ▼
//! handle_os_event()
//!    │
//!    ├── Added   → match → build driver → register → probe → connect → register tools
//!    ├── Removed → lookup devnode → disconnect → deregister tools
//!    └── Changed → re-probe / reconnect
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::platform::create_os_monitor;
use super::{MatcherEntry, OsDeviceAction, OsDeviceEvent};
use crate::device::registry::DeviceRegistry;
use crate::device::DeviceDriver;
use crate::perception::{DeviceSourceAdapter, PerceptionRegistry};
use crate::tools::device_tool::DeviceToolWrapper;
use crate::tools::ToolRegistry;

/// Type of a driver builder closure — maps a driver kind string and JSON
/// parameters to a boxed driver instance.
///
/// Using a closure avoids a circular dependency between
/// `device::os_bridge` and `gateway::init::devices::DriverFactory`.
pub type DriverBuilder =
    Arc<dyn Fn(&str, serde_json::Value) -> crate::Result<Arc<dyn DeviceDriver>> + Send + Sync>;

/// Spawn the OS device bridge event loop.
///
/// Creates the platform-specific OS monitor, subscribes to device events,
/// and dispatches them to the matching / connection logic.
///
/// Returns a `JoinHandle` that can be aborted during shutdown or config reload.
pub fn spawn_os_bridge_loop(
    registry: Arc<DeviceRegistry>,
    matchers: Vec<MatcherEntry>,
    tool_registry: Arc<ToolRegistry>,
    perception_registry: Option<Arc<PerceptionRegistry>>,
    build_driver: DriverBuilder,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let monitor = create_os_monitor();
        let mut rx = monitor.subscribe();

        // devnode → (driver_name, device_id) tracking
        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        loop {
            match rx.recv().await {
                Ok(event) => {
                    handle_os_event(
                        &registry,
                        &matchers,
                        &tool_registry,
                        perception_registry.as_deref(),
                        &build_driver,
                        &mut devnode_map,
                        event,
                    )
                    .await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("OS bridge monitor lagged by {n} events");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("OS bridge monitor channel closed, shutting down");
                    break;
                }
            }
        }
    })
}

/// Dispatch an [`OsDeviceEvent`] to the appropriate handler based on action.
async fn handle_os_event(
    registry: &DeviceRegistry,
    matchers: &[MatcherEntry],
    tool_registry: &ToolRegistry,
    perception_registry: Option<&PerceptionRegistry>,
    build_driver: &DriverBuilder,
    devnode_map: &mut HashMap<String, (String, String)>,
    event: OsDeviceEvent,
) {
    match event.action {
        OsDeviceAction::Added => {
            handle_added(
                registry,
                matchers,
                tool_registry,
                perception_registry,
                build_driver,
                devnode_map,
                event,
            )
            .await;
        }
        OsDeviceAction::Removed => {
            handle_removed(registry, tool_registry, perception_registry, devnode_map, event).await;
        }
        OsDeviceAction::Changed => {
            handle_changed(
                registry,
                matchers,
                tool_registry,
                perception_registry,
                build_driver,
                devnode_map,
                event,
            )
            .await;
        }
    }
}

/// Handle a device Added event: match, build driver, probe, connect, register
/// tools and perception sources.
async fn handle_added(
    registry: &DeviceRegistry,
    matchers: &[MatcherEntry],
    tool_registry: &ToolRegistry,
    perception_registry: Option<&PerceptionRegistry>,
    build_driver: &DriverBuilder,
    devnode_map: &mut HashMap<String, (String, String)>,
    event: OsDeviceEvent,
) {
    // Skip if we already know this devnode
    if let Some(ref devnode) = event.devnode {
        if devnode_map.contains_key(devnode) {
            return;
        }
    }

    for entry in matchers {
        if !entry.matcher.matches(&event) {
            continue;
        }

        let driver = match build_driver(&entry.driver_kind, entry.params.clone()) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("OS bridge: failed to build driver '{}': {e}", entry.driver_kind);
                continue;
            }
        };

        let driver_name = driver.driver_name().to_string();

        // Register the driver with the registry (async, Arc-safe method)
        registry.register(driver).await;

        // Probe the hardware
        match registry.probe_driver(&driver_name).await {
            Ok(true) => {
                tracing::info!("OS bridge: auto-discovered device for driver '{driver_name}'");
            }
            Ok(false) => {
                tracing::debug!("OS bridge: driver '{driver_name}' not present yet");
                return;
            }
            Err(e) => {
                tracing::warn!("OS bridge: probe failed for '{driver_name}': {e}");
                return;
            }
        }

        // Connect the device
        let device = match registry.connect(&driver_name).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("OS bridge: connect failed for '{driver_name}': {e}");
                return;
            }
        };

        let device_id = device.id().to_string();

        // Register each capability as a tool and (optionally) as a perception source
        for cap in &device.capabilities {
            let wrapper = DeviceToolWrapper::new(driver_name.clone(), cap.clone());
            tool_registry.register_dynamic(Arc::new(wrapper));

            if let Some(per_reg) = perception_registry {
                per_reg
                    .register_source(Arc::new(DeviceSourceAdapter::new(
                        device_id.clone(),
                        cap.clone(),
                    )))
                    .await;
            }
        }

        // Track the devnode → driver / device mapping
        if let Some(devnode) = &event.devnode {
            devnode_map.insert(devnode.clone(), (driver_name, device_id));
        }

        // First match wins
        return;
    }
}

/// Handle a device Removed event: look up devnode, disconnect, deregister tools
/// and perception sources.
async fn handle_removed(
    registry: &DeviceRegistry,
    tool_registry: &ToolRegistry,
    perception_registry: Option<&PerceptionRegistry>,
    devnode_map: &mut HashMap<String, (String, String)>,
    event: OsDeviceEvent,
) {
    let devnode = match &event.devnode {
        Some(d) => d,
        None => return,
    };

    let (driver_name, device_id) = match devnode_map.remove(devnode) {
        Some(v) => v,
        None => return,
    };

    // Disconnect from registry
    registry.disconnect(&device_id).await.ok();

    // Deregister all tools for this device
    let tool_prefix = format!("device_{}_", driver_name);
    tool_registry.deregister_prefix(&tool_prefix);

    // Deregister perception sources for this device
    let per_prefix = format!("device:{}:", device_id);
    if let Some(per_reg) = perception_registry {
        per_reg.deregister_prefix(&per_prefix).await;
    }

    tracing::info!("OS bridge: auto-disconnected '{device_id}' (removed)");
}

/// Handle a device Changed event: re-probe, reconnect if necessary.
async fn handle_changed(
    registry: &DeviceRegistry,
    matchers: &[MatcherEntry],
    tool_registry: &ToolRegistry,
    perception_registry: Option<&PerceptionRegistry>,
    build_driver: &DriverBuilder,
    devnode_map: &mut HashMap<String, (String, String)>,
    event: OsDeviceEvent,
) {
    // If we don't know this devnode, treat it as a new addition
    if let Some(ref devnode) = event.devnode {
        if !devnode_map.contains_key(devnode) {
            return handle_added(
                registry,
                matchers,
                tool_registry,
                perception_registry,
                build_driver,
                devnode_map,
                event,
            )
            .await;
        }
    }

    // Re-probe known device; disconnect if it has gone away
    if let Some(devnode) = &event.devnode {
        if let Some((driver_name, _device_id)) = devnode_map.get(devnode) {
            match registry.probe_driver(driver_name).await {
                Ok(true) => {
                    tracing::debug!("OS bridge: device '{driver_name}' still present after change");
                }
                _ => {
                    tracing::info!(
                        "OS bridge: device '{driver_name}' gone after change, disconnecting"
                    );
                    handle_removed(
                        registry,
                        tool_registry,
                        perception_registry,
                        devnode_map,
                        event,
                    )
                    .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;

    use super::super::OsDeviceAction;
    use super::super::{DeviceMatcher, MatcherEntry, OsDeviceEvent};
    use super::*;
    use crate::device::mock::MockDeviceDriver;
    use crate::device::{Capability, CapabilityResult};

    /// A driver builder that always returns a MockDeviceDriver.
    fn mock_driver_builder() -> DriverBuilder {
        Arc::new(|kind: &str, _params: serde_json::Value| {
            Ok(Arc::new(MockDeviceDriver::new(kind, true)) as Arc<dyn DeviceDriver>)
        })
    }

    /// Create a dummy OsDeviceEvent for testing.
    fn make_event(action: OsDeviceAction, subsystem: &str, devnode: Option<&str>) -> OsDeviceEvent {
        OsDeviceEvent {
            action,
            subsystem: subsystem.to_string(),
            devnode: devnode.map(|s| s.to_string()),
            properties: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_added_creates_device_and_registers_tools() {
        let registry = Arc::new(DeviceRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let build_driver = mock_driver_builder();

        let matchers = vec![MatcherEntry {
            driver_kind: "mock_dev".into(),
            params: json!({}),
            matcher: DeviceMatcher::Subsystem("tty".into()),
        }];

        let event = make_event(OsDeviceAction::Added, "tty", Some("/dev/ttyUSB0"));

        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        handle_os_event(
            &registry,
            &matchers,
            &tool_registry,
            None,
            &build_driver,
            &mut devnode_map,
            event,
        )
        .await;

        // The driver was built with present:true, so probe during handle_added
        // should have connected the device.
        assert!(!registry.is_empty().await);
        assert_eq!(registry.len().await, 1);
        // devnode should be tracked
        assert!(devnode_map.contains_key("/dev/ttyUSB0"));
    }

    #[tokio::test]
    async fn test_match_first_matcher_wins() {
        let registry = Arc::new(DeviceRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let build_driver = mock_driver_builder();

        let matchers = vec![
            MatcherEntry {
                driver_kind: "mock_first".into(),
                params: json!({}),
                matcher: DeviceMatcher::Subsystem("tty".into()),
            },
            MatcherEntry {
                driver_kind: "mock_second".into(),
                params: json!({}),
                matcher: DeviceMatcher::Subsystem("tty".into()),
            },
        ];

        let event = make_event(OsDeviceAction::Added, "tty", Some("/dev/ttyUSB0"));

        // Only first matcher should match — "mock_first" is the only one tried
        // via handle_os_event.
        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        handle_os_event(
            &registry,
            &matchers,
            &tool_registry,
            None,
            &build_driver,
            &mut devnode_map,
            event,
        )
        .await;

        // Since the mock driver builder creates drivers with default names,
        // probe should work
        assert_eq!(devnode_map.len(), 1);
        assert!(devnode_map.contains_key("/dev/ttyUSB0"));
    }

    #[tokio::test]
    async fn test_removed_disconnects_device() {
        let registry = Arc::new(DeviceRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        // Manually register a device in the devnode map
        devnode_map.insert(
            "/dev/ttyUSB0".to_string(),
            ("mock_device".to_string(), "dev-mock_device".to_string()),
        );

        let event = make_event(OsDeviceAction::Removed, "tty", Some("/dev/ttyUSB0"));

        handle_os_event(
            &registry,
            &[],
            &tool_registry,
            None,
            &mock_driver_builder(),
            &mut devnode_map,
            event,
        )
        .await;

        // Devnode should be removed from map
        assert!(devnode_map.is_empty());
    }

    #[tokio::test]
    async fn test_unknown_devnode_removed_is_noop() {
        let registry = Arc::new(DeviceRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        // Devnode NOT in map
        let event = make_event(OsDeviceAction::Removed, "tty", Some("/dev/unknown"));

        handle_os_event(
            &registry,
            &[],
            &tool_registry,
            None,
            &mock_driver_builder(),
            &mut devnode_map,
            event,
        )
        .await;

        assert!(devnode_map.is_empty());
    }

    #[tokio::test]
    async fn test_changed_unknown_devnode_treated_as_add() {
        let registry = Arc::new(DeviceRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        // Devnode NOT in map — Changed should be treated as Added
        let event = make_event(OsDeviceAction::Changed, "tty", Some("/dev/ttyUSB0"));

        handle_os_event(
            &registry,
            &[], // no matchers — should still run the "add" path but not match
            &tool_registry,
            None,
            &mock_driver_builder(),
            &mut devnode_map,
            event,
        )
        .await;

        // No matchers configured, so nothing should be added
        assert!(devnode_map.is_empty());
    }

    #[tokio::test]
    async fn test_skips_known_devnode() {
        let registry = Arc::new(DeviceRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        devnode_map.insert(
            "/dev/ttyUSB0".to_string(),
            ("existing".to_string(), "dev-existing".to_string()),
        );

        // Added event for known devnode — should be skipped
        let event = make_event(OsDeviceAction::Added, "tty", Some("/dev/ttyUSB0"));

        handle_os_event(
            &registry,
            &[], // would trigger matcher if not skipped
            &tool_registry,
            None,
            &mock_driver_builder(),
            &mut devnode_map,
            event,
        )
        .await;

        // Still exactly 1 entry (no duplicate)
        assert_eq!(devnode_map.len(), 1);
    }

    #[tokio::test]
    async fn test_no_devnode_removed_is_noop() {
        let registry = Arc::new(DeviceRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let mut devnode_map: HashMap<String, (String, String)> = HashMap::new();

        let event = make_event(OsDeviceAction::Removed, "tty", None);

        handle_os_event(
            &registry,
            &[],
            &tool_registry,
            None,
            &mock_driver_builder(),
            &mut devnode_map,
            event,
        )
        .await;

        assert!(devnode_map.is_empty());
    }

    // ── Helper capability for tests ──────────────────────────────────────

    struct DummySensor;

    #[async_trait]
    impl Capability for DummySensor {
        fn name(&self) -> &str {
            "sensor.read_temperature"
        }
        fn param_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _params: serde_json::Value) -> CapabilityResult {
            CapabilityResult {
                success: true,
                output: Some(json!({ "celsius": 23.5 })),
                error: None,
                duration_ms: 2,
            }
        }
    }
}
