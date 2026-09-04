#!/bin/bash
# Syscity Release Script
# Bumps the version, updates the CHANGELOG, and publishes a release tag.
#
# Usage:
#   ./scripts/release.sh                # patch bump (default): 0.3.1 → 0.3.2
#   ./scripts/release.sh --patch        # patch bump:   0.3.1 → 0.3.2
#   ./scripts/release.sh --minor        # minor bump:   0.3.1 → 0.4.0
#   ./scripts/release.sh --major        # major bump:   0.3.1 → 1.0.0
#   ./scripts/release.sh --tag v1.2.3   # custom tag (no bump / changelog / commit)
#   --no-edit                           # don't open an editor on CHANGELOG.md
#   --skip-tests                        # skip the pre-release cargo test run
#
# Release flow (bump modes):
#   1. Preconditions: clean tree, on main, in sync with origin/main, target tag free
#   2. cargo test --all-features --lib (unless --skip-tests)
#   3. Promote the CHANGELOG `## [Unreleased]` section to `## [X.Y.Z] - <date>`;
#      if that section is empty, draft one from commit subjects since the
#      previous tag (✨→Added, 🐛→Fixed, rest→Changed). Opens $EDITOR unless
#      --no-edit or non-interactive.
#   4. Bump the version in Cargo.toml, desktop/Cargo.toml, web/package.json
#      and refresh Cargo.lock
#   5. Commit `🔖 chore(release): vX.Y.Z`, push main, create the tag and push it
#      (pushing the tag triggers the Release workflow that builds artifacts)

set -euo pipefail

BUMP=""
CUSTOM_TAG=""
NO_EDIT=false
SKIP_TESTS=false

while [ $# -gt 0 ]; do
  case "$1" in
    --patch|--minor|--major)
      if [ -n "$BUMP" ] && [ "$BUMP" != "${1#--}" ]; then
        echo "✗ Pick only one of --patch/--minor/--major" >&2
        exit 1
      fi
      BUMP="${1#--}"
      ;;
    --tag)
      CUSTOM_TAG="${2:-}"
      [ -n "$CUSTOM_TAG" ] || { echo "✗ --tag requires a value" >&2; exit 1; }
      shift
      ;;
    --no-edit) NO_EDIT=true ;;
    --skip-tests) SKIP_TESTS=true ;;
    -h|--help)
      echo "Usage: ./scripts/release.sh [--patch|--minor|--major|--tag <tag>] [--no-edit] [--skip-tests]"
      echo ""
      echo "  --patch       bump the patch version (default): 0.3.1 → 0.3.2"
      echo "  --minor       bump the minor version:           0.3.1 → 0.4.0"
      echo "  --major       bump the major version:           0.3.1 → 1.0.0"
      echo "  --tag <tag>   publish a custom tag without bumping/committing"
      echo "  --no-edit     skip opening an editor on CHANGELOG.md"
      echo "  --skip-tests  skip the pre-release test run"
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
  shift
done
BUMP="${BUMP:-patch}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "🧭 Checking preconditions..."
if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "✗ Not in a git repository" >&2
  exit 1
fi
if ! git diff-index --quiet HEAD --; then
  echo "✗ Uncommitted changes to tracked files — commit or stash first" >&2
  git status --short >&2
  exit 1
fi
branch="$(git rev-parse --abbrev-ref HEAD)"
if [ "$branch" != "main" ]; then
  echo "✗ Releases are cut from main (currently on $branch)" >&2
  exit 1
fi
git fetch -q origin main
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
  echo "✗ main is out of sync with origin/main — push/pull first" >&2
  exit 1
fi

if [ -z "$CUSTOM_TAG" ]; then
  current="$(grep -m1 '^version = ' Cargo.toml | cut -d '"' -f2)"
  desktop_current="$(grep -m1 '^version = ' desktop/Cargo.toml | cut -d '"' -f2)"
  web_current="$(grep -m1 '"version"' web/package.json | sed 's/.*"version": *"\([^"]*\)".*/\1/')"
  if [ "$current" != "$desktop_current" ] || [ "$current" != "$web_current" ]; then
    echo "✗ Version files disagree: Cargo.toml=$current desktop=$desktop_current web=$web_current" >&2
    exit 1
  fi

  IFS='.' read -r MA MI PA <<< "$current"
  case "$BUMP" in
    major) MA=$((MA + 1)); MI=0; PA=0 ;;
    minor) MI=$((MI + 1)); PA=0 ;;
    patch) PA=$((PA + 1)) ;;
  esac
  next="$MA.$MI.$PA"
  TAG="v$next"
  echo "🚀 Releasing $current → $next"
