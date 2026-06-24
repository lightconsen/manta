#!/usr/bin/env bash
#
# Self-check: run all automated checks before committing.
#
# Usage:
#   ./scripts/self-check.sh         # full check (default)
#   ./scripts/self-check.sh --quick # skip docs and audit, run tests in lib only
#
# Exit code:
#   0 — all automated checks pass
#   1 — one or more automated checks failed
#
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RESET='\033[0m'

QUICK=false
for arg in "$@"; do
    case "$arg" in
        --quick) QUICK=true ;;
        -h|--help)
            echo "Usage: $0 [--quick]"
            echo ""
            echo "  (no flag)  Full check: fmt, clippy, static-analysis(2x), audit, docs, tests"
            echo "  --quick    Skip docs and audit, run lib tests only"
            exit 0
            ;;
    esac
done

errors=0

check() {
    local name=$1
    shift
    echo -e "${YELLOW}[${name}]${RESET} $*"
    if "$@" 2>&1; then
        echo -e "${GREEN}[${name}] passed${RESET}"
    else
        echo -e "${RED}[${name}] FAILED${RESET}"
        errors=$((errors + 1))
    fi
    echo ""
}

# ── Step 1: Format ────────────────────────────────────────────────────────
check "fmt" cargo fmt -- --check

# ── Step 2: Clippy ────────────────────────────────────────────────────────
check "clippy" cargo clippy -- -D warnings

# ── Step 3: Static analysis (CI mode) ─────────────────────────────────────
check "static-analysis" ./scripts/static-analysis.sh

# ── Step 4: Static analysis (full mode) ───────────────────────────────────
check "static-analysis --full" ./scripts/static-analysis.sh --full

# ── Step 5: Tests ─────────────────────────────────────────────────────────
if $QUICK; then
    check "test --lib" cargo test --lib
else
    check "test --all-features" cargo test --all-features
fi

# ── Step 6: Docs ──────────────────────────────────────────────────────────
if ! $QUICK; then
    check "doc" cargo doc --no-deps
fi

# ── Step 7: Security audit ────────────────────────────────────────────────
if ! $QUICK; then
    check "audit" cargo audit 2>/dev/null || echo "  (cargo audit not installed or failed — skipping)"
fi

# ── Summary ────────────────────────────────────────────────────────────────
if [[ $errors -gt 0 ]]; then
    echo -e "${RED}$errors check(s) failed.${RESET}"
    echo ""
    exit 1
fi

echo -e "${GREEN}All automated checks passed.${RESET}"
echo ""
echo "────────────────────────────────────────────────────────────────"
echo "  Manual checklist (not automated)"
echo "────────────────────────────────────────────────────────────────"
echo ""
echo "  □ All tokio::spawn handles registered in TaskRegistry?"
echo "  □ All long-running loops use select! with a shutdown signal?"
echo "  □ Audit log / event send failures logged with warn!?"
echo "  □ Changes covered by tests (unit, integration, E2E)?"
echo ""
