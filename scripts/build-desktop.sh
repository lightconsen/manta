#!/usr/bin/env bash
set -euo pipefail

# Syscity Desktop — Production build
# Builds release binaries and platform-specific bundles.

cd "$(dirname "$0")/../desktop"

echo "Building Syscity Desktop..."
cargo tauri build "$@"

echo ""
echo "Build complete. Bundles are in:"
echo "  desktop/target/release/bundle/"
