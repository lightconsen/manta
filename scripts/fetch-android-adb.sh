#!/usr/bin/env bash
#
# Fetch a runnable `adb` client for the Syscity Android app (mobile-migration
# §4.5 "loopback-ADB self-pairing").
#
# On Android there is no adb *client* on the phone — only the adbd *server*
# (wireless debugging). To self-pair over loopback the app needs a client
# binary bundled in the APK. Termux's android-tools ships one built against
# bionic (interpreter /system/bin/linker64), so it can exec from the app's
# nativeLibraryDir via AndroidShellRunner — but it is dynamically linked, so
# its runtime libraries must travel with it.
#
# This script downloads the Termux aarch64 packages, verifies every download
# against the sha256 recorded in Termux's signed package index (packages.termux.dev),
# and installs the adb client + its dependency closure into
#   desktop/gen/android/app/src/main/jniLibs/arm64-v8a/
#
# Nothing is installed unless every sha256 matches (all-or-nothing).
#
# Usage:
#   ./scripts/fetch-android-adb.sh            # fetch + verify + install (aarch64)
#   ./scripts/fetch-android-adb.sh --verify   # check the installed closure only
#   ./scripts/fetch-android-adb.sh --cache <dir>   # override the download cache
#
# Notes:
#   - aarch64/arm64-v8a only (the default APK ABI). x86_64 emulators are not
#     covered — on those the tools degrade gracefully (`has_adb()` is false).
#   - The bundle keeps the app's own libc++_shared.so (NDK build) untouched;
#     adb resolves `libc++_shared.so` from nativeLibraryDir at exec time.
#   - Re-run the script to refresh / self-heal. Deleting jniLibs/arm64-v8a/adb
#     reverts to "not available" without touching any other behaviour.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── pinned package manifest ────────────────────────────────────────────────
# name | repo path under packages.termux.dev/apt/termux-main | sha256
# (sha256 values come from the Termux stable repo Packages index, which is
#  cryptographically signed by the Termux maintainers.)
BASE_URL="https://packages.termux.dev/apt/termux-main"
PACKAGES=(
  "android-tools|pool/main/a/android-tools/android-tools_36.0.1+really35.0.2_aarch64.deb|82e48bf8038250fb0997b1f2cf5f780730104f2544a5532298c453d94cfe1537"
  "libprotobuf|pool/main/libp/libprotobuf/libprotobuf_2:35.1_aarch64.deb|a1ba7c7f0e5903a2134662653d3e7b9ffceaa78bdd00e07ac985e2d313ebc738"
  "abseil-cpp|pool/main/a/abseil-cpp/abseil-cpp_20260526.0_aarch64.deb|e489fac652cddc39d9436141e627285f1034a545a06fbb19c420514a419ad877"
  "zlib|pool/main/z/zlib/zlib_1.3.2_aarch64.deb|75e7d0af17fcc3b40004309fdc00a1ddb9ae08346dce5e269902c34ac3966ac9"
  "brotli|pool/main/b/brotli/brotli_1.2.0_aarch64.deb|db1502601d40fb44e6085ad8bfd9311a8b472e98db831ceec9d404c5708bb52c"
  "liblz4|pool/main/libl/liblz4/liblz4_1.10.0-1_aarch64.deb|09b9449418d5c2dc4f5c1c140ba8138d56be3e9ae5fd3be3318825ec9f8a0499"
  "zstd|pool/main/z/zstd/zstd_1.5.7-1_aarch64.deb|e1b4a5113648da8de189620ba1fce74c48b2d0833d9043391b9a1c91fb606fd3"
  "libc++|pool/main/libc/libc++/libc++_29_aarch64.deb|bb9f12113c137aa0e8513bb51cc49fe77a5ce3ca39ab9e92c57d228ecdf00222"
)

JNI_LIB_DIR="${SYSCITY_JNI_DIR:-$REPO_ROOT/desktop/gen/android/app/src/main/jniLibs/arm64-v8a}"

