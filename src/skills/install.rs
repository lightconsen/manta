//! Installation handler for skills
//!
//! Handles installation of dependencies using various package managers
//! and installation methods (brew, npm, go, uv, download, etc.)

use crate::skills::frontmatter::InstallSpec;
use std::process::Stdio;
use tracing::{error, info};

/// Installation result
#[derive(Debug, Clone)]
pub enum InstallResult {
    /// Already installed
    AlreadyPresent { spec: InstallSpec },
    /// Successfully installed
    Installed { spec: InstallSpec },
    /// Installation failed
    Failed { spec: InstallSpec, error: String },
    /// Skipped (not supported on this platform)
    Skipped { spec: InstallSpec, reason: String },
}

/// Install a skill dependency using the appropriate method
pub async fn install_skill(spec: &InstallSpec) -> crate::Result<InstallResult> {
    let result = install_binary_internal(spec).await;

    match result {
        InstallResultInternal::AlreadyPresent => {
            Ok(InstallResult::AlreadyPresent { spec: spec.clone() })
        }
        InstallResultInternal::Installed => Ok(InstallResult::Installed { spec: spec.clone() }),
        InstallResultInternal::Failed(e) => {
            Ok(InstallResult::Failed { spec: spec.clone(), error: e })
        }
        InstallResultInternal::Skipped(r) => {
            Ok(InstallResult::Skipped { spec: spec.clone(), reason: r })
        }
    }
}

/// Install a binary using the appropriate method (public API for direct use)
pub async fn install_binary(spec: &InstallSpec) -> InstallResult {
    match install_skill(spec).await {
        Ok(result) => result,
        Err(e) => InstallResult::Failed {
            spec: spec.clone(),
            error: e.to_string(),
        },
    }
}

/// Internal installation result for easier handling
enum InstallResultInternal {
    AlreadyPresent,
    Installed,
    Failed(String),
    Skipped(String),
}

/// Internal implementation
async fn install_binary_internal(spec: &InstallSpec) -> InstallResultInternal {
    match spec {
        InstallSpec::Brew { package, tap, binary } => {
            install_with_brew(package, tap.as_deref(), binary.as_deref()).await
        }
        InstallSpec::Npm { package, global, binary } => {
            install_with_npm(package, *global, binary.as_deref()).await
        }
        InstallSpec::Go { package, binary } => install_with_go(package, binary.as_deref()).await,
        InstallSpec::Uv { package, binary } => install_with_uv(package, binary.as_deref()).await,
        InstallSpec::Download { binary, from, extract } => {
            install_by_download(binary, from, extract.as_deref()).await
        }
        InstallSpec::Shell { command, binary } => {
            install_with_shell(command, binary.as_deref()).await
        }
        InstallSpec::Cargo { package, binary } => {
            install_with_cargo(package, binary.as_deref()).await
        }
    }
}

/// Check if a binary is already available in PATH
pub async fn is_binary_available(name: &str) -> bool {
    match tokio::process::Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

/// Install using Homebrew (macOS/Linux)
async fn install_with_brew(
    package: &str,
    tap: Option<&str>,
    binary: Option<&str>,
) -> InstallResultInternal {
    // Check if already installed
    let bin_name = binary.unwrap_or(package);
    if is_binary_available(bin_name).await {
        return InstallResultInternal::AlreadyPresent;
    }

    // Check if brew is available
    if !is_binary_available("brew").await {
        return InstallResultInternal::Skipped("Homebrew not available".to_string());
    }

    // Add tap if specified
    if let Some(tap_name) = tap {
        info!("Adding tap: {}", tap_name);
        let tap_result = tokio::process::Command::new("brew")
            .args(["tap", tap_name])
            .output()
            .await;

        if let Err(e) = tap_result {
            return InstallResultInternal::Failed(format!("Failed to add tap: {}", e));
        }
    }

    // Install package
    info!("Installing {} via Homebrew", package);
    match tokio::process::Command::new("brew")
        .args(["install", package])
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                info!("Successfully installed {} via Homebrew", package);
                InstallResultInternal::Installed
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error!("Homebrew install failed: {}", stderr);
                InstallResultInternal::Failed(format!("Homebrew error: {}", stderr))
            }
        }
        Err(e) => {
            error!("Failed to run brew: {}", e);
            InstallResultInternal::Failed(format!("Failed to run brew: {}", e))
        }
    }
}

