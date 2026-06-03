# Syscity Deployment

This directory contains deployment configurations for Syscity AI Assistant.

## Quick Start

### Systemd Service (Recommended)

```bash
# Install
cd systemd
sudo ./install.sh

# Configure API keys
sudo nano /etc/syscity/syscity.env

# Start service
sudo systemctl start syscity
sudo systemctl enable syscity

# View logs
sudo journalctl -u syscity -f
```

## Systemd Configuration

### Files

- `/etc/systemd/system/syscity.service` - Service definition
- `/etc/syscity/syscity.env` - Environment variables
- `/etc/syscity/config.yaml` - Main configuration
- `/var/lib/syscity/` - Data directory

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `SYSCITY_BASE_URL` | Yes | - | LLM API endpoint |
| `SYSCITY_API_KEY` | Yes | - | API key for LLM |
| `SYSCITY_MODEL` | No | gpt-4o-mini | Model name |
| `SYSCITY_IS_ANTHROPIC` | No | false | Use Anthropic format |
| `SYSCITY_AGENT_NAME` | No | Syscity | Assistant name |
| `SYSCITY_ALLOW_SHELL` | No | true | Allow shell commands |
| `SYSCITY_SANDBOXED` | No | true | Enable sandboxing |

### Security Features

- Runs as unprivileged `syscity` user
- Filesystem sandboxing (`ProtectSystem=strict`)
- No new privileges
- Resource limits
- Capability dropping

## Reverse Proxy

### Nginx

```nginx
server {
    listen 80;
    server_name syscity.net;

    location / {
        proxy_pass http://localhost:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_cache_bypass $http_upgrade;
    }
}
```

### Caddy

```
syscity.net {
    reverse_proxy localhost:8080
}
```

## Health Checks

- Systemd: Service restart on failure
- HTTP health endpoint: `http://localhost:8080/health`

## Troubleshooting

### Check service status

```bash
sudo systemctl status syscity
sudo journalctl -u syscity -n 100
```

### Verify configuration

```bash
sudo -u syscity syscity config validate
```

### Reset data

```bash
sudo systemctl stop syscity
sudo rm -rf /var/lib/syscity/*
sudo systemctl start syscity
```
