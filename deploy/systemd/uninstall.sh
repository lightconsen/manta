#!/bin/bash
# Manta Systemd Service Uninstallation Script
# Run as root or with sudo

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}Uninstalling Manta AI Assistant Systemd Service...${NC}"

if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Please run as root or with sudo${NC}"
    exit 1
fi

# Stop and disable service
if systemctl is-active --quiet manta; then
    echo "Stopping manta service..."
    systemctl stop manta
fi

if systemctl is-enabled --quiet manta 2>/dev/null; then
    echo "Disabling manta service..."
    systemctl disable manta
fi

# Remove service file
if [ -f /etc/systemd/system/manta.service ]; then
    echo "Removing service file..."
    rm /etc/systemd/system/manta.service
    systemctl daemon-reload
fi

echo -e "${GREEN}Service removed.${NC}"
echo
echo -e "${YELLOW}The following were NOT removed (manual cleanup required):${NC}"
echo "  - User 'manta' (userdel manta)"
echo "  - /var/lib/manta (data directory)"
echo "  - /etc/manta (config directory)"
echo "  - /usr/local/bin/manta (binary)"
echo
echo "To remove everything, run:"
echo "  sudo userdel manta"
echo "  sudo rm -rf /var/lib/manta /etc/manta"
echo "  sudo rm -f /usr/local/bin/manta"