/// Install using npm
async fn install_with_npm(
    package: &str,
    global: bool,
    binary: Option<&str>,
) -> InstallResultInternal {
    // Check if already installed
    let bin_name = binary.unwrap_or_else(|| {
        package
            .trim_start_matches('@')
            .split('/')
            .last()
            .unwrap_or(package)
    });
    if is_binary_available(bin_name).await {
        return InstallResultInternal::AlreadyPresent;
    }

    // Determine npm command
    let npm_cmd = if is_binary_available("pnpm").await {
        "pnpm"
    } else if is_binary_available("yarn").await {
        "yarn"
    } else {
        "npm"
    };

    info!("Installing {} via {}", package, npm_cmd);

    let mut args = Vec::new();
    if npm_cmd == "yarn" {
        if global {
            args.push("global");
        }
        args.push("add");
    } else {
        if global {
            args.push("-g");
        }
        args.push("install");
    }
    args.push(package);

    match tokio::process::Command::new(npm_cmd)
        .args(&args)
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                info!("Successfully installed {} via {}", package, npm_cmd);
                InstallResultInternal::Installed
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error!("{} install failed: {}", npm_cmd, stderr);
                InstallResultInternal::Failed(format!("{} error: {}", npm_cmd, stderr))
            }
        }
        Err(e) => {
            error!("Failed to run {}: {}", npm_cmd, e);
            InstallResultInternal::Failed(format!("Failed to run {}: {}", npm_cmd, e))
        }
    }
}

/// Install using Go
async fn install_with_go(package: &str, binary: Option<&str>) -> InstallResultInternal {
    // Check if already installed
    let bin_name = binary.unwrap_or_else(|| package.split('/').last().unwrap_or(package));
    if is_binary_available(bin_name).await {
        return InstallResultInternal::AlreadyPresent;
    }

    // Check if go is available
    if !is_binary_available("go").await {
        return InstallResultInternal::Skipped("Go not available".to_string());
    }

    info!("Installing {} via Go", package);

    // Set GOBIN if not set to ensure binary goes to a discoverable location
    let gobin = std::env::var("GOBIN")
        .or_else(|_| std::env::var("GOPATH").map(|p| format!("{}/bin", p)))
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .map(|h| format!("{}/go/bin", h.display()))
                .unwrap_or_default()
        });

    let output = tokio::process::Command::new("go")
        .env("GOBIN", &gobin)
        .args(["install", &format!("{}@latest", package)])
        .output()
        .await;

    match output {
        Ok(result) => {
            if result.status.success() {
                info!("Successfully installed {} via Go", package);
                // Try to add to PATH if not already there
                add_to_path_if_needed(&gobin).await;
                InstallResultInternal::Installed
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr);
                error!("Go install failed: {}", stderr);
                InstallResultInternal::Failed(format!("Go error: {}", stderr))
            }
        }
        Err(e) => {
            error!("Failed to run go: {}", e);
            InstallResultInternal::Failed(format!("Failed to run go: {}", e))
        }
    }
}