# Files to install, mapped from the extracted package layout. "->" rewrites a
# versioned file to an unversioned SONAME as a real copy (Android APK native
# lib extraction does not reliably preserve symlinks).
INSTALL_RULES=(
  "android-tools|data/data/com.termux/files/usr/bin/adb|adb"
  "libprotobuf|data/data/com.termux/files/usr/lib/libprotobuf.so|libprotobuf.so"
  "libprotobuf|data/data/com.termux/files/usr/lib/libutf8_validity.so|libutf8_validity.so"
  "zlib|data/data/com.termux/files/usr/lib/libz.so.1.3.2|libz.so.1"
  "brotli|data/data/com.termux/files/usr/lib/libbrotlicommon.so|libbrotlicommon.so"
  "brotli|data/data/com.termux/files/usr/lib/libbrotlidec.so|libbrotlidec.so"
  "brotli|data/data/com.termux/files/usr/lib/libbrotlienc.so|libbrotlienc.so"
  "liblz4|data/data/com.termux/files/usr/lib/liblz4.so|liblz4.so"
  "zstd|data/data/com.termux/files/usr/lib/libzstd.so.1.5.7|libzstd.so.1"
)
# NOTE: libc++_shared.so is intentionally NOT installed. The app already ships
# its own NDK build at jniLibs/arm64-v8a/libc++_shared.so; adb's NEEDED
# `libc++_shared.so` resolves to it from nativeLibraryDir at exec time. A
# renamed copy cannot satisfy the SONAME, so installing one would be dead
# weight (and clobbering the app's own copy risks breaking startup).

# libprotobuf's abseil dependency closure: every libabsl_*.so it links.
# Collected with llvm-readelf -d on libprotobuf.so (see fetch script header).
ABSEIL_DEPS=(
  "libabsl_base.so"
  "libabsl_cord.so"
  "libabsl_cord_internal.so"
  "libabsl_cordz_info.so"
  "libabsl_die_if_null.so"
  "libabsl_hash.so"
  "libabsl_log_internal_check_op.so"
  "libabsl_log_internal_conditions.so"
  "libabsl_log_internal_message.so"
  "libabsl_log_internal_nullguard.so"
  "libabsl_raw_hash_set.so"
  "libabsl_spinlock_wait.so"
  "libabsl_status.so"
  "libabsl_statusor.so"
  "libabsl_str_format_internal.so"
  "libabsl_strings.so"
  "libabsl_synchronization.so"
  "libabsl_time.so"
  "libabsl_time_zone.so"
)

# The libraries AndroidShellRunner must make discoverable via LD_LIBRARY_PATH
# when exec'ing the bundled adb (its RUNPATH points at a Termux path that does
# not exist on a stock device). The app's own libc++_shared.so is NOT copied;
# adb resolves it from nativeLibraryDir at exec time.
INSTALLED_LIBS=(
  "libprotobuf.so"
  "libutf8_validity.so"
  "libz.so.1"
  "libbrotlicommon.so"
  "libbrotlidec.so"
  "libbrotlienc.so"
  "liblz4.so"
  "libzstd.so.1"
)

help() {
  sed -n '2,32p' "$0"
}

