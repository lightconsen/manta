#!/bin/bash
# Syscity One-Line Installer
# Usage: curl -sSL https://syscity.net/install.sh | bash

set -e

REPO="lightconsen/syscity"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
BINARY="syscity"

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$OS" in
    linux)
        case "$ARCH" in
            x86_64) TARGET="linux-amd64" ;;
            aarch64|arm64) TARGET="linux-arm64" ;;
            *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
        esac
        ;;
    darwin)
        case "$ARCH" in
            x86_64) TARGET="macos-amd64" ;;
            arm64) TARGET="macos-arm64" ;;
            *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
        esac
        ;;
    *)
        echo "Unsupported OS: $OS"
        exit 1
        ;;
esac

# Find latest release version
LATEST=$(curl -sSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
if [ -z "$LATEST" ]; then
    echo "Failed to detect latest version. Using v0.1.0"
    LATEST="v0.1.0"
fi

echo "Installing Syscity $LATEST for $TARGET..."

# Download
echo "Downloading..."
TMPDIR=$(mktemp -d)
curl -sSL "https://github.com/$REPO/releases/download/$LATEST/syscity-$TARGET.tar.gz" -o "$TMPDIR/syscity.tar.gz"

# Verify sha256 checksum (published alongside the tarball; matches `syscity update`)
if command -v sha256sum >/dev/null 2>&1; then
    SHA=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    SHA="shasum -a 256"
else
    SHA=""
fi
if [ -n "$SHA" ]; then
    CHECKSUM=$(curl -sSL "https://github.com/$REPO/releases/download/$LATEST/syscity-$TARGET.tar.gz.sha256" | awk '{print $1}')
    if [ -n "$CHECKSUM" ]; then
        ACTUAL=$($SHA "$TMPDIR/syscity.tar.gz" | awk '{print $1}')
        if [ "$ACTUAL" != "$CHECKSUM" ]; then
            echo "Checksum mismatch! Expected $CHECKSUM, got $ACTUAL."
            rm -rf "$TMPDIR"
            exit 1
        fi
        echo "Checksum verified."
    else
        echo "WARNING: no checksum published for $LATEST; skipping verification."
    fi
else
    echo "WARNING: no sha256 utility found; skipping checksum verification."
fi

# Extract
echo "Extracting..."
tar -xzf "$TMPDIR/syscity.tar.gz" -C "$TMPDIR"

# Install binary
echo "Installing to $INSTALL_DIR..."
if [ -w "$INSTALL_DIR" ]; then
    mv "$TMPDIR/$BINARY" "$INSTALL_DIR/"
else
    sudo mv "$TMPDIR/$BINARY" "$INSTALL_DIR/"
fi
chmod +x "$INSTALL_DIR/$BINARY"

# Create config directory
mkdir -p "$HOME/.syscity"

# Cleanup
rm -rf "$TMPDIR"

echo ""
echo "Syscity installed successfully!"
echo ""
echo "Next steps:"
echo "  1. Configure:   syscity setup"
echo "  2. Start:       syscity start"
echo "  3. Open Web UI: http://127.0.0.1:18080"
echo ""
echo "For more options: syscity --help"