/// Install using uv (Python)
async fn install_with_uv(package: &str, binary: Option<&str>) -> InstallResultInternal {
    // Check if already installed
    let bin_name = binary.unwrap_or(package);
    if is_binary_available(bin_name).await {
        return InstallResultInternal::AlreadyPresent;
    }

    // Check if uv is available
    if !is_binary_available("uv").await {
        return InstallResultInternal::Skipped("uv not available".to_string());
    }

    info!("Installing {} via uv", package);

    match tokio::process::Command::new("uv")
        .args(["tool", "install", "--force", package])
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                info!("Successfully installed {} via uv", package);
                InstallResultInternal::Installed
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error!("uv install failed: {}", stderr);
                InstallResultInternal::Failed(format!("uv error: {}", stderr))
            }
        }
        Err(e) => {
            error!("Failed to run uv: {}", e);
            InstallResultInternal::Failed(format!("Failed to run uv: {}", e))
        }
    }
}

/// Install by downloading from URL
async fn install_by_download(
    binary: &str,
    url: &str,
    extract: Option<&str>,
) -> InstallResultInternal {
    // Check if already installed
    if is_binary_available(binary).await {
        return InstallResultInternal::AlreadyPresent;
    }

    info!("Downloading {} from {}", binary, url);

    // Determine install directory
    let install_dir = dirs::home_dir()
        .map(|h| h.join(".local").join("bin"))
        .or_else(|| std::env::current_dir().ok().map(|d| d.join("bin")))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

    // Create directory if needed
    if let Err(e) = tokio::fs::create_dir_all(&install_dir).await {
        return InstallResultInternal::Failed(format!("Failed to create install directory: {}", e));
    }

    let temp_file = std::env::temp_dir().join(format!("manta-download-{}", binary));

    // Download file using curl
    let download_result = tokio::process::Command::new("curl")
        .args([
            "-fsSL",
            "-o",
            temp_file.to_str().unwrap_or("/tmp/download"),
            url,
        ])
        .output()
        .await;

    match download_result {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return InstallResultInternal::Failed(format!("Download failed: {}", stderr));
            }
        }
        Err(e) => {
            return InstallResultInternal::Failed(format!("Failed to run curl: {}", e));
        }
    }

    // Handle extraction if needed
    let final_path = if let Some(archive_type) = extract {
        match extract_archive(&temp_file, &install_dir, archive_type).await {
            Ok(path) => path,
            Err(e) => return InstallResultInternal::Failed(format!("Extraction failed: {}", e)),
        }
    } else {
        let dest = install_dir.join(binary);
        if let Err(e) = tokio::fs::copy(&temp_file, &dest).await {
            return InstallResultInternal::Failed(format!("Failed to copy binary: {}", e));
        }
        dest
    };

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = tokio::fs::metadata(&final_path).await {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = tokio::fs::set_permissions(&final_path, perms).await;
        }
    }

    // Clean up temp file
    let _ = tokio::fs::remove_file(&temp_file).await;

    info!("Successfully installed {} to {:?}", binary, final_path);
    InstallResultInternal::Installed
}

/// Extract an archive file
async fn extract_archive(
    archive: &std::path::Path,
    dest_dir: &std::path::Path,
    archive_type: &str,
) -> crate::Result<std::path::PathBuf> {
    match archive_type {
        "tar.gz" | "tgz" => {
            let output = tokio::process::Command::new("tar")
                .args([
                    "-xzf",
                    archive.to_str().unwrap_or(""),
                    "-C",
                    dest_dir.to_str().unwrap_or(""),
                ])
                .output()
                .await
                .map_err(|e| crate::error::MantaError::Io(e))?;

            if !output.status.success() {
                return Err(crate::error::MantaError::Internal(
                    "Failed to extract tar.gz".to_string(),
                ));
            }

            // Return the destination directory as the result
            Ok(dest_dir.to_path_buf())
        }
        "zip" => {
            let output = tokio::process::Command::new("unzip")
                .args([
                    "-o",
                    archive.to_str().unwrap_or(""),
                    "-d",
                    dest_dir.to_str().unwrap_or(""),
                ])
                .output()
                .await
                .map_err(|e| crate::error::MantaError::Io(e))?;

            if !output.status.success() {
                return Err(crate::error::MantaError::Internal(
                    "Failed to extract zip".to_string(),
                ));
            }

            Ok(dest_dir.to_path_buf())
        }
        _ => Err(crate::error::MantaError::Internal(format!(
            "Unsupported archive type: {}",
            archive_type
        ))),
    }
}

