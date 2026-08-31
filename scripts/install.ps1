# Syscity One-Line Installer (Windows)
# Usage: irm https://syscity.net/install.ps1 | iex

$ErrorActionPreference = "Stop"

$Repo = "lightconsen/syscity"
$InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\syscity" }
$Binary = "syscity.exe"

# Detect architecture
$Arch = if ([System.Environment]::Is64BitProcessor) { "amd64" } else { "x86" }
if ($Arch -eq "x86") {
    Write-Host "Unsupported architecture: x86 (32-bit)" -ForegroundColor Red
    exit 1
}
$Target = "windows-amd64"

# Find latest release version
$Latest = (Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "syscity-install" }).tag_name
if (-not $Latest) {
    $Latest = "v0.1.0"
}

Write-Host "Installing Syscity $Latest for $Target..."

# Download
Write-Host "Downloading..."
$TmpDir = New-Item -ItemType Directory -Path ([System.IO.Path]::GetTempPath() + [System.Guid]::NewGuid().ToString()) | Select-Object -ExpandProperty FullName
$DownloadUrl = "https://github.com/$Repo/releases/download/$Latest/syscity-desktop-windows-amd64.exe"
Invoke-WebRequest -Uri $DownloadUrl -OutFile "$TmpDir\syscity.exe"

# Install
Write-Host "Installing to $InstallDir..."
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
Copy-Item "$TmpDir\syscity.exe" "$InstallDir\$Binary"

# Add to PATH (user-level, persistent)
$UserPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [System.Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Added $InstallDir to your PATH."
}

# Create config directory
$ConfigDir = "$env:USERPROFILE\.syscity"
New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null

# Cleanup
Remove-Item -Recurse -Force $TmpDir

Write-Host ""
Write-Host "Syscity installed successfully!"
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Configure:   syscity setup"
Write-Host "  2. Start:       syscity start"
Write-Host "  3. Open Web UI: http://127.0.0.1:18080"
Write-Host ""
Write-Host "For more options: syscity --help"
Write-Host ""
Write-Host "Note: You may need to restart your terminal for PATH changes to take effect."
