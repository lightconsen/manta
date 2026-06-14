# Tailscale Module

Tailscale integration for secure remote access to the Syscity Gateway.

## Design

Provides built-in Tailscale Serve/Funnel support for secure remote access without complex network configuration.

- **`start()`** — Start Tailscale serve or funnel
- **`stop()`** — Stop Tailscale serve/funnel
- **`status()`** — Get Tailscale status

### Modes

| Mode | Use Case | Command |
|------|----------|---------|
| Serve | Tailnet-only access | `tailscale serve --http localhost:PORT` |
| Funnel | Public internet access | `tailscale funnel --http DOMAIN:PORT` |

### Configuration

```rust
pub struct TailscaleConfig {
    pub port: u16,
    pub domain: Option<String>,
    pub use_funnel: bool,
}
```

## Key Types

```rust
pub struct TailscaleConfig {
    pub port: u16,
    pub domain: Option<String>,
    pub use_funnel: bool,
}
```

## Data Flow

```
Gateway Startup
    │
    ├──▶ tailscale_enabled = true
    │       │
    │       ├──▶ tailscale_domain set → funnel mode
    │       └──▶ tailscale_domain unset → serve mode
    │
    └──▶ tailscale_enabled = false → skip
```

## Implemented Features

- Tailscale CLI detection and validation
- Serve mode for tailnet-only access
- Funnel mode for public internet access
- Graceful stop with serve off + funnel off
- Status retrieval
- Error handling for missing Tailscale CLI
- Integration with Gateway configuration

