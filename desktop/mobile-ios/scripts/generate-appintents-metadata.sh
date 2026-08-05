#!/usr/bin/env bash
# Post-build hook for the iOS app: emit Metadata.appintents so the
# Shortcuts/AppIntents bus (mobile-migration §4.6) is discoverable by linkd.
#
# WHY: the swift-rs build compiles AppIntents.swift into libapp.a WITHOUT
# Xcode's "Extract App Intents Metadata" build phase, so Xcode never emits
# Metadata.appintents for this app. Without it, linkd logs
# "net.syscity.desktop is not link enabled" and the intents never surface in
# the Shortcuts app (verified on the simulator: absent bundle -> not link
# enabled -> no discovery). This script reproduces that phase:
#   1. recompile AppIntents.swift with -emit-const-values (Xcode 16 requires
#      compile-time extraction; swiftc emits AppIntents.swiftconstvalues), and
#   2. run appintentsmetadataprocessor against the built .app.
#
# Wired as a postBuildScript on the iOS target in gen/apple/project.yml. Runs
# after Xcode links the .app; reads Xcode build-settings env vars. Also runs
# standalone when those vars are supplied (TARGET_BUILD_DIR, WRAPPER_NAME,
# EXECUTABLE_NAME, SDKROOT, IPHONEOS_DEPLOYMENT_TARGET, ARCHS, PLATFORM_NAME,
# CODE_SIGNING_ALLOWED, plus optional CONFIGURATION).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPINTENTS_SWIFT="$SCRIPT_DIR/../Sources/SyscityDevice/AppIntents.swift"
CONST_PROTOCOLS="$SCRIPT_DIR/const_protocols.json"
MODULE_NAME="${APPINTENTS_MODULE_NAME:-syscity_device}"
BUNDLE_ID="${APPINTENTS_BUNDLE_ID:-net.syscity.desktop}"

APP_DIR="${TARGET_BUILD_DIR:?TARGET_BUILD_DIR not set}"
WRAPPER="${WRAPPER_NAME:?WRAPPER_NAME not set}"
APP="$APP_DIR/$WRAPPER"
EXE="${EXECUTABLE_NAME:?EXECUTABLE_NAME not set}"

SDKROOT="${SDKROOT:?SDKROOT not set}"
DEVELOPER_DIR="${DEVELOPER_DIR:-$(xcode-select -p)}"
TOOLCHAIN="$DEVELOPER_DIR/Toolchains/XcodeDefault.xctoolchain"
PROCESSOR="$TOOLCHAIN/usr/bin/appintentsmetadataprocessor"
XCODE_VERSION="${XCODE_VERSION_BUILD:-$(xcodebuild -version 2>/dev/null | awk '/Build version/{print $3}')}"
XCODE_VERSION="${XCODE_VERSION:-16B40}"

DEPLOY_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:?IPHONEOS_DEPLOYMENT_TARGET not set}"
ARCH="${ARCHS%% *}"
if [ "${PLATFORM_NAME:-}" = "iphonesimulator" ]; then
  TRIPLE="${ARCH}-apple-ios${DEPLOY_TARGET}-simulator"
else
  TRIPLE="${ARCH}-apple-ios${DEPLOY_TARGET}"
fi

SCRATCH="${TMPDIR:-/tmp}/syscity-appintents-${CONFIGURATION:-debug}"
rm -rf "$SCRATCH"
mkdir -p "$SCRATCH"

log() { echo "[appintents-metadata] $*"; }

# 1. Recompile the intent sources with -emit-const-values so swiftc emits
#    AppIntents.swiftconstvalues into $SCRATCH (compile-time extraction).
log "compiling const-values for $APPINTENTS_SWIFT"
(
  cd "$SCRATCH"
  xcrun swiftc \
    -sdk "$SDKROOT" \
    -target "$TRIPLE" \
    -parse-as-library \
    -module-name "$MODULE_NAME" \
    -c \
    -emit-const-values \
    -Xfrontend -const-gather-protocols-file -Xfrontend "$CONST_PROTOCOLS" \
    -o "$SCRATCH/AppIntents.o" \
    "$APPINTENTS_SWIFT"
)

if [ ! -f "$SCRATCH/AppIntents.swiftconstvalues" ]; then
  log "ERROR: -emit-const-values produced no .swiftconstvalues"
  exit 1
fi

printf '%s\n' "$APPINTENTS_SWIFT" > "$SCRATCH/sources.txt"
printf '%s\n' "$SCRATCH/AppIntents.swiftconstvalues" > "$SCRATCH/constvals.txt"

# 2. Emit Metadata.appintents into the .app.
log "running appintentsmetadataprocessor (bundle $BUNDLE_ID, $TRIPLE)"
"$PROCESSOR" \
  --output "$APP" \
  --toolchain-dir "$TOOLCHAIN" \
  --module-name "$MODULE_NAME" \
  --sdk-root "$SDKROOT" \
  --xcode-version "$XCODE_VERSION" \
  --platform-family iOS \
  --deployment-target "$DEPLOY_TARGET" \
  --bundle-identifier "$BUNDLE_ID" \
  --target-triple "$TRIPLE" \
  --binary-file "$APP/$EXE" \
  --source-file-list "$SCRATCH/sources.txt" \
  --swift-const-vals-list "$SCRATCH/constvals.txt" \
  --compile-time-extraction --deployment-aware-processing --validate-assistant-intents

# 3. Injecting Metadata.appintents invalidates the code signature (verified:
#    linkd can still read it, but a broken signature makes the app crash on
#    launch). Re-sign: with the build's identity when the build signs (Xcode's
#    own Code Sign phase may re-sign anyway, but be safe), otherwise adhoc so
#    the simulator can launch the injected bundle.
if [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ] && [ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" ]; then
  log "re-signing $APP with $EXPANDED_CODE_SIGN_IDENTITY"
  /usr/bin/codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" \
    --timestamp=none --preserve-metadata=identifier,entitlements,flags "$APP"
else
  log "adhoc re-signing $APP"
  /usr/bin/codesign --force --sign - --deep "$APP"
fi

log "done: $APP/Metadata.appintents"