verify_only() {
  local missing=()
  [[ -f "$JNI_LIB_DIR/adb" ]] || missing+=("adb")
  for lib in "${INSTALLED_LIBS[@]}"; do
    [[ -f "$JNI_LIB_DIR/$lib" ]] || missing+=("$lib")
  done
  for lib in "${ABSEIL_DEPS[@]}"; do
    [[ -f "$JNI_LIB_DIR/$lib" ]] || missing+=("$lib")
  done
  if [[ ${#missing[@]} -eq 0 ]]; then
    echo "OK: bundled adb + dependency closure present in $JNI_LIB_DIR"
    return 0
  fi
  echo "MISSING in $JNI_LIB_DIR:"
  printf '  %s\n' "${missing[@]}"
  return 1
}

# ── arg parsing ─────────────────────────────────────────────────────────────
MODE="install"
CACHE_DIR="${SYSCITY_ADB_CACHE:-${TMPDIR:-/tmp}/syscity-adb-cache}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --verify)
      MODE="verify"
      shift
      ;;
    --cache)
      CACHE_DIR="$2"
      shift 2
      ;;
    -h|--help)
      help
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      help >&2
      exit 1
      ;;
  esac
done

if [[ "$MODE" == "verify" ]]; then
  verify_only
  exit $?
fi

mkdir -p "$JNI_LIB_DIR" "$CACHE_DIR"

# ── download + verify (all-or-nothing) ──────────────────────────────────────
declare -A DEB_PATH=()
for entry in "${PACKAGES[@]}"; do
  IFS='|' read -r name rel want <<< "$entry"
  deb="$CACHE_DIR/$name.deb"
  DEB_PATH["$name"]="$deb"
  echo "fetching $name ..."
  if [[ ! -f "$deb" ]]; then
    curl -fsSL "$BASE_URL/$rel" -o "$deb"
  fi
  got="$(shasum -a 256 "$deb" | awk '{print $1}')"
  if [[ "$got" != "$want" ]]; then
    echo "ERROR: sha256 mismatch for $name" >&2
    echo "  expected: $want" >&2
    echo "  got:      $got" >&2
    echo "  Delete $deb and re-run to re-download." >&2
    exit 1
  fi
done
echo "all $(echo "${#PACKAGES[@]}") packages verified against the Termux signed index"

# ── stage extraction ────────────────────────────────────────────────────────
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

extract_deb() {
  local name="$1" dest="$2"
  local deb="${DEB_PATH[$name]}"
  # A .deb is an ar archive: unpack the inner data.tar.xz (no dpkg needed).
  local ar_dir="$dest/_ar"
  mkdir -p "$ar_dir"
  tar -xf "$deb" -C "$ar_dir"
  tar -xJf "$ar_dir/data.tar.xz" -C "$dest"
  rm -rf "$ar_dir"
}

install_file() {
  local src="$1" dst="$2"
  mkdir -p "$(dirname "$dst")"
  cp -f "$src" "$dst"
}

for entry in "${INSTALL_RULES[@]}"; do
  IFS='|' read -r pkg rel dst <<< "$entry"
  pkg_dir="$STAGE/$pkg"
  [[ -d "$pkg_dir" ]] || extract_deb "$pkg" "$pkg_dir"
  install_file "$pkg_dir/$rel" "$JNI_LIB_DIR/$dst"
done

# abseil closure
absl_dir="$STAGE/abseil-cpp"
[[ -d "$absl_dir" ]] || extract_deb "abseil-cpp" "$absl_dir"
for lib in "${ABSEIL_DEPS[@]}"; do
  install_file "$absl_dir/data/data/com.termux/files/usr/lib/$lib" "$JNI_LIB_DIR/$lib"
done

# ── provenance report ───────────────────────────────────────────────────────
echo ""
echo "Installed into $JNI_LIB_DIR:"
echo "  adb ($(du -h "$JNI_LIB_DIR/adb" | awk '{print $1}')) — android-tools 36.0.1, sha 82e48bf8038250fb…"
echo "  libprotobuf.so + libutf8_validity.so — libprotobuf 2:35.1"
echo "  $(find "$JNI_LIB_DIR" -maxdepth 1 -name 'libabsl_*.so' | wc -l | tr -d ' ') abseil libs — abseil-cpp 20260526.0 (libprotobuf closure)"
printf '  %s\n' "${INSTALLED_LIBS[@]}"
echo ""
echo "Provenance: every package sha256 is pinned from the Termux stable repo"
echo "Packages index (packages.termux.dev), which is signed by the maintainers."
echo ""
echo "Next: the app discovers adb via has_adb() and exec's it with"
echo "LD_LIBRARY_PATH=nativeLibraryDir (AndroidShellRunner). If adb does not"
echo "run on a given device, the 4.5 tools simply stay hidden (graceful"
echo "degradation) — no other behaviour is affected."
