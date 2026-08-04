#!/usr/bin/env bash
set -euo pipefail

# Syscity Android — Tauri mobile build.
# Builds the Android APK (debug or release) for the given target ABIs.
#
# Usage:
#   ./android-build.sh              Release APK, arm64 (default)
#   ./android-build.sh --debug      Debug APK, arm64 (emulator / on-device testing)
#   ./android-build.sh --debug x86_64
#   ./android-build.sh --release aarch64 x86_64
#
# Notes:
#   - Must run from the repo root or anywhere on disk (it cd's to desktop/).
#   - Release builds sign with gen/android/syscity-release.keystore
#     (see gen/android/app/keystore.properties) and bundle libc++_shared.so
#     automatically (desktop/build.rs links -lc++_shared for android).
#   - Requires Android SDK + NDK and the `cargo tauri` CLI.

PROFILE="release"
TARGETS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)
      PROFILE="debug"
      shift
      ;;
    --release)
      # `cargo tauri android build` has no --release flag; release is the
      # default. Accepted for symmetry; treat as a no-op.
      PROFILE="release"
      shift
      ;;
    *)
      TARGETS+=("$1")
      shift
      ;;
  esac
done

if [[ ${#TARGETS[@]} -eq 0 ]]; then
  TARGETS=("aarch64")
fi

cd "$(dirname "$0")/../desktop"

CARGO_ARGS=("android" "build" "--apk")
if [[ "$PROFILE" == "debug" ]]; then
  CARGO_ARGS+=("--debug")
fi
for t in "${TARGETS[@]}"; do
  CARGO_ARGS+=("--target" "$t")
done

echo "Building Syscity Android ($PROFILE) for targets: ${TARGETS[*]}"
cargo tauri "${CARGO_ARGS[@]}"

echo ""
echo "Build complete. APKs are in:"
echo "  gen/android/app/build/outputs/apk/"
