#!/usr/bin/env bash
#
# Gate integrity checks — machine-enforced evaluation discipline.
#
# The release gate's meaning must not be quietly changed to make a failing
# version pass ("don't lower the bar to make the numbers fit"). This script
# enforces that in two modes:
#
#   --staged  (pre-commit)  checks what is about to be committed:
#       * hard-fail: min_pass_rate lowered or removed in evals/**/*.yaml
#       * warn:      criteria/conditions/threshold edits in evals yaml
#       * warn:      threshold-constant changes in src/eval/{loader,scorer}.rs
#
#   --tree    (CI)          checks whole-tree invariants:
#       * the release_gate suite keeps min_pass_rate >= 0.85
#       * no eval task-id literals hard-coded in src/** outside src/eval/
#         (the engine must never special-case evaluation tasks)
#
# Usage: scripts/gate-integrity.sh [--staged|--tree]   (default: --tree)

set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
RESET='\033[0m'

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$ROOT" ]; then
    echo "gate-integrity: must run inside a git repository" >&2
    exit 1
fi
cd "$ROOT"

errors=0
warnings=0
fail() {
    echo -e "${RED}FAIL${RESET}  $1"
    errors=$((errors + 1))
}
warn() {
    echo -e "${YELLOW}WARN${RESET}  $1"
    warnings=$((warnings + 1))
}
pass() { echo -e "${GREEN}PASS${RESET}  $1"; }

MODE="${1:---tree}"

# ── helpers ─────────────────────────────────────────────────────────────────

# Extract the first `min_pass_rate:` value from stdin (empty if absent).
rate_of() {
    awk '/^[[:space:]]*min_pass_rate:/ { print $2; exit }'
}

# a < b on floats
float_lt() {
    awk -v a="$1" -v b="$2" 'BEGIN { exit !(a < b) }'
}

# The release gate is the regression + adversarial sections only (see
# evals/suites/release_gate.yaml). Capability/skill task ids are deliberately
# named after the tools they exercise (image_generate, memory_search, ...)
# and would false-positive against legitimate tool name references, so they
# are excluded from the hard-coding scan.
TASK_YAML_DIRS=(evals/adversarial evals/regression)

# All declared eval task ids (one per line).
task_ids() {
    local dirs=()
    for d in "${TASK_YAML_DIRS[@]}"; do
        [ -d "$d" ] && dirs+=("$d")
    done
    [ "${#dirs[@]}" -eq 0 ] && return 0
    grep -rhoE '^[[:space:]]*- id: [A-Za-z0-9_]+' "${dirs[@]}" --include='*.yaml' 2>/dev/null \
        | awk '{ print $3 }' | sort -u
}

# ── --staged: diff-based pre-commit checks ──────────────────────────────────

check_staged() {
    local staged
    staged=$(git diff --cached --name-only --diff-filter=ACMR)
    [ -z "$staged" ] && { echo "gate-integrity: nothing staged"; exit 0; }

    echo "🛡  gate integrity (staged)"

    # 1. min_pass_rate must not be lowered or removed in evals yaml.
    local f old new
    for f in $(printf '%s\n' "$staged" | grep -E '^evals/.*\.yaml$' | grep -vE '^evals/(badcases|actions)/' || true); do
        old=$(git show "HEAD:$f" 2>/dev/null | rate_of)
        new=$(git show ":$f" 2>/dev/null | rate_of)
        if [ -n "$old" ] && [ -n "$new" ]; then
            if float_lt "$new" "$old"; then
                fail "$f: min_pass_rate lowered $old -> $new (don't lower the bar to make a version pass)"
            fi
        elif [ -n "$old" ] && [ -z "$new" ]; then
            fail "$f: min_pass_rate removed (falls back to a lower default)"
        fi

        # 2. Criteria / conditions / threshold edits are legitimate when a
        #    task is genuinely broken, but must be visible and deliberate.
        if git diff --cached -- "$f" | grep -E '^[+-][^+-]' \
            | grep -qE '(^|[[:space:]])(criteria|conditions|threshold|pass_rate)(:|[[:space:]]|$)'; then
            warn "$f: scoring criteria/conditions changed — review that this fixes the task, not the verdict"
        fi
    done

    # 3. Threshold constants in the eval engine.
    for f in $(printf '%s\n' "$staged" | grep -E '^src/eval/(loader|scorer)\.rs$' || true); do
        if git diff --cached -- "$f" | grep -E '^[+-][^+-]' \
            | grep -qE '(min_pass_rate|_threshold|0\.8[0-9]*|0\.9[0-9]*)'; then
            warn "$f: eval threshold constants changed — confirm this is not a silent gate change"
        fi
    done

    if [ "$errors" -gt 0 ]; then
        echo -e "${RED}${BOLD}$errors gate-integrity error(s).${RESET}"
        echo "    A lowered threshold blocks the commit; if the change is deliberate,"
        echo "    split it into its own commit and bypass with: git commit --no-verify"
        exit 1
    fi
    [ "$warnings" -gt 0 ] && echo -e "${YELLOW}$warnings warning(s) — review above.${RESET}"
    [ "$errors" -eq 0 ] && [ "$warnings" -eq 0 ] && pass "gate thresholds untouched"
    exit 0
}

# ── --tree: whole-tree invariants for CI ────────────────────────────────────

check_tree() {
    echo "🛡  gate integrity (tree)"

    # 1. Release gate keeps its certified bar.
    local gate_yaml="evals/suites/release_gate.yaml"
    if [ -f "$gate_yaml" ]; then
        local rate
        rate=$(rate_of < "$gate_yaml")
        if [ -z "$rate" ]; then
            fail "$gate_yaml: min_pass_rate missing"
        elif float_lt "$rate" 0.85; then
            fail "$gate_yaml: min_pass_rate $rate < 0.85 (release gate bar)"
        else
            pass "release_gate min_pass_rate = $rate (>= 0.85)"
        fi
    else
        warn "$gate_yaml not found (skipping release-gate bar check)"
    fi

    # 2. No eval task-id literals hard-coded outside the eval subsystem.
    #    The engine/tools must never special-case evaluation tasks.
    local ids id hits hardcoded=""
    ids=$(task_ids)
    if [ -n "$ids" ]; then
        while IFS= read -r id; do
            [ -z "$id" ] && continue
            hits=$(git grep -nIwF "$id" -- 'src/' ':(exclude)src/eval/' 2>/dev/null || true)
            if [ -n "$hits" ]; then
                hardcoded+="       $id"$'\n'"$(printf '%s\n' "$hits" | head -3 | sed 's/^/         /')"$'\n'
            fi
        done <<< "$ids"
        if [ -n "$hardcoded" ]; then
            fail "eval task ids hard-coded in src/ (outside src/eval/):"
            printf '%s' "$hardcoded"
        else
            pass "no eval task ids hard-coded outside src/eval/"
        fi
    else
        warn "no eval task ids found to scan"
    fi

    echo ""
    if [ "$errors" -gt 0 ]; then
        echo -e "${RED}${BOLD}$errors gate-integrity error(s).${RESET}"
        exit 1
    fi
    exit 0
}

case "$MODE" in
    --staged) check_staged ;;
    --tree) check_tree ;;
    *)
        echo "usage: scripts/gate-integrity.sh [--staged|--tree]" >&2
        exit 2
        ;;
esac
