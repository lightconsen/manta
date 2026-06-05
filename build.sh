#!/bin/bash
# Syscity Build Script
# Builds web terminal, cleans Rust artifacts, and builds release binary
#
# Usage:
#   ./build.sh        Build frontend + backend (full)
#   ./build.sh --front  Build frontend only

set -e  # Exit on error

FRONT_ONLY=false
if [ "$1" = "--front" ]; then
  FRONT_ONLY=true
fi

echo "🚀 Starting Syscity build..."

# Build web frontend
echo "📦 Building web frontend..."
cd web/chat-ui
pnpm install --frozen-lockfile --allow-build
pnpm build
cd ../..

if [ "$FRONT_ONLY" = true ]; then
  echo "✅ Frontend build complete!"
  echo "📦 Web bundle: ./web/dist/"
  exit 0
fi

# Clean Rust build artifacts
echo "🧹 Cleaning Rust build artifacts..."
cargo clean

# Build release binary
echo "🔨 Building release binary..."
cargo build --release

echo "✅ Build complete!"
echo "📍 Binary location: ./target/release/syscity"
echo ""
echo "Run with:"
echo "  SYSCITY_BASE_URL=\"https://coding.dashscope.aliyuncs.com/v1\" \\"
echo "  SYSCITY_API_KEY=\"your-api-key\" \\"
echo "  SYSCITY_MODEL=\"qwen3.5-plus\" \\"
echo "  ./target/release/syscity start --foreground"
