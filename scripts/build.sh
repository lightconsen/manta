#!/bin/bash
# Syscity Build Script
# Builds web terminal, cleans Rust artifacts, and builds release binary
#
# Usage:
#   ./build.sh           Build frontend + backend (full)
#   ./build.sh --front   Build frontend only
#   ./build.sh --cloud   Also enable the cloud Cargo feature (off by default)
#   ./build.sh --front --cloud   Frontend only (cloud is ignored here)

set -e  # Exit on error

FRONT_ONLY=false
CLOUD=false
for arg in "$@"; do
  case "$arg" in
    --front) FRONT_ONLY=true ;;
    --cloud) CLOUD=true ;;
    -h|--help)
      echo "Usage:"
      echo "  ./build.sh             Build frontend + backend (full)"
      echo "  ./build.sh --front     Build frontend only"
      echo "  ./build.sh --cloud     Include the cloud Cargo feature (off by default)"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

echo "🚀 Starting Syscity build..."

# Navigate to project root (in case script is run from scripts/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

# Build web frontend
echo "📦 Building web frontend..."
cd web
pnpm install --frozen-lockfile
pnpm build
cd ..

if [ "$FRONT_ONLY" = true ]; then
  echo "✅ Frontend build complete!"
  echo "📦 Web bundle: ./dist/"
echo "   Frontend is served from dist/ at runtime — no binary rebuild needed for frontend changes."
  exit 0
fi

# Clean Rust build artifacts
echo "🧹 Cleaning Rust build artifacts..."
cargo clean

# Build release binary (frontend served from dist/ at runtime)
echo "🔨 Building release binary..."
CARGO_ARGS=(build --release)
if [ "$CLOUD" = true ]; then
  echo "   (with cloud feature)"
  CARGO_ARGS+=(--features cloud)
fi
cargo "${CARGO_ARGS[@]}"

echo "✅ Build complete!"
echo "📍 Binary location: ./target/release/syscity"
echo ""
echo "Run with:"
echo "  SYSCITY_BASE_URL=\"https://coding.dashscope.aliyuncs.com/v1\" \\"
echo "  SYSCITY_API_KEY=\"your-api-key\" \\"
echo "  SYSCITY_MODEL=\"qwen3.5-plus\" \\"
echo "  ./target/release/syscity start --foreground"
