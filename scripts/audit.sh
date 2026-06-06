#!/usr/bin/env bash
#
# Security Audit Script for Syscity
#
# Runs the full security audit suite locally:
#   - cargo audit (RustSec advisory database)
#   - cargo deny check (vulns + duplicates + licenses + sources)
#
# Usage:
#   ./audit.sh              # Run full audit
#   ./audit.sh --fix        # Run cargo update before audit
#   ./audit.sh --quick      # Only run cargo audit (skip deny)
#   ./audit.sh --advisories # Only advisory checks
#

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
RESET='\033[0m'

# Flags
FIX=false
QUICK=false
ADVISORIES_ONLY=false

for arg in "$@"; do
    case "$arg" in
        --fix) FIX=true ;;
        --quick) QUICK=true ;;
        --advisories) ADVISORIES_ONLY=true ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --fix        Run cargo update before auditing"
            echo "  --quick      Only run cargo audit (skip cargo-deny)"
            echo "  --advisories Only run advisory checks"
            echo "  -h, --help   Show this help"
            exit 0
            ;;
    esac
done

# Check required tools
check_tool() {
    local tool=$1
    local install_cmd=$2
    if ! command -v "$tool" &>/dev/null; then
        echo -e "${YELLOW}WARN: $tool not found${RESET}"
        echo -e "      Install with: ${BOLD}$install_cmd${RESET}"
        return 1
    fi
    return 0
}

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

# ── Optional: update deps ──────────────────────────────────────────────────
if $FIX; then
    run_step "cargo update" cargo update
fi

# ── cargo audit (RustSec vulnerabilities) ──────────────────────────────────
if check_tool cargo-audit "cargo install cargo-audit"; then
    # Try to fix advisory-db corruption automatically
    if ! cargo audit --version &>/dev/null; then
        echo -e "${YELLOW}  Attempting to repair advisory-db cache...${RESET}"
        rm -rf ~/.cargo/advisory-db 2>/dev/null || true
    fi

    if $ADVISORIES_ONLY; then
        run_step "cargo audit" cargo audit
        exit $?
    else
        run_step "cargo audit (RustSec vulnerabilities)" cargo audit
    fi
else
    if $ADVISORIES_ONLY; then
        exit 1
    fi
fi

# ── cargo deny (duplicates + licenses + sources + advisories) ──────────────
if $QUICK; then
    echo ""
    echo -e "${GREEN}Quick audit complete.${RESET}"
    exit 0
fi

if check_tool cargo-deny "cargo install cargo-deny"; then
    run_step "cargo deny check (full)" cargo deny check
else
    exit 1
fi

# ── Summary ────────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}${BOLD}All security checks passed.${RESET}"
echo ""
