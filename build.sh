#!/bin/bash
# Manta Build Script
# Builds web terminal, cleans Rust artifacts, and builds release binary

set -e  # Exit on error

echo "🚀 Starting Manta build..."

# Build web terminal
echo "📦 Building web terminal..."
cd web
pnpm run build
cd ..

# Clean Rust build artifacts
echo "🧹 Cleaning Rust build artifacts..."
cargo clean

# Build release binary
echo "🔨 Building release binary..."
cargo build --release

echo "✅ Build complete!"
echo "📍 Binary location: ./target/release/manta"
echo ""
echo "Run with:"
echo "  MANTA_BASE_URL=\"https://coding.dashscope.aliyuncs.com/v1\" \\"
echo "  MANTA_API_KEY=\"your-api-key\" \\"
echo "  MANTA_MODEL=\"qwen3.5-plus\" \\"
echo "  ./target/release/manta start --foreground"