/// Install using shell command
async fn install_with_shell(command: &str, binary: Option<&str>) -> InstallResultInternal {
    // Check if already installed
    if let Some(bin) = binary {
        if is_binary_available(bin).await {
            return InstallResultInternal::AlreadyPresent;
        }
    }

    info!("Running install command: {}", command);

    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                info!("Install command completed successfully");
                InstallResultInternal::Installed
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error!("Install command failed: {}", stderr);
                InstallResultInternal::Failed(format!("Shell error: {}", stderr))
            }
        }
        Err(e) => {
            error!("Failed to run shell: {}", e);
            InstallResultInternal::Failed(format!("Failed to run shell: {}", e))
        }
    }
}

/// Install using Cargo
async fn install_with_cargo(package: &str, binary: Option<&str>) -> InstallResultInternal {
    // Check if already installed
    let bin_name = binary.unwrap_or_else(|| package.split('/').last().unwrap_or(package));
    if is_binary_available(bin_name).await {
        return InstallResultInternal::AlreadyPresent;
    }

    // Check if cargo is available
    if !is_binary_available("cargo").await {
        return InstallResultInternal::Skipped("Cargo not available".to_string());
    }

    info!("Installing {} via Cargo", package);

    match tokio::process::Command::new("cargo")
        .args(["install", package])
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                info!("Successfully installed {} via Cargo", package);
                InstallResultInternal::Installed
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error!("Cargo install failed: {}", stderr);
                InstallResultInternal::Failed(format!("Cargo error: {}", stderr))
            }
        }
        Err(e) => {
            error!("Failed to run cargo: {}", e);
            InstallResultInternal::Failed(format!("Failed to run cargo: {}", e))
        }
    }
}

/// Try to add a directory to PATH in shell config
async fn add_to_path_if_needed(dir: &str) {
    // This is a best-effort operation
    let shell = std::env::var("SHELL").unwrap_or_default();
    let config_file = if shell.contains("zsh") {
        dirs::home_dir().map(|h| h.join(".zshrc"))
    } else if shell.contains("bash") {
        dirs::home_dir().map(|h| h.join(".bashrc"))
    } else {
        None
    };

    if let Some(config) = config_file {
        let path_line = format!("export PATH=\"{}:$PATH\"", dir);

        // Check if already in config
        if let Ok(content) = tokio::fs::read_to_string(&config).await {
            if content.contains(&path_line) {
                return; // Already present
            }
        }

        // Append to config
        let _ = tokio::fs::write(&config, format!("\n# Added by Manta\n{}\n", path_line)).await;

        info!("Added {} to PATH in {:?}", dir, config);
    }
}

/// Install all specs for a skill, returning results for each
pub async fn install_all(specs: &[InstallSpec]) -> Vec<(InstallSpec, InstallResult)> {
    let mut results = Vec::new();

    for spec in specs {
        let result = install_binary(spec).await;
        results.push((spec.clone(), result));
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_binary_available_true() {
        // "sh" should always be available on Unix
        #[cfg(unix)]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(is_binary_available("sh"));
            assert!(result);
        }
    }

    #[test]
    fn test_is_binary_available_false() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(is_binary_available("this_binary_definitely_does_not_exist_12345"));
        assert!(!result);
    }

    #[test]
    fn test_install_result_display() {
        use crate::skills::frontmatter::InstallSpec;

        let spec = InstallSpec::Shell {
            command: "test".to_string(),
            binary: None,
        };

        let result = InstallResult::Installed { spec: spec.clone() };
        assert!(matches!(result, InstallResult::Installed { .. }));

        let result = InstallResult::Failed {
            spec: spec.clone(),
            error: "test error".to_string(),
        };
        assert!(matches!(result, InstallResult::Failed { .. }));
    }
}
