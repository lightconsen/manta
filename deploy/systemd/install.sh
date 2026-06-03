#!/bin/bash
# Syscity Systemd Service Installation Script
# Run as root or with sudo

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SYSCITY_USER="syscity"
SYSCITY_GROUP="syscity"
SYSCITY_HOME="/var/lib/syscity"
SYSCITY_CONFIG="/etc/syscity"
BINARY_PATH="/usr/local/bin/syscity"

echo -e "${GREEN}Installing Syscity AI Assistant Systemd Service...${NC}"

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Please run as root or with sudo${NC}"
    exit 1
fi

# Check if syscity binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo -e "${YELLOW}Warning: Syscity binary not found at $BINARY_PATH${NC}"
    echo "Please build and install the binary first:"
    echo "  cargo build --release"
    echo "  sudo cp target/release/syscity $BINARY_PATH"
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Create user and group
echo -e "${GREEN}Creating user and group...${NC}"
if ! id "$SYSCITY_USER" &>/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SYSCITY_USER"
    echo "Created user: $SYSCITY_USER"
else
    echo "User $SYSCITY_USER already exists"
fi

# Create directories
echo -e "${GREEN}Creating directories...${NC}"
mkdir -p "$SYSCITY_HOME"
mkdir -p "$SYSCITY_CONFIG"
mkdir -p "$SYSCITY_CONFIG/skills"

# Set permissions
chown -R "$SYSCITY_USER:$SYSCITY_GROUP" "$SYSCITY_HOME"
chown -R "$SYSCITY_USER:$SYSCITY_GROUP" "$SYSCITY_CONFIG"
chmod 750 "$SYSCITY_HOME"
chmod 755 "$SYSCITY_CONFIG"

# Copy service file
echo -e "${GREEN}Installing systemd service...${NC}"
cp "$(dirname "$0")/syscity.service" /etc/systemd/system/

# Create environment file template
ENV_FILE="$SYSCITY_CONFIG/syscity.env"
if [ ! -f "$ENV_FILE" ]; then
    echo -e "${GREEN}Creating environment file template...${NC}"
    cat > "$ENV_FILE" << 'EOF'
# Syscity AI Assistant Environment Configuration
# Add your API keys and configuration here

# Required: LLM Provider
SYSCITY_BASE_URL=https://api.openai.com/v1
SYSCITY_API_KEY=your_api_key_here
SYSCITY_MODEL=gpt-4o-mini

# Optional: Anthropic API format
# SYSCITY_IS_ANTHROPIC=false

# Optional: Agent Configuration
# SYSCITY_AGENT_NAME=Syscity

# Optional: Security
# SYSCITY_ALLOW_SHELL=true
# SYSCITY_SANDBOXED=true
EOF
    chmod 600 "$ENV_FILE"
    chown "$SYSCITY_USER:$SYSCITY_GROUP" "$ENV_FILE"
    echo -e "${YELLOW}Please edit $ENV_FILE with your API keys${NC}"
fi

# Create config.yaml template
CONFIG_FILE="$SYSCITY_CONFIG/config.yaml"
if [ ! -f "$CONFIG_FILE" ]; then
    echo -e "${GREEN}Creating config.yaml template...${NC}"
    cat > "$CONFIG_FILE" << 'EOF'
# Syscity AI Assistant Configuration

provider:
  type: openai
  model: gpt-4o-mini
  temperature: 0.7

agent:
  name: Syscity
  system_prompt: |
    You are Syscity, a helpful AI assistant.
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
    chown "$SYSCITY_USER:$SYSCITY_GROUP" "$CONFIG_FILE"
fi

# Reload systemd
echo -e "${GREEN}Reloading systemd...${NC}"
systemctl daemon-reload

# Enable service
echo -e "${GREEN}Enabling syscity service...${NC}"
systemctl enable syscity.service

echo
echo -e "${GREEN}Installation complete!${NC}"
echo
echo "Next steps:"
echo "  1. Edit $ENV_FILE with your API keys"
echo "  2. Customize $CONFIG_FILE as needed"
echo "  3. Copy example skills: cp -r examples/skills/* $SYSCITY_CONFIG/skills/"
echo "  4. Start the service: sudo systemctl start syscity"
echo "  5. Check status: sudo systemctl status syscity"
echo "  6. View logs: sudo journalctl -u syscity -f"
echo
