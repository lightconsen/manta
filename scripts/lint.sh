#!/usr/bin/env bash
#
# Local lint / static analysis script for Syscity.
#
# Runs the same checks that CI enforces, plus project-specific static
# analysis for recurring gateway correctness anti-patterns.
#
# Usage:
#   ./scripts/lint.sh              # Run all checks
#   ./scripts/lint.sh --quick      # Skip expensive test/doc builds
#   ./scripts/lint.sh --fix        # Run cargo fmt before checks
#

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
RESET='\033[0m'

QUICK=false
FIX=false

for arg in "$@"; do
    case "$arg" in
        --quick) QUICK=true ;;
        --fix) FIX=true ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --quick    Skip expensive test/doc builds"
            echo "  --fix      Run cargo fmt before checks"
            echo "  -h, --help Show this help"
            exit 0
            ;;
    esac
done

run_step() {
    local name=$1
    shift
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo -e "${BOLD}$name${RESET}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    if "$@"; then
        echo -e "${GREEN}PASS${RESET}  $name"
        return 0
    else
        echo -e "${RED}FAIL${RESET}  $name"
        return 1
    fi
}

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

# ── Optional format fix ────────────────────────────────────────────────────
if $FIX; then
    run_step "cargo fmt" cargo +nightly fmt
fi

# ── Format check ───────────────────────────────────────────────────────────
run_step "cargo fmt --check" cargo +nightly fmt -- --check

# ── Clippy with warnings as errors ─────────────────────────────────────────
run_step "cargo clippy" cargo clippy --all-features -- -D warnings

# ── Static analysis: recurring anti-patterns ───────────────────────────────
run_step "static analysis" bash scripts/static-analysis.sh

# ── Security audit ─────────────────────────────────────────────────────────
if ! $QUICK; then
    run_step "security audit" bash scripts/audit.sh --quick
fi

# ── Check / test (skipped in quick mode) ───────────────────────────────────
if ! $QUICK; then
    run_step "cargo check" cargo check --all-features
    run_step "cargo test --lib" cargo test --lib
    run_step "cargo doc" cargo doc --no-deps --all-features
fi

echo ""
echo -e "${GREEN}${BOLD}All lint checks passed.${RESET}"
echo ""
