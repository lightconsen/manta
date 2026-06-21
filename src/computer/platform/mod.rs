//! Platform tool sets for Syscity
//!
//! This module defines `PlatformToolSet` — a way to group platform-specific
//! tools by environment (Linux Server, macOS Desktop, etc.). Tools are
//! registered individually into `ToolRegistry`; `PlatformToolSet` is only an
//! organizational unit that controls *which* tools are exposed for the current
//! environment.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::tools::Tool;

/// OS control permission scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsControlScope {
    /// Read-only observation (processes, logs, configs).
    ReadOnly = 0,
    /// User-space operations (user processes, user configs).
    UserSpace = 1,
    /// System-level operations (services, packages, system configs).
    System = 2,
    /// Full control (kernel params, firewall, user management).
    Root = 3,
}

/// Platform constraints that determine whether a `PlatformToolSet` is
/// available.
#[derive(Debug, Clone, Default)]
pub struct PlatformConstraints {
    /// Target operating systems (e.g. `["linux"]`, `["macos"]`).
    pub target_os: Vec<String>,
    /// Whether a GUI/display server is required.
    pub requires_gui: bool,
    /// Optional services that must be available (e.g. `["systemd"]`).
    pub requires_services: Vec<String>,
}

impl PlatformConstraints {
    /// Check whether the current environment satisfies all constraints.
    pub fn check(&self) -> bool {
        let current_os = std::env::consts::OS;
        if !self.target_os.is_empty() && !self.target_os.iter().any(|os| os == current_os) {
            return false;
        }

        if self.requires_gui && !has_display_server() {
            return false;
        }

        for svc in &self.requires_services {
            if !is_service_available(svc) {
                return false;
            }
        }

        true
    }
}

/// A set of tools scoped to a specific platform / environment.
pub trait PlatformToolSet: Send + Sync {
    /// Unique identifier, e.g. `"linux-server"`.
    fn id(&self) -> &str;

    /// Human-readable name.
    fn name(&self) -> &str;

    /// Description shown to users / in logs.
    fn description(&self) -> &str;

    /// Platform constraints for runtime availability checks.
    fn constraints(&self) -> &PlatformConstraints;

    /// Maximum permission scope this set requires.
    fn scope(&self) -> OsControlScope {
        OsControlScope::ReadOnly
    }

    /// Tools provided by this set.
    fn tools(&self) -> Vec<Box<dyn Tool>>;

    /// Whether this set is available on the current platform.
    fn is_available(&self) -> bool {
        self.constraints().check()
    }
}

/// Strategy when two sets register tools with the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolConflictStrategy {
    /// Panic on conflict.
    Reject,
    /// Later registration overrides earlier.
    Override,
}

/// Pre-defined platform tool profiles.
#[derive(Debug, Clone)]
pub enum CapabilityProfile {
    /// Only built-in generic tools, no OS-specific sets.
    Minimal,
    /// Read-only observation only.
    Observer,
    /// Server-oriented sets (no GUI required).
    Server,
    /// Desktop-oriented sets (GUI required).
    Desktop,
    /// All available sets.
    Full,
    /// Explicit list of set IDs.
    Custom(Vec<String>),
}

impl CapabilityProfile {
    /// Apply the profile by disabling sets that don't match.
    pub fn apply(&self, registry: &mut PlatformCapabilityRegistry) {
        match self {
            CapabilityProfile::Minimal => {
                let ids: Vec<String> = registry
                    .all_sets()
                    .iter()
                    .map(|s| s.id().to_string())
                    .collect();
                for id in ids {
                    registry.disable(&id);
                }
            }
            CapabilityProfile::Observer => {
                let ids: Vec<String> = registry
                    .all_sets()
                    .iter()
                    .filter(|s| s.scope() != OsControlScope::ReadOnly)
                    .map(|s| s.id().to_string())
                    .collect();
                for id in ids {
                    registry.disable(&id);
                }
            }
            CapabilityProfile::Server => {
                let ids: Vec<String> = registry
                    .all_sets()
                    .iter()
                    .filter(|s| s.constraints().requires_gui)
                    .map(|s| s.id().to_string())
                    .collect();
                for id in ids {
                    registry.disable(&id);
                }
            }
            CapabilityProfile::Desktop => {
                let ids: Vec<String> = registry
                    .all_sets()
                    .iter()
                    .filter(|s| !s.constraints().requires_gui)
                    .map(|s| s.id().to_string())
                    .collect();
                for id in ids {
                    registry.disable(&id);
                }
            }
            CapabilityProfile::Full => {
                // Default — enable everything that passes env checks.
            }
            CapabilityProfile::Custom(ids) => {
                let id_set: HashSet<String> = ids.iter().cloned().collect();
                let to_disable: Vec<String> = registry
                    .all_sets()
                    .iter()
                    .filter(|s| !id_set.contains(s.id()))
                    .map(|s| s.id().to_string())
                    .collect();
                for id in to_disable {
                    registry.disable(&id);
                }
            }
        }
    }
}

