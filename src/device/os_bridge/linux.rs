//! Linux OS device monitor — listens to udev events via
//! `NETLINK_KOBJECT_UEVENT`.
//!
//! Uses a raw netlink socket (no external dependencies beyond `libc`) to
//! subscribe to kernel uevent messages.  The wire format is a sequence of
//! null-terminated `KEY=VALUE` pairs prefixed by a small header.
//!
//! # Wire format
//!
//! Each message from the kernel looks like:
//!
//! ```text
//! struct udev_monitor_netlink_header {
//!     prefix: "libudev\0"   // 8 bytes
//!     magic: u32
//!     header_size: u32
//!     properties_size: u32
//!     // padding to 16-byte alignment
//! }
//! action=add\0
//! devpath=/devices/...\0
//! subsystem=usb\0
//! devname=ttyUSB0\0
//! id_vendor_id=2341\0
//! id_model_id=0043\0
//! ...
//! \0
//! ```

// This module performs low-level Linux netlink/socket syscalls that cannot be
// expressed safely in Rust.  Each unsafe block is documented with the
// invariants that the caller must uphold.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::os::unix::io::RawFd;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::{OsDeviceAction, OsDeviceEvent, OsDeviceMonitor};

// ── Netlink constants ────────────────────────────────────────────────────

const NETLINK_KOBJECT_UEVENT: i32 = 15;
const UDEV_MONITOR_MAGIC: u32 = 0xdeadbeef;
const UDEV_HDR_SIZE: usize = 16; // 8 (prefix) + 4 (magic) + 4 (hdr_size) -- padded

/// Receive buffer size for netlink messages (256 KB).
const RCVBUF_SIZE: usize = 256 * 1024;

// ── LinuxUdevMonitor ─────────────────────────────────────────────────────

/// Monitor that subscribes to kernel uevents via a `NETLINK_KOBJECT_UEVENT`
/// socket.
///
/// The reader runs on a `spawn_blocking` task since `recv()` is a blocking
/// syscall. Parsed events are forwarded to the broadcast channel.
pub struct LinuxUdevMonitor {
    tx: broadcast::Sender<OsDeviceEvent>,
    /// JoinHandle for the reader task.
    _reader: JoinHandle<()>,
}

impl LinuxUdevMonitor {
    /// Open a netlink socket and start reading uevents.
    ///
    /// Returns an error if the socket cannot be created (e.g. insufficient
    /// permissions on the host).
    pub fn new() -> std::io::Result<Self> {
        let (tx, _) = broadcast::channel(256);

        // Open netlink socket
        let fd = open_uevent_socket()?;
        let tx_clone = tx.clone();

        let reader = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; RCVBUF_SIZE];
            loop {
                match nix::sys::socket::recv(fd, &mut buf, nix::sys::socket::MsgFlags::empty()) {
                    Ok(n) if n > 0 => {
                        if let Some(event) = parse_uevent(&buf[..n]) {
                            let _ = tx_clone.send(event);
                        }
                    }
                    Ok(_) => {
                        // Empty message — socket may be closing
                        break;
                    }
                    Err(e) => {
                        // EAGAIN / EWOULDBLOCK shouldn't happen since we
                        // set SOCK_NONBLOCK; ENOBUFS is recoverable.
                        if let nix::errno::Errno::ENOBUFS = e {
                            continue;
                        }
                        tracing::warn!("LinuxUdevMonitor recv error: {e}");
                        break;
                    }
                }
            }
        });

        Ok(Self { tx, _reader: reader })
    }
}

impl OsDeviceMonitor for LinuxUdevMonitor {
    fn subscribe(&self) -> broadcast::Receiver<OsDeviceEvent> {
        self.tx.subscribe()
    }
}

// ─── Socket creation ─────────────────────────────────────────────────────

