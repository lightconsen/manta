#!/bin/bash
# Syscity Local Development Startup Script
# Loads environment from .env file and starts the server.
#
# Usage:
#   ./scripts/start-local.sh          # Start with .env config
#   ./scripts/start-local.sh --env .env.deepseek  # Use a different env file
#
# Prerequisites:
#   1. Copy scripts/.env.example to scripts/.env
#   2. Fill in your actual API key in scripts/.env
#   3. Build the binary: ./scripts/build.sh

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()  { echo -e "${BLUE}ℹ️  $1${NC}"; }
log_success() { echo -e "${GREEN}✅ $1${NC}"; }
log_warn()  { echo -e "${YELLOW}⚠️  $1${NC}"; }
log_error() { echo -e "${RED}❌ $1${NC}"; }

# Resolve project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

# Determine env file
ENV_FILE="$SCRIPT_DIR/.env"
if [[ $# -ge 2 && "$1" == "--env" ]]; then
    ENV_FILE="$2"
    shift 2
fi

# Load .env
if [[ ! -f "$ENV_FILE" ]]; then
    log_error "Env file not found: $ENV_FILE"
    echo ""
    echo "  1. Copy the example file:"
    echo "     cp $SCRIPT_DIR/.env.example $SCRIPT_DIR/.env"
    echo ""
    echo "  2. Edit .env and add your API key"
    echo ""
    exit 1
fi

set -a
source "$ENV_FILE"
set +a

# Validate required env vars
if [[ -z "${SYSCITY_API_KEY:-}" ]]; then
    log_error "SYSCITY_API_KEY is not set in $ENV_FILE"
    exit 1
fi
if [[ -z "${SYSCITY_BASE_URL:-}" ]]; then
    log_error "SYSCITY_BASE_URL is not set in $ENV_FILE"
    exit 1
fi

# Detect API type label
API_LABEL="OpenAI-compatible"
if [[ "${SYSCITY_IS_ANTHROPIC:-false}" == "true" ]]; then
    API_LABEL="Anthropic-compatible"
fi

MODEL="${SYSCITY_MODEL:-unknown}"

# Validate binary
if [[ ! -f "$PROJECT_ROOT/target/release/syscity" ]]; then
    log_warn "Binary not found at $PROJECT_ROOT/target/release/syscity"
    echo "Run ./scripts/build.sh first"
    exit 1
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
log_info "Force stopping any existing Syscity processes..."
pkill -9 syscity 2>/dev/null || true
sleep 2

# Truncate daemon log before starting fresh
LOG_FILE="$HOME/.syscity/logs/daemon.log"
if [[ -f "$LOG_FILE" ]]; then
    : > "$LOG_FILE"
    log_info "Truncated daemon log: $LOG_FILE"
fi

echo ""
log_success "Starting Syscity"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "   📡 Base URL: ${GREEN}${SYSCITY_BASE_URL}${NC}"
echo -e "   🤖 Model:    ${GREEN}${MODEL}${NC}"
echo -e "   🔌 API Type: ${GREEN}${API_LABEL}${NC}"
echo ""

# Start syscity
"$PROJECT_ROOT/target/release/syscity" start "$@"