/// Detect whether a display server is available.
fn has_display_server() -> bool {
    // Check common display environment variables.
    has_x11() || has_wayland() || cfg!(target_os = "macos") || cfg!(target_os = "windows")
}

/// Detect whether an X11 display server is available.
fn has_x11() -> bool {
    std::env::var("DISPLAY").is_ok() && std::env::var("WAYLAND_DISPLAY").is_err()
}

/// Detect whether a Wayland display server is available.
fn has_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
}

/// Detect whether a system service is available.
fn is_service_available(name: &str) -> bool {
    match name {
        "systemd" => std::path::Path::new("/run/systemd/system").exists(),
        _ => false,
    }
}

pub mod linux;
pub mod linux_desktop_wayland;
pub mod linux_desktop_x11;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod mobile;
pub mod registry;
pub mod server_operator;
pub mod windows;

pub use linux::LinuxToolset;
pub use linux_desktop_wayland::LinuxDesktopWaylandToolset;
pub use linux_desktop_x11::LinuxDesktopX11Toolset;
#[cfg(target_os = "macos")]
pub use macos::MacosToolset;
pub use mobile::{AndroidToolset, IosToolset};
pub use registry::PlatformCapabilityRegistry;
pub use server_operator::{Diagnosis, ServerOperator, SystemInspector, SystemSnapshot};
pub use windows::WindowsToolset;

/// Return all platform tool sets compiled into this binary.
///
/// This is the single source of truth for what *could* be available;
/// runtime detection (`PlatformConstraints::check`) decides what *is*
/// available on the current host.
pub fn all_known_toolsets() -> Vec<Box<dyn PlatformToolSet>> {
    let mut sets: Vec<Box<dyn PlatformToolSet>> = Vec::new();

    #[cfg(target_os = "linux")]
    {
        sets.push(Box::new(LinuxToolset::new()));
        sets.push(Box::new(LinuxDesktopX11Toolset::new()));
        sets.push(Box::new(LinuxDesktopWaylandToolset::new()));
    }

    #[cfg(target_os = "macos")]
    {
        sets.push(Box::new(MacosToolset::new()));
    }

    #[cfg(target_os = "windows")]
    {
        sets.push(Box::new(WindowsToolset::new()));
    }

    // Mobile bridges are platform-agnostic (depend on external CLI tools)
    sets.push(Box::new(AndroidToolset::new()));
    sets.push(Box::new(IosToolset::new()));

    sets
}

/// Build a human-readable summary of the current host environment
/// and which capability sets are available.
pub fn host_environment_summary() -> String {
    use std::fmt::Write;

    let mut buf = String::new();
    let _ = writeln!(&mut buf, "Host: {} ({})", std::env::consts::OS, std::env::consts::ARCH);

    let sets = all_known_toolsets();
    if sets.is_empty() {
        let _ = writeln!(&mut buf, "No OS-specific platform tool sets compiled.");
        return buf;
    }

    let mut available = Vec::new();
    let mut unavailable = Vec::new();

    for set in sets {
        if set.is_available() {
            available.push((set.id().to_string(), set.name().to_string()));
        } else {
            unavailable.push((set.id().to_string(), set.name().to_string()));
        }
    }

    if !available.is_empty() {
        let _ = writeln!(&mut buf, "Available capabilities:");
        for (id, name) in &available {
            let _ = writeln!(&mut buf, "  - {} ({})", name, id);
        }
    }

    if !unavailable.is_empty() {
        let _ = writeln!(&mut buf, "Unavailable capabilities:");
        for (id, name) in &unavailable {
            let _ = writeln!(&mut buf, "  - {} ({}) — environment constraints not met", name, id);
        }
    }

    buf
}