/// Open a `NETLINK_KOBJECT_UEVENT` socket, bind it to the udev multicast
/// group, and set a large receive buffer.
fn open_uevent_socket() -> std::io::Result<RawFd> {
    // SAFETY: libc socket/bind/setsockopt are standard C functions.
    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            NETLINK_KOBJECT_UEVENT,
        );
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Allow non-root processes to receive uevents (requires
        // net.ipv4.ping_group_range)
        let one: i32 = 1;
        let ret = libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>() as libc::socklen_t,
        );
        if ret < 0 {
            let _ = libc::close(fd);
            return Err(std::io::Error::last_os_error());
        }

        // Set large receive buffer
        let buf_size: i32 = RCVBUF_SIZE as i32;
        let ret = libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            &buf_size as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>() as libc::socklen_t,
        );
        if ret < 0 {
            let _ = libc::close(fd);
            return Err(std::io::Error::last_os_error());
        }

        // Bind to udev multicast group (nl_groups = 1)
        let mut addr: libc::sockaddr_nl = std::mem::zeroed();
        addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        addr.nl_groups = 1; // udev monitor group
        addr.nl_pid = 0; // let kernel assign

        let ret = libc::bind(
            fd,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        );
        if ret < 0 {
            let _ = libc::close(fd);
            return Err(std::io::Error::last_os_error());
        }

        Ok(fd)
    }
}

// ─── Uevent parsing ──────────────────────────────────────────────────────

/// Parse a raw uevent buffer into an [`OsDeviceEvent`].
fn parse_uevent(buf: &[u8]) -> Option<OsDeviceEvent> {
    let payload = skip_netlink_header(buf)?;
    let pairs = parse_key_value_pairs(payload)?;

    let action_str = pairs.get("ACTION")?.as_str();
    let action = match action_str {
        "add" => OsDeviceAction::Added,
        "remove" => OsDeviceAction::Removed,
        "change" | "move" => OsDeviceAction::Changed,
        _ => return None,
    };

    let subsystem = pairs.get("SUBSYSTEM").cloned().unwrap_or_default();

    let devname = pairs.get("DEVNAME").or_else(|| pairs.get("DEVICE_NAME"));
    let devnode = devname.map(|n| format!("/dev/{}", n));

    let mut properties: HashMap<String, String> = HashMap::new();
    for (k, v) in pairs {
        match k.as_str() {
            "ACTION" | "SUBSYSTEM" | "DEVNAME" | "DEVPATH" => { /* skip */ }
            _ => {
                properties.insert(k.to_lowercase(), v);
            }
        }
    }

    Some(OsDeviceEvent {
        action,
        subsystem,
        devnode,
        properties,
    })
}

/// Skip the udev netlink header and return the key=value payload.
fn skip_netlink_header(buf: &[u8]) -> Option<&[u8]> {
    if buf.len() < UDEV_HDR_SIZE {
        return None;
    }

    // The header starts with "libudev\0" magic prefix.
    let prefix = &buf[..8];
    let magic = u32::from_ne_bytes(buf[8..12].try_into().ok()?);

    if prefix != b"libudev\0" || magic != UDEV_MONITOR_MAGIC {
        // Non-udev uevent — skip header anyway (some kernels omit the
        // udev prefix but still use the same format).
        return Some(buf);
    }

    let hdr_size = u32::from_ne_bytes(buf[12..16].try_into().ok()?) as usize;
    let start = hdr_size.max(UDEV_HDR_SIZE);
    Some(&buf[start..])
}

