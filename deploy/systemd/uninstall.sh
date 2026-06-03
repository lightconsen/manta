#!/bin/bash
# Syscity Systemd Service Uninstallation Script
# Run as root or with sudo

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}Uninstalling Syscity AI Assistant Systemd Service...${NC}"

if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Please run as root or with sudo${NC}"
    exit 1
fi

# Stop and disable service
if systemctl is-active --quiet syscity; then
    echo "Stopping syscity service..."
    systemctl stop syscity
fi

if systemctl is-enabled --quiet syscity 2>/dev/null; then
    echo "Disabling syscity service..."
    systemctl disable syscity
fi

# Remove service file
if [ -f /etc/systemd/system/syscity.service ]; then
    echo "Removing service file..."
    rm /etc/systemd/system/syscity.service
    systemctl daemon-reload
fi

echo -e "${GREEN}Service removed.${NC}"
echo
echo -e "${YELLOW}The following were NOT removed (manual cleanup required):${NC}"
echo "  - User 'syscity' (userdel syscity)"
echo "  - /var/lib/syscity (data directory)"
echo "  - /etc/syscity (config directory)"
echo "  - /usr/local/bin/syscity (binary)"
echo
echo "To remove everything, run:"
echo "  sudo userdel syscity"
echo "  sudo rm -rf /var/lib/syscity /etc/syscity"
echo "  sudo rm -f /usr/local/bin/syscity"
