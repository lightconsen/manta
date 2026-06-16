//! macOS OS device monitor — watches `/dev/` for file-system events.
//!
//! Uses the `notify` crate (already a dependency) to detect device node
//! creation and removal. Device classification is based on the node name
//! prefix:
//!
//! | Prefix    | Subsystem |
//! |-----------|-----------|
//! | `tty.`    | tty       |
//! | `cu.`     | tty       |
//! | `disk`    | disk      |
//! | `rdisk`   | disk      |
//! | `video`   | video     |
//!
//! This is a pragmatic approach that avoids IOKit C FFI while covering
//! the most common hot-plug scenarios (USB serial adapters, disk mounts).
//! The polling-based hot-plug loop remains as a fallback for short-lived
//! devices that FSEvents may batch together.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;

use super::{OsDeviceAction, OsDeviceEvent, OsDeviceMonitor};

/// Monitor that watches `/dev/` for device node changes on macOS.
pub struct MacOsDevMonitor {
    tx: broadcast::Sender<OsDeviceEvent>,
    /// Kept alive for the lifetime of the monitor.
    _watcher: Arc<RecommendedWatcher>,
}

impl MacOsDevMonitor {
    /// Create a new macOS device monitor.
    ///
    /// Spawns a background `notify` watcher on `/dev/` and bridges its
    /// events into the broadcast channel.
    pub fn new() -> Result<Self, notify::Error> {
        let (tx, _) = broadcast::channel(256);
        let tx_clone = tx.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    if let Some(dev_event) = map_notify_event(&event) {
                        let _ = tx_clone.send(dev_event);
                    }
                }
            },
            Config::default(),
        )?;

        watcher.watch(Path::new("/dev"), RecursiveMode::NonRecursive)?;

        Ok(Self {
            tx,
            _watcher: Arc::new(watcher),
        })
    }
}

impl OsDeviceMonitor for MacOsDevMonitor {
    fn subscribe(&self) -> broadcast::Receiver<OsDeviceEvent> {
        self.tx.subscribe()
    }
}

/// Map a `notify::Event` to an `OsDeviceEvent`, or return `None` if the
/// event is not device-related.
fn map_notify_event(event: &notify::Event) -> Option<OsDeviceEvent> {
    let path = event.paths.first()?;
    let filename = path.file_name()?.to_str()?;

    // Only care about device node creation and removal
    let action = match event.kind {
        EventKind::Create(_) => OsDeviceAction::Added,
        EventKind::Remove(_) => OsDeviceAction::Removed,
        _ => return None,
    };

    // Classify by filename prefix
    let subsystem = classify_devnode(filename);

    Some(OsDeviceEvent {
        action,
        subsystem: subsystem.to_string(),
        devnode: Some(path.to_string_lossy().into_owned()),
        properties: HashMap::new(),
    })
}

/// Classify a macOS /dev/ entry by its filename prefix.
fn classify_devnode(filename: &str) -> &'static str {
    if filename.starts_with("tty.") || filename.starts_with("cu.") {
        "tty"
    } else if filename.starts_with("disk") || filename.starts_with("rdisk") {
        "disk"
    } else if filename.starts_with("video") {
        "video"
    } else {
        "unknown"
    }
}

/// Factory function (matches the signature used by platform module).
pub fn create_os_monitor() -> Box<dyn OsDeviceMonitor> {
    match MacOsDevMonitor::new() {
        Ok(m) => Box::new(m),
        Err(e) => {
            tracing::warn!("Failed to create macOS device monitor: {e}");
            // Return a no-op monitor (closed broadcast channel)
            let (tx, _) = broadcast::channel(1);
            struct Fallback(broadcast::Sender<OsDeviceEvent>);
            impl OsDeviceMonitor for Fallback {
                fn subscribe(&self) -> broadcast::Receiver<OsDeviceEvent> {
                    self.0.subscribe()
                }
            }
            Box::new(Fallback(tx))
        }
    }
}
