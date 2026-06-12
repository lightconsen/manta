#!/usr/bin/env bash
# SDK Boundary Lint - checks plugin projects only depend on SDK crates
set -euo pipefail

PLUGIN_DIRS="${SYSCITY_PLUGIN_DIRS:-./plugins}"
errors=0

echo "=== SDK Boundary Lint ==="
echo "Scanning: $PLUGIN_DIRS"

for plugin_dir in $PLUGIN_DIRS; do
    [ -d "$plugin_dir" ] || { echo "  [SKIP] $plugin_dir: not found"; continue; }

    while IFS= read -r -d '' manifest; do
        plugin_name=$(basename "$(dirname "$manifest")")
        echo "  Checking: $plugin_name..."

        # Check for path dependency on main syscity crate
        if grep -qE 'syscity\s*=\s*\{.*path\s*=' "$manifest" 2>/dev/null; then
            echo "    ERROR: '$plugin_name' depends directly on internal 'syscity' crate!"
            echo "           Use 'syscity-plugin-sdk' instead."
            errors=$((errors + 1))
        fi

        # Check non-SDK path dependencies
        while IFS= read -r line; do
            dep_name=$(echo "$line" | sed -n 's/\[dependencies\.\(.*\)\]/\1/p')
            [ -z "$dep_name" ] && continue
            # Skip allowed SDK crates
            [[ "$dep_name" == syscity-plugin-sdk* ]] || [[ "$dep_name" == syscity-channel-sdk* ]] && continue

            # Check if this dep uses a path
            if grep -A2 "\[dependencies\.$dep_name\]" "$manifest" | grep -q 'path\s*='; then
                echo "    ERROR: '$plugin_name' depends on '$dep_name' via path (non-SDK)"
                errors=$((errors + 1))
            fi
        done < <(grep -E '^\[dependencies\.' "$manifest" 2>/dev/null || true)

        echo "    OK"
    done < <(find "$plugin_dir" -name 'Cargo.toml' -print0 2>/dev/null)
done

if [ "$errors" -gt 0 ]; then
    echo ""
    echo "FAILED: $errors plugin(s) violate SDK boundary."
    exit 1
fi
echo "All plugins respect SDK boundary."
