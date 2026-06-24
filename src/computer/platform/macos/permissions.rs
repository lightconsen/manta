//! macOS accessibility permissions — detection and user guidance.

use std::process::Command;

/// Check whether the current process has macOS Accessibility permissions.
///
/// Runs a minimal AppleScript that requires accessibility access.
/// Returns `true` if the script succeeds, `false` if it fails with
/// the assistive-access error.
pub fn has_accessibility_permission() -> bool {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events" to return name of first application process whose frontmost is true"#)
        .output();

    match output {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            !stderr.contains("assistive access")
                && !stderr.contains("-25211")
                && !stderr.contains("-1719")
        }
        Err(_) => false,
    }
}

/// Trigger the macOS accessibility permission dialog.
///
/// Runs a short AppleScript that requires System Events access.
/// On the first call (or after a `tccutil reset`), macOS shows the
/// standard security dialog asking the user to allow the calling app.
/// The dialog is non-blocking for osascript — the script fails
/// immediately with a permission error, but the dialog stays visible.
pub fn trigger_accessibility_prompt() {
    let _ = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events" to return name of first application process whose frontmost is true"#)
        .output();
}

/// Open System Settings to the Accessibility pane (best-effort).
pub fn open_accessibility_settings() {
    // macOS Ventura+ uses "System Settings", older versions use "System
    // Preferences". Try the modern name first.
    let modern = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Settings" to activate"#)
        .output();
    if !matches!(&modern, Ok(out) if out.status.success()) {
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(r#"tell application "System Preferences" to activate"#)
            .output();
    }
}

/// Human-readable instructions for granting Accessibility permissions.
pub fn accessibility_permission_guide() -> String {
    let app_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "Syscity".to_string());

    format!(
        "macOS Accessibility permission is required for desktop control tools (screenshot, UI \
         inspection, clicking, typing).\n\nTo grant permission:\n1. Open System Settings → \
         Privacy & Security → Accessibility\n2. Click the '+' button\n3. Select the terminal app \
         running {} (e.g. Terminal.app, iTerm2.app, or VS Code)\n4. Toggle the switch ON\n\nThen \
         restart {}.",
        app_name, app_name
    )
}
