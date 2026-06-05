#!/usr/bin/env bash
set -euo pipefail

# Syscity Desktop — Development mode
# Runs the Tauri app with hot-reload for both frontend and Rust backend.

cd "$(dirname "$0")/../src"

cargo tauri dev
