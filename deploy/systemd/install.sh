#!/bin/bash
# Manta Systemd Service Installation Script
# Run as root or with sudo

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
MANTA_USER="manta"
MANTA_GROUP="manta"
MANTA_HOME="/var/lib/manta"
MANTA_CONFIG="/etc/manta"
BINARY_PATH="/usr/local/bin/manta"

echo -e "${GREEN}Installing Manta AI Assistant Systemd Service...${NC}"

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Please run as root or with sudo${NC}"
    exit 1
fi

# Check if manta binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo -e "${YELLOW}Warning: Manta binary not found at $BINARY_PATH${NC}"
    echo "Please build and install the binary first:"
    echo "  cargo build --release"
    echo "  sudo cp target/release/manta $BINARY_PATH"
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Create user and group
echo -e "${GREEN}Creating user and group...${NC}"
if ! id "$MANTA_USER" &>/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin "$MANTA_USER"
    echo "Created user: $MANTA_USER"
else
    echo "User $MANTA_USER already exists"
fi

# Create directories
echo -e "${GREEN}Creating directories...${NC}"
mkdir -p "$MANTA_HOME"
mkdir -p "$MANTA_CONFIG"
mkdir -p "$MANTA_CONFIG/skills"

# Set permissions
chown -R "$MANTA_USER:$MANTA_GROUP" "$MANTA_HOME"
chown -R "$MANTA_USER:$MANTA_GROUP" "$MANTA_CONFIG"
chmod 750 "$MANTA_HOME"
chmod 755 "$MANTA_CONFIG"

# Copy service file
echo -e "${GREEN}Installing systemd service...${NC}"
cp "$(dirname "$0")/manta.service" /etc/systemd/system/

# Create environment file template
ENV_FILE="$MANTA_CONFIG/manta.env"
if [ ! -f "$ENV_FILE" ]; then
    echo -e "${GREEN}Creating environment file template...${NC}"
    cat > "$ENV_FILE" << 'EOF'
# Manta AI Assistant Environment Configuration
# Add your API keys and configuration here

# Required: LLM Provider
MANTA_BASE_URL=https://api.openai.com/v1
MANTA_API_KEY=your_api_key_here
MANTA_MODEL=gpt-4o-mini

# Optional: Anthropic API format
# MANTA_IS_ANTHROPIC=false

# Optional: Agent Configuration
# MANTA_AGENT_NAME=Manta

# Optional: Security
# MANTA_ALLOW_SHELL=true
# MANTA_SANDBOXED=true
EOF
    chmod 600 "$ENV_FILE"
    chown "$MANTA_USER:$MANTA_GROUP" "$ENV_FILE"
    echo -e "${YELLOW}Please edit $ENV_FILE with your API keys${NC}"
fi

# Create config.yaml template
CONFIG_FILE="$MANTA_CONFIG/config.yaml"
if [ ! -f "$CONFIG_FILE" ]; then
    echo -e "${GREEN}Creating config.yaml template...${NC}"
    cat > "$CONFIG_FILE" << 'EOF'
# Manta AI Assistant Configuration

provider:
  type: openai
  model: gpt-4o-mini
  temperature: 0.7

agent:
  name: Manta
  system_prompt: |
    You are Manta, a helpful AI assistant.
    You have access to tools for file operations,
    web search, shell commands, and more.

features:
  skills: true
  cron: true
  memory: true

security:
  allow_shell: true
  sandboxed: true
  max_budget: 50
EOF
    chmod 644 "$CONFIG_FILE"
    chown "$MANTA_USER:$MANTA_GROUP" "$CONFIG_FILE"
fi

# Reload systemd
echo -e "${GREEN}Reloading systemd...${NC}"
systemctl daemon-reload

# Enable service
echo -e "${GREEN}Enabling manta service...${NC}"
systemctl enable manta.service

echo
echo -e "${GREEN}Installation complete!${NC}"
echo
echo "Next steps:"
echo "  1. Edit $ENV_FILE with your API keys"
echo "  2. Customize $CONFIG_FILE as needed"
echo "  3. Copy example skills: cp -r examples/skills/* $MANTA_CONFIG/skills/"
echo "  4. Start the service: sudo systemctl start manta"
echo "  5. Check status: sudo systemctl status manta"
echo "  6. View logs: sudo journalctl -u manta -f"
echo
