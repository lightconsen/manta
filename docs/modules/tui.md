# TUI Module

Interactive terminal UI client for Syscity, enabled by the `tui` Cargo feature.

## Design

Provides a full-screen terminal interface that connects to a running Syscity gateway over WebSocket. Built with `crossterm` for input handling and `ratatui` for rendering.

- **`run()`** — Entry point that initializes the terminal and starts the app
- **`AppState`** — Central mutable state (messages, sessions, input, popups, toasts)
- **`WsClient`** — Async WebSocket client with request/response framing
- **`EventLoop`** — Merges keyboard input, network messages, and render ticks
- **`Commands`** — Slash command parser and executor
- **`UI`** — Ratatui rendering of chat panels, sidebars, popups, and input bar

### Slash Commands

| Command | Description |
|---------|-------------|
| `/new` | Create a new session |
| `/clear` | Clear the message panel |
| `/status` | Query gateway presence |
| `/tools` | List available commands |
| `/model <id>` | Set the default model |
| `/help` | Open help popup with command list |
| `/config` | Open config editor popup |
| `/sessions` | List and refresh sessions |
| `/quit` / `/exit` | Exit the TUI |

## Key Types

```rust
pub async fn run(host: &str, port: u16, token: Option<&str>, session: Option<&str>) -> Result<()>

pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected { features: Vec<String>, scopes_granted: Vec<String>, server_version: String },
    Error(String),
}

pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub thinking: Option<String>,
    pub tool_name: Option<String>,
    pub status: MessageStatus,
    pub timestamp: DateTime<Local>,
    pub metadata: Option<Value>,
}

pub struct AppState {
    pub connection: ConnectionState,
    pub terminal_size: (u16, u16),
    pub current_session: Option<String>,
    pub sessions: Vec<SessionSummary>,
    pub messages: Vec<ChatMessage>,
    pub input_buffer: String,
    pub input_mode: InputMode,
    pub popup: Popup,
    pub scroll_offset: usize,
    pub toasts: Vec<Toast>,
    pub config_cache: HashMap<String, Value>,
    pub config_edits: HashMap<String, String>,
    pub command_list: Vec<CommandInfo>,
    pub is_running: bool,
    pub should_quit: bool,
}

pub enum WsMessage {
    Event(ClientEvent),
    OrphanResponse(ClientResponse),
}

pub struct WsClient {
    event_rx: mpsc::UnboundedReceiver<WsMessage>,
    write_tx: mpsc::UnboundedSender<Message>,
    pending: PendingMap,
    _stop_tx: mpsc::Sender<()>,
}

pub enum AuthConfig {
    None,
    Token { token: String },
}
```

## Data Flow

```
Terminal Startup
    │
    ▼
setup_terminal() → raw mode + alternate screen + mouse capture
    │
    ▼
run_app()
    │
    ├──▶ AuthConfig::ws_url() → WebSocket URL with token/session query params
    │
    ├──▶ WsClient::connect() → handshake (connect → hello-ok)
    │
    ├──▶ spawn_input_reader() → crossterm EventStream → TuiAction
    │
    └──▶ event_loop::run()
            │
            ├──▶ Input action → handle_action() → slash command or chat.send
            ├──▶ Network event → handle_event()
            │       ├──▶ chat.delta → append streaming content
            │       ├──▶ agent.thinking → append reasoning text
            │       ├──▶ tool.calling / tool.result → system messages
            │       ├──▶ chat.final → finalize assistant message
            │       ├──▶ chat.error → error toast
            │       ├──▶ session.created / session.renamed → update sidebar
            │       └──▶ approval.required → toast notification
            │
            └──▶ Render tick → ui::render() → ratatui draw
```

## Implemented Features

- Full-screen terminal UI with crossterm and ratatui
- WebSocket connection to gateway with protocol handshake
- Token-based and no-auth connection modes
- Real-time chat with streaming assistant responses
- Session sidebar with create, select, delete, and list
- Slash commands for session management, model switching, and gateway queries
- Help popup with dynamically fetched command list
- Config editor popup with live get/set via gateway API
- Toast notifications (success and error) with TTL expiration
- Tool call and approval event rendering
- Panic hook that restores terminal before printing
- Graceful terminal cleanup on exit
- 30 FPS render loop (33ms interval)
- Scope-based feature gating from hello-ok handshake
- Message status tracking (Sending, Streaming, Complete, Error)
- Delta-based streaming message append with finalize
- Keyboard input handling (typing, backspace, scroll, focus, popup navigation)
- Unit tests for command parsing, state management, and WebSocket framing

