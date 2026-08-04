use std::process::Command;

fn main() {
    // Embed the git commit hash for version display at runtime
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={}", git_hash);

    // Named cfg aliases for platform families, so `#[cfg]` gates read
    // `desktop_os` / `mobile_os` instead of repeating
    // `any(target_os = "linux", target_os = "macos", target_os = "windows")`
    // at every site (and drifting out of sync).
    println!("cargo::rustc-check-cfg=cfg(desktop_os)");
    println!("cargo::rustc-check-cfg=cfg(mobile_os)");
    match std::env::var("CARGO_CFG_TARGET_OS")
        .unwrap_or_default()
        .as_str()
    {
        "linux" | "macos" | "windows" => println!("cargo:rustc-cfg=desktop_os"),
        "android" | "ios" => println!("cargo:rustc-cfg=mobile_os"),
        _ => {}
    }
}
