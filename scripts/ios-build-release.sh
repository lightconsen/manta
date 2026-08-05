#!/usr/bin/env bash
set -euo pipefail

# Syscity iOS — Tauri release build for the Simulator.
# Builds the iOS app in release mode (aarch64 simulator slice), then installs
# and launches it on a booted Simulator.
#
# Usage:
#   ./ios-build-release.sh                      Build release, install + launch
#   ./ios-build-release.sh --skip-launch        Build only (no install/launch)
#   ./ios-build-release.sh --device "iPhone 16" Install/launch on a named device
#   ./ios-build-release.sh --device <UDID>
#
# Notes:
#   - Must be run from the repo root or anywhere on disk (the script cd's).
#   - Simulator builds need no signing cert (`--no-sign`).
#   - `cargo tauri ios build` regenerates the Xcode project from
#     gen/apple/project.yml and runs the web frontend build (beforeBuildCommand).
#   - Stale gen/apple/build artifacts are removed first: a repeat build fails
#     with "failed to rename ... Directory not empty (os error 66)" otherwise.
#   - Requires the `aarch64-apple-ios-sim` rustup target.

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT/desktop"

BUILD_DIR="gen/apple/build"
APP_PATH="$BUILD_DIR/arm64-sim/Syscity.app"
BUNDLE_ID="net.syscity.desktop"

LAUNCH=1
DEVICE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-launch)
      LAUNCH=0
      shift
      ;;
    --device)
      DEVICE="${2:?--device requires a device name or UDID}"
      shift 2
      ;;
    *)
      echo "Unknown argument: $1" >&2
      echo "Usage: $0 [--skip-launch] [--device \"<name or UDID>\"]" >&2
      exit 1
      ;;
  esac
done

echo "Removing stale iOS build artifacts ..."
rm -rf "$BUILD_DIR/arm64-sim/Syscity.app" "$BUILD_DIR/syscity-desktop_iOS.xcarchive"

echo "Building Syscity iOS (release, aarch64-sim) ..."
cargo tauri ios build --target aarch64-sim --no-sign --ci

if [[ ! -d "$APP_PATH" ]]; then
  echo "ERROR: built app not found at $APP_PATH" >&2
  exit 1
fi

echo ""
echo "Built: $APP_PATH"
echo "  size: $(du -sh "$APP_PATH" | awk '{print $1}')"

if [[ "$LAUNCH" -eq 0 ]]; then
  echo "Skipping install/launch (--skip-launch)."
  exit 0
fi

# Pick a simulator: explicit --device, else the first booted one.
if [[ -n "$DEVICE" ]]; then
  SIM="$DEVICE"
else
  SIM="$(xcrun simctl list devices booted | grep -Eo '[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}' | head -1 || true)"
  if [[ -z "$SIM" ]]; then
    echo "No booted simulator. Boot one first, e.g.: xcrun simctl boot \"iPhone 16\"" >&2
    exit 1
  fi
fi

echo "Installing on simulator: $SIM"
xcrun simctl install "$SIM" "$APP_PATH"

echo "Launching $BUNDLE_ID ..."
xcrun simctl launch "$SIM" "$BUNDLE_ID"
