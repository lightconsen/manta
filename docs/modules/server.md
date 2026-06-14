# Server Module

HTTP server for Syscity providing REST API endpoints and WebSocket support.

## Design

- **`AppState`** — Shared application state with `Engine` and optional `Agent`
- **`ServerConfig`** — Host and port configuration
- **REST Endpoints** — HTTP API for chat, entities, and health checks
- **WebSocket** — Real-time bidirectional communication
- **Cron Broadcast** — Global broadcast channel for cron job output

### API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Root status |
| `/health` | GET | Health check |
| `/chat` | POST | Chat request |
| `/chat/stream` | GET | Streaming chat |
| `/entities` | POST | Create entity |
| `/entities/:id` | GET | Get entity |
| `/entities/:id` | POST | Update entity |
| `/webhooks` | GET | Webhook info |

### WebSocket

- Bidirectional message streaming
- Cron output broadcast
- Real-time agent output

## Key Types

```rust
pub struct AppState {
    pub engine: Arc<Engine>,
    pub agent: Option<Arc<Agent>>,
    pub cron_tx: broadcast::Sender<String>,
}

pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

pub struct ChatRequest {
    pub message: String,
    pub conversation_id: Option<String>,
}

pub struct ChatResponse {
    pub response: String,
    pub conversation_id: String,
}
```

## Data Flow

```
HTTP Request
    │
    ▼
Axum Router
    │
    ├──▶ /chat ──▶ Agent::process_message()
    ├──▶ /entities ──▶ Engine::create_entity()
    ├──▶ /health ──▶ Health check
    └──▶ /webhooks ──▶ Webhook handlers

WebSocket Connection
    │
    ▼
Real-time streaming
    │
    ├──▶ Agent output
    ├──▶ Cron broadcasts
    └──▶ Progress events
```

## Implemented Features

- Axum-based HTTP server with REST API
- WebSocket for real-time communication
- Chat endpoint with conversation tracking
- Entity CRUD endpoints
- Health check endpoint
- Webhook information endpoint
- Cron output broadcast channel
- Global broadcast with 100-message buffer
- Server configuration with host and port
- Integration with `Engine` and `Agent`

