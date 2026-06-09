#!/usr/bin/env bash
set -euo pipefail

# Syscity Desktop — Install to system
# Builds release binaries and installs to the platform-appropriate location.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUNDLE_DIR="${SCRIPT_DIR}/../desktop/target/release/bundle"

echo "Building Syscity Desktop release..."
"${SCRIPT_DIR}/desktop-build.sh"

# Detect platform and install
if [[ "$OSTYPE" == "darwin"* ]]; then
    APP_NAME="Syscity.app"
    SRC="${BUNDLE_DIR}/macos/${APP_NAME}"
    DEST="/Applications/${APP_NAME}"

    if [ -d "${DEST}" ]; then
        echo "Removing existing app at ${DEST}..."
        rm -rf "${DEST}"
    fi

    echo "Installing to /Applications..."
    cp -R "${SRC}" "${DEST}"
    echo "Installed. You can now launch Syscity from Launchpad or Spotlight."

elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
    # Try .deb first, then .AppImage
    DEB="${BUNDLE_DIR}/deb/*.deb"
    APPIMAGE="${BUNDLE_DIR}/appimage/*.AppImage"

    if compgen -G "${DEB}" > /dev/null; then
        echo "Installing .deb package..."
        sudo dpkg -i ${DEB}
    elif compgen -G "${APPIMAGE}" > /dev/null; then
        DEST_DIR="${HOME}/Applications"
        mkdir -p "${DEST_DIR}"
        cp ${APPIMAGE} "${DEST_DIR}/"
        chmod +x "${DEST_DIR}"/*.AppImage
        echo "AppImage copied to ${DEST_DIR}/"
    else
        echo "No installable bundle found in ${BUNDLE_DIR}"
        exit 1
    fi

elif [[ "$OSTYPE" == "msys" || "$OSTYPE" == "cygwin" || "$OSTYPE" == "win32" ]]; then
    MSI="${BUNDLE_DIR}/msi/*.msi"
    if compgen -G "${MSI}" > /dev/null; then
        echo "MSI installer found: ${MSI}"
        echo "Please run the MSI installer manually."
    else
        echo "No MSI bundle found in ${BUNDLE_DIR}"
        exit 1
    fi
else
    echo "Unsupported platform: ${OSTYPE}"
    echo "Bundles are available in: ${BUNDLE_DIR}"
    exit 1
fi
