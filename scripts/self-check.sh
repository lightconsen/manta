#!/usr/bin/env bash
#
# Self-check: pre-commit check aligned with GitHub CI.
#
# Thin wrapper around scripts/ci-check.sh, which mirrors every job in
# .github/workflows/ci.yml. A green self-check here means the push is very
# likely to pass CI — checks, not a lighter approximation.
#
# Usage:
#   ./scripts/self-check.sh          # full CI mirror (default)
#   ./scripts/self-check.sh --quick  # fast pre-commit: skip tests + frontend
#   ./scripts/self-check.sh --skip-tests  # skip all cargo test steps
#   ./scripts/self-check.sh --backend     # Rust steps only (no frontend)
#
# Exit code:
#   0 — all checks pass
#   1 — one or more checks failed
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Map the historical --quick flag to ci-check.sh's fast flags.
args=()
for arg in "$@"; do
    case "$arg" in
        --quick) args+=(--skip-tests --backend) ;;
        *) args+=("$arg") ;;
    esac
done

"$SCRIPT_DIR/ci-check.sh" "${args[@]}"

# ── Manual checklist (not automated) ────────────────────────────────────────
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
