//! Plugin logging
//!
//! Write log messages to Syscity's logger.

/// Log an informational message
pub fn info(message: &str) {
    let ptr = message.as_ptr();
    let len = message.len();
    unsafe {
        super::log(ptr, len);
    }
}

/// Log a warning message
pub fn warn(message: &str) {
    let prefixed = format!("[WARN] {}", message);
    info(&prefixed);
}

/// Log an error message
pub fn error(message: &str) {
    let prefixed = format!("[ERROR] {}", message);
    info(&prefixed);
}

/// Log a debug message
pub fn debug(message: &str) {
    let prefixed = format!("[DEBUG] {}", message);
    info(&prefixed);
}
