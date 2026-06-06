#!/usr/bin/env bash
set -euo pipefail

# Syscity Desktop — Production build
# Builds release binaries and platform-specific bundles.

cd "$(dirname "$0")/../desktop"

# llama-cpp-sys-2 uses std::filesystem which requires macOS 10.15+
export MACOSX_DEPLOYMENT_TARGET=10.15
export CMAKE_OSX_DEPLOYMENT_TARGET=10.15

echo "Building Syscity Desktop..."
cargo tauri build "$@"

echo ""
echo "Build complete. Bundles are in:"
echo "  desktop/target/release/bundle/"