else
  TAG="$CUSTOM_TAG"
  echo "🚀 Publishing custom tag: $TAG"
  if ! [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    printf "⚠️  Tag '%s' is not plain semver with a v prefix. Continue? (y/N) " "$TAG"
    read -r -n 1 reply
    echo
    case "$reply" in
      y|Y) ;;
      *) exit 1 ;;
    esac
  fi
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  echo "✗ Tag $TAG already exists locally" >&2
  exit 1
fi
if git ls-remote --tags origin "refs/tags/$TAG" | grep -qF "refs/tags/$TAG"; then
  echo "✗ Tag $TAG already exists on origin" >&2
  exit 1
fi

if [ -z "$CUSTOM_TAG" ] && [ "$SKIP_TESTS" = false ]; then
  echo "🧪 Running cargo test --all-features --lib ..."
  cargo test -q --all-features --lib
fi

if [ -n "$CUSTOM_TAG" ]; then
  git tag "$TAG"
  git push -q origin "$TAG"
  echo "✅ Published $TAG"
  echo "   The Release workflow is building artifacts — track it with:"
  echo "     gh run watch \$(gh run list --workflow=release.yml --limit 1 --json databaseId -q '.[0].databaseId')"
  exit 0
fi

echo "📝 Updating CHANGELOG.md..."
release_date="$(date +%F)"
added=""
fixed=""
changed=""
if git rev-parse -q --verify "refs/tags/v$current" >/dev/null 2>&1; then
  subjects="$(git log "v$current..HEAD" --pretty=%s)"
  # Only pipe non-empty subjects: `echo ""` emits an empty line and the
  # exclusion grep (-Ev) would pass it through, producing a phantom "- "
  # bullet that defeats the "Nothing to release" guard below.
  if [ -n "$subjects" ]; then
    added="$(echo "$subjects" | grep '^✨' | sed 's/^[^ ]* //' | sed 's/^/- /' || true)"
    fixed="$(echo "$subjects" | grep '^🐛' | sed 's/^[^ ]* //' | sed 's/^/- /' || true)"
    changed="$(echo "$subjects" | grep -Ev '^(✨|🐛|🔖)' | sed 's/^[^ ]* //' | sed 's/^/- /' || true)"
  fi
fi

source_note="$(python3 - "$next" "$release_date" "$current" "$added" "$fixed" "$changed" <<'PY'
import re
import sys

next_ver, date, current, added, fixed, changed = sys.argv[1:7]
path = "CHANGELOG.md"
text = open(path, encoding="utf-8").read()

marker = "## [Unreleased]"
try:
    start = text.index(marker)
except ValueError:
    sys.exit("CHANGELOG.md has no ## [Unreleased] section")
rest = text[start + len(marker):]
m = re.search(r"\n## \[", rest)
body, tail = (rest[: m.start()], rest[m.start():]) if m else (rest, "")
body = body.strip("\n")

if not body.strip():
    parts = []
    if added.strip():
        parts.append("### Added\n\n" + added.strip())
    if fixed.strip():
        parts.append("### Fixed\n\n" + fixed.strip())
    if changed.strip():
        parts.append("### Changed\n\n" + changed.strip())
    if not parts:
        sys.exit(
            "Nothing to release: the Unreleased section is empty and there are "
            "no commits since v" + current
        )
    body = "\n\n".join(parts)
    source = "Drafted release notes from commits since v" + current
else:
    source = "Promoted the Unreleased section to " + next_ver

section = f"## [{next_ver}] - {date}\n\n{body}\n"
open(path, "w", encoding="utf-8").write(text[:start] + marker + "\n\n" + section + tail)
print(source)
PY
)"

if ! grep -q "^## \[$next\]" CHANGELOG.md; then
  echo "✗ $source_note" >&2
  exit 1
fi
echo "   $source_note"

if [ "$NO_EDIT" = false ] && [ -t 0 ]; then
  echo "✏️  Review CHANGELOG.md (${EDITOR:-vi})..."
  "${EDITOR:-vi}" CHANGELOG.md
fi

echo "🔢 Bumping versions to $next..."
perl -pi -e 's/^version = "[^"]*"/version = "'"$next"'"/' Cargo.toml desktop/Cargo.toml
perl -pi -e 's/("version":\s*)"[^"]*"/$1"'"$next"'"/' web/package.json
cargo check -q  # refresh Cargo.lock + catch compile breakage before publishing

git add CHANGELOG.md Cargo.toml Cargo.lock desktop/Cargo.toml web/package.json
git commit -m "🔖 chore(release): $TAG"
git push -q origin main
git tag "$TAG"
git push -q origin "$TAG"

echo "✅ Released $TAG"
echo "   The Release workflow is building artifacts — track it with:"
echo "     gh run watch \$(gh run list --workflow=release.yml --limit 1 --json databaseId -q '.[0].databaseId')"
