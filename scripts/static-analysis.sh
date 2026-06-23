#!/usr/bin/env bash
#
# Static analysis for recurring Syscity correctness anti-patterns.
#
# Currently focused on src/gateway/; expand patterns as new issue classes
# are identified during design reviews.
#

set -euo pipefail

RED='\033[0;31m'
YELLOW='\033[1;33m'
RESET='\033[0m'

failures=0

# Patterns we want to flag as errors in production code.
# Tests are allowed to use `.unwrap()` / `.expect()` freely.
analyze() {
    local pattern=$1
    local message=$2
    local files=$3

    local matches
    matches=$(git grep -n -E "$pattern" -- "$files" 2>/dev/null || true)
    if [[ -n "$matches" ]]; then
        echo ""
        echo -e "${RED}ERROR:${RESET} $message"
        echo -e "${YELLOW}Matches:${RESET}"
        echo "$matches"
        failures=$((failures + 1))
    fi
}

# 1. Silent discard of async Results in gateway code.
analyze \
    'let _ = .*\.await;?$' \
    "Found 'let _ = ... .await' in gateway code. Use if-let-Err or match and log/return the error." \
    'src/gateway/*.rs:src/gateway/**/*.rs'

# 2. Trailing .ok() that silently discards Results.
analyze \
    '\.ok\(\);?$' \
    "Found trailing '.ok()' that discards errors. Use match or if-let." \
    'src/gateway/*.rs:src/gateway/**/*.rs'

# 3. tokio::spawn without TaskRegistry registration in gateway.
# This is a heuristic: direct spawn calls are suspicious; helper wrappers are fine.
analyze \
    'tokio::spawn\(' \
    "Found direct tokio::spawn in gateway code. Consider registering the handle in TaskRegistry." \
    'src/gateway/*.rs:src/gateway/**/*.rs'

# 4. Holding a std::sync::MutexGuard across an await point.
# (Does not catch tokio::sync::Mutex, which is safe to hold across await.)
analyze \
    '\.lock\(\).*\.await' \
    "Possible std::sync::Mutex held across await. Use tokio::sync::Mutex or scope the lock." \
    'src/gateway/*.rs:src/gateway/**/*.rs'

# 5. tokio::time::sleep used as a shutdown signal instead of select.
# Heuristic: bare sleep in a loop body.
analyze \
    'loop \{.*tokio::time::sleep' \
    "Possible busy-wait loop using tokio::time::sleep without shutdown select." \
    'src/gateway/*.rs:src/gateway/**/*.rs'

if [[ $failures -gt 0 ]]; then
    echo ""
    echo -e "${RED}Static analysis failed with $failures issue(s).${RESET}"
    exit 1
fi

echo "Static analysis passed."
