# Manta Deployment

This directory contains deployment configurations for Manta AI Assistant.

## Quick Start

### Systemd Service (Recommended)

```bash
# Install
cd systemd
sudo ./install.sh

# Configure API keys
sudo nano /etc/manta/manta.env

# Start service
sudo systemctl start manta
sudo systemctl enable manta

# View logs
sudo journalctl -u manta -f
```

## Systemd Configuration

### Files

- `/etc/systemd/system/manta.service` - Service definition
- `/etc/manta/manta.env` - Environment variables
- `/etc/manta/config.yaml` - Main configuration
- `/var/lib/manta/` - Data directory

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `MANTA_BASE_URL` | Yes | - | LLM API endpoint |
| `MANTA_API_KEY` | Yes | - | API key for LLM |
| `MANTA_MODEL` | No | gpt-4o-mini | Model name |
| `MANTA_IS_ANTHROPIC` | No | false | Use Anthropic format |
| `MANTA_AGENT_NAME` | No | Manta | Assistant name |
| `MANTA_ALLOW_SHELL` | No | true | Allow shell commands |
| `MANTA_SANDBOXED` | No | true | Enable sandboxing |

### Security Features

- Runs as unprivileged `manta` user
- Filesystem sandboxing (`ProtectSystem=strict`)
- No new privileges
- Resource limits
- Capability dropping

## Reverse Proxy

### Nginx

```nginx
server {
    listen 80;
    server_name manta.example.com;

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
manta.example.com {
    reverse_proxy localhost:8080
}
```

## Health Checks

- Systemd: Service restart on failure
- HTTP health endpoint: `http://localhost:8080/health`

## Troubleshooting

### Check service status

```bash
sudo systemctl status manta
sudo journalctl -u manta -n 100
```

### Verify configuration

```bash
sudo -u manta manta config validate
```

### Reset data

```bash
sudo systemctl stop manta
sudo rm -rf /var/lib/manta/*
sudo systemctl start manta
```
