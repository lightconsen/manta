#!/usr/bin/env bash
#
# clean.sh — Clean all cargo/build artifacts (desktop + mobile).
#
# Removes compiled output for the Rust workspace (shared by the desktop app,
# the Android build, and the iOS simulator build), the Android gradle build,
# and the iOS/xcode + Swift staging build. Only gitignored build-output
# directories are touched; tracked sources (gen/apple/xcodeproj, gen/android
# Kotlin sources, main.mm, ...) are never deleted.
#
# Usage:
#   ./scripts/clean.sh [flags]
#
# Flags:
#   -c, --cargo     Clean only the Rust workspace target/.
#   -a, --android   Clean only Android gradle build outputs.
#   -i, --ios       Clean only iOS/xcode + Swift staging outputs.
#   -w, --web       Also clean the web frontend dist/ (not cargo, opt-in).
#   -y, --yes       Skip the confirmation prompt.
#   -h, --help      Show this help.
#
# With no flags, everything is cleaned (cargo + android + ios).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DO_CARGO=0
DO_ANDROID=0
DO_IOS=0
DO_WEB=0
ASSUME_YES=0

usage() {
  cat <<'EOF'
clean.sh — Clean all cargo/build artifacts (desktop + mobile).

Usage:
  ./scripts/clean.sh [flags]

Flags:
  -c, --cargo     Clean only the Rust workspace target/.
  -a, --android   Clean only Android gradle build outputs.
  -i, --ios       Clean only iOS/xcode + Swift staging outputs.
  -w, --web       Also clean the web frontend dist/ (not cargo, opt-in).
  -y, --yes       Skip the confirmation prompt.
  -h, --help      Show this help.

With no flags, everything is cleaned (cargo + android + ios).
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    -c|--cargo)   DO_CARGO=1; shift ;;
    -a|--android) DO_ANDROID=1; shift ;;
    -i|--ios)     DO_IOS=1; shift ;;
    -w|--web)     DO_WEB=1; shift ;;
    -y|--yes)     ASSUME_YES=1; shift ;;
    -h|--help)    usage; exit 0 ;;
    *)
      echo "Unknown flag: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

# No flags → clean everything.
if [ "$DO_CARGO" -eq 0 ] && [ "$DO_ANDROID" -eq 0 ] && [ "$DO_IOS" -eq 0 ]; then
  DO_CARGO=1
  DO_ANDROID=1
  DO_IOS=1
fi

# Gitignored build-output directories (verified against .gitignore / per-gen .gitignore).
CARGO_TARGET="$REPO_ROOT/target"

ANDROID_DIRS=(
  "$REPO_ROOT/desktop/gen/android/app/build"
  "$REPO_ROOT/desktop/gen/android/build"
  "$REPO_ROOT/desktop/gen/android/.gradle"
)

IOS_DIRS=(
  "$REPO_ROOT/desktop/gen/apple/build"
  "$REPO_ROOT/desktop/gen/apple/Externals"
  "$REPO_ROOT/desktop/mobile-ios/.build"
  "$REPO_ROOT/desktop/mobile-ios/.tauri"
)

WEB_DIRS=(
  "$REPO_ROOT/web/dist"
)

# Refuse to remove anything outside the repo.
rm_dir() {
  local path="$1"
  case "$path" in
    "$REPO_ROOT"/*) : ;;
    *)
      echo "Refusing to remove path outside the repo: $path" >&2
      exit 1
      ;;
  esac
  if [ -d "$path" ]; then
    printf "  %-66s %s\n" "${path#"$REPO_ROOT"/}" "$(du -sh "$path" 2>/dev/null | cut -f1)"
    rm -rf "$path"
  fi
}

clean_cargo() {
  echo "==> Rust workspace (desktop + android + ios-sim share this target/)"
  if command -v cargo >/dev/null 2>&1; then
    (cd "$REPO_ROOT" && cargo clean)
  else
    rm_dir "$CARGO_TARGET"
  fi
}

clean_dir_list() {
  local name="$1"
  shift
  local dirs=("$@")
  echo "==> $name"
  for d in "${dirs[@]}"; do
    rm_dir "$d"
  done
}

echo "Will clean:"
if [ "$DO_CARGO" -eq 1 ]; then
  printf "  %s\n" "cargo target/ (workspace)"
fi
if [ "$DO_ANDROID" -eq 1 ]; then
  printf "  %s\n" "android gradle build dirs"
fi
if [ "$DO_IOS" -eq 1 ]; then
  printf "  %s\n" "ios xcode + swift staging dirs"
fi
if [ "$DO_WEB" -eq 1 ]; then
  printf "  %s\n" "web dist/"
fi

if [ "$ASSUME_YES" -eq 0 ]; then
  read -r -p "Proceed? [y/N] " ans
  case "$ans" in
    y|Y) : ;;
    *)
      echo "Aborted."
      exit 1
      ;;
  esac
fi

echo
if [ "$DO_CARGO" -eq 1 ]; then
  clean_cargo
fi
if [ "$DO_ANDROID" -eq 1 ]; then
  clean_dir_list "Android gradle build" "${ANDROID_DIRS[@]}"
fi
if [ "$DO_IOS" -eq 1 ]; then
  clean_dir_list "iOS xcode + Swift staging" "${IOS_DIRS[@]}"
fi
if [ "$DO_WEB" -eq 1 ]; then
  clean_dir_list "Web frontend" "${WEB_DIRS[@]}"
fi

echo
echo "Done."
