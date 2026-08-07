#!/usr/bin/env bash
#
# Run the GitHub CI pipeline in a Linux container.
#
# Mirrors .github/workflows/ci.yml on ubuntu-latest: builds the ci image once
# (cached afterwards), then runs scripts/ci-check.sh inside a container with
# the repo mounted at /workspace. The container's Linux node_modules, cargo
# registry, and build artifacts are kept in named volumes so they don't touch
# your host's macOS caches and are reused across runs.
#
# Usage:
#   ./scripts/ci-check-docker.sh               # full CI mirror in Linux
#   ./scripts/ci-check-docker.sh --skip-tests  # skip cargo test steps
#   ./scripts/ci-check-docker.sh --backend     # Rust steps only
#
# Requires a running Docker daemon (OrbStack on macOS). The SYS_PTRACE cap is
# kept from the tarpaulin era; llvm-cov doesn't need it but it is harmless.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

IMAGE="${SYSCTY_CI_IMAGE:-syscity-ci:latest}"

if ! docker info >/dev/null 2>&1; then
    echo "Docker daemon is not running." >&2
    echo "On macOS: start OrbStack (or run 'open -a OrbStack') and retry." >&2
    exit 1
fi

echo "==> Building CI image ($IMAGE) — cached on subsequent runs"
docker build -f scripts/Dockerfile.ci -t "$IMAGE" scripts/

echo ""
echo "==> Running CI checks in Linux container"
exec docker run --rm --init --cap-add SYS_PTRACE \
    -v "$PWD:/workspace" \
    -v syscity-ci-target:/workspace/target \
    -v syscity-ci-node-modules:/workspace/web/node_modules \
    -v syscity-ci-cargo-registry:/usr/local/cargo/registry \
    -v syscity-ci-cargo-git:/usr/local/cargo/git \
    -v syscity-ci-pnpm-store:/pnpm-store \
    "$IMAGE" "$@"
