//! Mobile device bridge — control Android and iOS devices from Syscity.
//!
//! This module provides `PlatformToolSet`s for mobile automation via:
//! - **Android**: ADB (Android Debug Bridge)
//! - **iOS**: libimobiledevice (idevice*) tools
//!
//! Both sets are platform-agnostic — they can run on any host OS as long as
//! the corresponding CLI tools are installed.


pub mod android;
pub mod ios;

pub use android::AndroidToolset;
pub use ios::IosToolset;

/// Check whether a command is available on PATH.
fn has_command(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check whether `adb` is available on PATH.
pub fn has_adb() -> bool {
    has_command("adb")
}

/// Check whether `idevice_id` (libimobiledevice) is available on PATH.
pub fn has_idevice() -> bool {
    has_command("idevice_id")
}

/// Shared helper: run a command and return (success, stdout, stderr).
pub async fn run_cmd(
    cmd: &str,
    args: &[&str],
) -> std::io::Result<(std::process::ExitStatus, String, String)> {
    let output = tokio::process::Command::new(cmd).args(args).output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status, stdout, stderr))
}