/// Parse null-terminated `KEY=VALUE` pairs from a byte slice.
fn parse_key_value_pairs(data: &[u8]) -> Option<HashMap<String, String>> {
    let mut map = HashMap::new();

    // Split on null bytes
    for chunk in data.split(|&b| b == 0) {
        if chunk.is_empty() {
            continue;
        }

        // Find the '=' separator
        let s = std::str::from_utf8(chunk).ok()?;
        if let Some(eq_pos) = s.find('=') {
            let key = s[..eq_pos].to_uppercase();
            let value = s[eq_pos + 1..].to_string();
            map.insert(key, value);
        }
    }

    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

/// Factory function.
/// Factory function (matches the signature used by platform module).
pub fn create_os_monitor() -> Box<dyn OsDeviceMonitor> {
    match LinuxUdevMonitor::new() {
        Ok(m) => Box::new(m),
        Err(e) => {
            tracing::warn!("Failed to open Linux udev monitor: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uevent_add() {
        // Simulate a minimal uevent for a USB serial adapter.
        let mut buf = Vec::new();
        // Header
        buf.extend_from_slice(b"libudev\0");
        buf.extend_from_slice(&UDEV_MONITOR_MAGIC.to_ne_bytes());
        buf.extend_from_slice(&16u32.to_ne_bytes()); // header_size
                                                     // Padding (UDEV_HDR_SIZE = 16, already there)
                                                     // Payload
        buf.extend_from_slice(b"ACTION=add\0");
        buf.extend_from_slice(b"SUBSYSTEM=tty\0");
        buf.extend_from_slice(b"DEVNAME=ttyUSB0\0");
        buf.extend_from_slice(b"ID_VENDOR_ID=2341\0");
        buf.extend_from_slice(b"ID_MODEL_ID=0043\0");
        buf.push(0); // terminator

        let event = parse_uevent(&buf).expect("should parse");
        assert_eq!(event.action, OsDeviceAction::Added);
        assert_eq!(event.subsystem, "tty");
        assert_eq!(event.devnode, Some("/dev/ttyUSB0".into()));
        assert_eq!(event.get("id_vendor_id"), Some("2341"));
        assert_eq!(event.get("id_model_id"), Some("0043"));
    }

    #[test]
    fn test_parse_uevent_remove() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"libudev\0");
        buf.extend_from_slice(&UDEV_MONITOR_MAGIC.to_ne_bytes());
        buf.extend_from_slice(&16u32.to_ne_bytes());
        buf.extend_from_slice(b"ACTION=remove\0");
        buf.extend_from_slice(b"SUBSYSTEM=usb\0");
        buf.extend_from_slice(b"DEVNAME=bus/usb/001/002\0");
        buf.push(0);

        let event = parse_uevent(&buf).expect("should parse");
        assert_eq!(event.action, OsDeviceAction::Removed);
        assert_eq!(event.subsystem, "usb");
    }

    #[test]
    fn test_parse_uevent_includes_driver() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"libudev\0");
        buf.extend_from_slice(&UDEV_MONITOR_MAGIC.to_ne_bytes());
        buf.extend_from_slice(&16u32.to_ne_bytes());
        buf.extend_from_slice(b"ACTION=add\0");
        buf.extend_from_slice(b"SUBSYSTEM=tty\0");
        buf.extend_from_slice(b"DEVNAME=ttyUSB0\0");
        buf.extend_from_slice(b"DRIVER=ftdi_sio\0");
        buf.extend_from_slice(b"ID_VENDOR_ID=2341\0");
        buf.extend_from_slice(b"ID_MODEL_ID=0043\0");
        buf.push(0);

        let event = parse_uevent(&buf).expect("should parse");
        assert_eq!(event.action, OsDeviceAction::Added);
        assert_eq!(event.subsystem, "tty");
        assert_eq!(event.devnode, Some("/dev/ttyUSB0".into()));
        // DRIVER field is stored as lowercased "driver" in properties
        assert_eq!(event.get("driver"), Some("ftdi_sio"));
        // ID_DRIVER is not present in this event
        assert_eq!(event.get("id_driver"), None);
    }

    #[test]
    fn test_parse_uevent_with_id_driver() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"libudev\0");
        buf.extend_from_slice(&UDEV_MONITOR_MAGIC.to_ne_bytes());
        buf.extend_from_slice(&16u32.to_ne_bytes());
        buf.extend_from_slice(b"ACTION=add\0");
        buf.extend_from_slice(b"SUBSYSTEM=usb\0");
        buf.extend_from_slice(b"DEVNAME=bus/usb/001/003\0");
        buf.extend_from_slice(b"ID_DRIVER=usbhid\0");
        buf.push(0);

        let event = parse_uevent(&buf).expect("should parse");
        assert_eq!(event.get("id_driver"), Some("usbhid"));
        // DRIVER is not present in this event
        assert_eq!(event.get("driver"), None);
    }

    #[test]
    fn test_parse_uevent_skips_unknown_action() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"libudev\0");
        buf.extend_from_slice(&UDEV_MONITOR_MAGIC.to_ne_bytes());
        buf.extend_from_slice(&16u32.to_ne_bytes());
        buf.extend_from_slice(b"ACTION=bind\0");
        buf.extend_from_slice(b"SUBSYSTEM=usb\0");
        buf.push(0);

        assert!(parse_uevent(&buf).is_none(), "bind is not an action we track");
    }
}
