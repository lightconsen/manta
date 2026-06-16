//! OS Device Bridge — subscribes to host OS device plug/unplug events and
//! auto-discovers Syscity devices.
//!
//! # Architecture
//!
//! ```text
//! OS (udev / IOKit / /dev/ notify)
//!    │  OsDeviceEvent
//!    ▼
//! OsDeviceMonitor (trait)
//!    │
//!    ├── Added   → match DeviceMatcher → build driver → probe → connect
//!    ├── Removed → disconnect by devnode
//!    └── Changed → re-probe
//! ```
//!
//! Each platform provides its own `OsDeviceMonitor` implementation:
//!
//! | Platform | Implementation | Mechanism |
//! |----------|---------------|-----------|
//! | Linux    | `LinuxUdevMonitor` | `NETLINK_KOBJECT_UEVENT` socket |
//! | macOS    | `MacOsDevMonitor`  | `notify` on `/dev/` |
//! | Other    | `NoopOsMonitor`    | closed channel (no-op) |

pub mod bridge;

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), path = "noop.rs")]
mod platform;

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub use platform::create_os_monitor;

// ── Core types ───────────────────────────────────────────────────────────

/// What happened to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OsDeviceAction {
    /// Device was added / plugged in.
    Added,
    /// Device was removed / unplugged.
    Removed,
    /// Device properties changed (e.g. a USB device transitioned states).
    Changed,
}

/// A device event received from the host operating system.
#[derive(Debug, Clone)]
pub struct OsDeviceEvent {
    /// What happened.
    pub action: OsDeviceAction,
    /// Kernel subsystem, e.g. `"usb"`, `"tty"`, `"hid"`.
    pub subsystem: String,
    /// Device node path, e.g. `"/dev/ttyUSB0"`.
    pub devnode: Option<String>,
    /// Arbitrary key-value properties (VID, PID, serial, interface, etc.).
    pub properties: HashMap<String, String>,
}

impl OsDeviceEvent {
    /// Convenience: get a property value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(|s| s.as_str())
    }
}

// ── Monitor trait ────────────────────────────────────────────────────────

/// Trait for subscribing to OS-level device events.
#[async_trait]
pub trait OsDeviceMonitor: Send + Sync {
    /// Subscribe to OS device events.
    ///
    /// Returns a `broadcast::Receiver` that yields [`OsDeviceEvent`]s.
    fn subscribe(&self) -> broadcast::Receiver<OsDeviceEvent>;
}

// ── Device matcher ───────────────────────────────────────────────────────

/// Describes how to match an OS device event to a driver kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DeviceMatcher {
    /// Match a USB device by vendor and product ID (hex strings).
    UsbDevice {
        /// USB vendor ID, e.g. `"2341"`.
        vid: String,
        /// USB product ID, e.g. `"0043"`.
        pid: String,
    },
    /// Match by kernel subsystem name, e.g. `"tty"`, `"hid"`, `"usb"`.
    Subsystem(String),
    /// Match by devnode glob pattern, e.g. `"/dev/ttyUSB*"`.
    DevPattern(String),
}

impl DeviceMatcher {
    /// Returns `true` if this matcher matches the given OS device event.
    pub fn matches(&self, event: &OsDeviceEvent) -> bool {
        match self {
            DeviceMatcher::UsbDevice { vid, pid } => {
                let ev_vid = event.get("ID_VENDOR_ID").or_else(|| event.get("VID"));
                let ev_pid = event.get("ID_MODEL_ID").or_else(|| event.get("PID"));
                ev_vid == Some(vid.as_str()) && ev_pid == Some(pid.as_str())
            }
            DeviceMatcher::Subsystem(name) => event.subsystem == *name,
            DeviceMatcher::DevPattern(pattern) => {
                if let Some(ref devnode) = event.devnode {
                    glob::Pattern::new(pattern)
                        .map(|p| p.matches(devnode))
                        .unwrap_or(false)
                } else {
                    false
                }
            }
        }
    }
}

// ── Configuration ────────────────────────────────────────────────────────

/// A single matcher entry in the configuration file.
///
/// When an OS device event matches the `matcher` predicate, a driver of
/// `driver_kind` is constructed (with `params`) and probed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatcherEntry {
    /// The device driver kind to instantiate (e.g. `"mock"`).
    pub driver_kind: String,
    /// JSON parameters passed to the driver constructor.
    #[serde(default)]
    pub params: serde_json::Value,
    /// The matcher predicate.
    pub matcher: DeviceMatcher,
}

/// Configuration for the OS device bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsBridgeConfig {
    /// Master on/off switch.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Matcher entries that map OS device events to driver kinds.
    #[serde(default)]
    pub matchers: Vec<MatcherEntry>,
}

const fn default_enabled() -> bool {
    true
}

impl Default for OsBridgeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            matchers: Vec::new(),
        }
    }
}
