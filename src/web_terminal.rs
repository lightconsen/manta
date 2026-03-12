//! Web Terminal for Manta
//!
//! Provides a browser-based terminal interface for interacting with the AI assistant.
//! Uses WebSockets for real-time bidirectional communication.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Html,
    routing::get,
    Router,
};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};

use crate::agent::Agent;
use crate::channels::IncomingMessage;

/// Broadcast channel for cron output
static CRON_BROADCAST: RwLock<Option<broadcast::Sender<String>>> = RwLock::const_new(None);

/// Initialize the cron broadcast channel
pub async fn init_cron_broadcast() -> broadcast::Receiver<String> {
    let tx = {
        let guard = CRON_BROADCAST.read().await;
        if let Some(ref tx) = *guard {
            tx.clone()
        } else {
            drop(guard);
            let (tx, _rx) = broadcast::channel(100);
            let mut guard = CRON_BROADCAST.write().await;
            *guard = Some(tx.clone());
            tx
        }
    };
    tx.subscribe()
}

/// Broadcast a cron job output to all connected web clients
pub async fn broadcast_cron_output(output: &str) {
    let guard = CRON_BROADCAST.read().await;
    if let Some(ref tx) = *guard {
        let msg = serde_json::json!({
            "type": "cron",
            "content": output
        });
        let _ = tx.send(msg.to_string());
    }
}

/// Shared application state
#[derive(Clone)]
pub struct WebTerminalState {
    pub agent: Arc<Agent>,
}

/// Start the web terminal server
pub async fn start_web_terminal(agent: Arc<Agent>, port: u16) -> crate::Result<()> {
    // Initialize broadcast channel
    let _ = init_cron_broadcast().await;

    let state = WebTerminalState { agent };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("🌐 Web Terminal starting on http://{}", addr);
    println!("🌐 Open your browser and navigate to http://localhost:{}", port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// HTML page with terminal interface
async fn index_handler() -> Html<&'static str> {
    Html(TERMINAL_HTML)
}

/// WebSocket upgrade handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<WebTerminalState>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle WebSocket connection
async fn handle_socket(mut socket: WebSocket, state: WebTerminalState) {
    info!("New WebSocket connection established");

    // Subscribe to cron broadcasts
    let mut cron_rx = init_cron_broadcast().await;

    // Generate conversation ID for this session
    let conversation_id = uuid::Uuid::new_v4().to_string();

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "system",
        "content": "Connected to Manta AI Assistant. Type your message below."
    });
    if let Err(e) = socket.send(Message::Text(welcome.to_string())).await {
        error!("Failed to send welcome: {}", e);
        return;
    }

    // Main message processing loop with cron broadcast handling
    loop {
        tokio::select! {
            // Handle incoming WebSocket messages
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        info!("Received message: {}", text);

                        // Send typing indicator
                        let typing = serde_json::json!({
                            "type": "typing",
                            "content": true
                        });
                        if socket.send(Message::Text(typing.to_string())).await.is_err() {
                            break;
                        }

                        // Process message with agent
                        let incoming = IncomingMessage::new(
                            "user",
                            &conversation_id,
                            &text
                        );

                        match state.agent.process_message(incoming).await {
                            Ok(response) => {
                                let resp_json = serde_json::json!({
                                    "type": "message",
                                    "role": "assistant",
                                    "content": response.content
                                });
                                if socket.send(Message::Text(resp_json.to_string())).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Agent error: {}", e);
                                let error_json = serde_json::json!({
                                    "type": "error",
                                    "content": format!("Error: {}", e)
                                });
                                if socket.send(Message::Text(error_json.to_string())).await.is_err() {
                                    break;
                                }
                            }
                        }

                        // Send typing indicator off
                        let typing_off = serde_json::json!({
                            "type": "typing",
                            "content": false
                        });
                        if socket.send(Message::Text(typing_off.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        info!("WebSocket connection closed");
                        break;
                    }
                    _ => {}
                }
            }

            // Handle cron broadcasts
            Ok(cron_msg) = cron_rx.recv() => {
                if socket.send(Message::Text(cron_msg)).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// HTML/CSS/JS for the terminal interface
const TERMINAL_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Manta AI Terminal</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        body {
            background: #0d1117;
            color: #c9d1d9;
            font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
            font-size: 14px;
            line-height: 1.6;
            height: 100vh;
            display: flex;
            flex-direction: column;
        }

        .header {
            background: #161b22;
            padding: 12px 20px;
            border-bottom: 1px solid #30363d;
            display: flex;
            align-items: center;
            justify-content: space-between;
        }

        .header h1 {
            font-size: 16px;
            color: #58a6ff;
            display: flex;
            align-items: center;
            gap: 8px;
        }

        .status {
            display: flex;
            align-items: center;
            gap: 8px;
            font-size: 12px;
        }

        .status-dot {
            width: 8px;
            height: 8px;
            border-radius: 50%;
            background: #238636;
            animation: pulse 2s infinite;
        }

        .status-dot.disconnected {
            background: #da3633;
            animation: none;
        }

        @keyframes pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.5; }
        }

        .terminal {
            flex: 1;
            overflow-y: auto;
            padding: 20px;
            scroll-behavior: smooth;
        }

        .message {
            margin-bottom: 16px;
            display: flex;
            gap: 12px;
        }

        .message.user {
            flex-direction: row-reverse;
        }

        .message.cron {
            opacity: 0.8;
        }

        .avatar {
            width: 32px;
            height: 32px;
            border-radius: 6px;
            display: flex;
            align-items: center;
            justify-content: center;
            font-size: 14px;
            flex-shrink: 0;
        }

        .message.assistant .avatar {
            background: #1f6feb;
        }

        .message.user .avatar {
            background: #238636;
        }

        .message.system .avatar {
            background: #6e7681;
        }

        .message.cron .avatar {
            background: #8957e5;
        }

        .content {
            max-width: 80%;
            padding: 12px 16px;
            border-radius: 8px;
            word-wrap: break-word;
        }

        .message.assistant .content {
            background: #161b22;
            border: 1px solid #30363d;
        }

        .message.user .content {
            background: #1f6feb;
            color: #fff;
        }

        .message.system .content {
            background: transparent;
            color: #8b949e;
            font-style: italic;
            font-size: 12px;
        }

        .message.cron .content {
            background: #212136;
            border: 1px solid #8957e5;
            font-family: monospace;
            font-size: 13px;
        }

        .typing {
            display: flex;
            gap: 4px;
            padding: 12px 16px;
        }

        .typing span {
            width: 8px;
            height: 8px;
            background: #8b949e;
            border-radius: 50%;
            animation: bounce 1.4s ease-in-out infinite both;
        }

        .typing span:nth-child(1) { animation-delay: -0.32s; }
        .typing span:nth-child(2) { animation-delay: -0.16s; }

        @keyframes bounce {
            0%, 80%, 100% { transform: scale(0); }
            40% { transform: scale(1); }
        }

        .input-area {
            background: #161b22;
            padding: 16px 20px;
            border-top: 1px solid #30363d;
            display: flex;
            gap: 12px;
        }

        .input-wrapper {
            flex: 1;
            display: flex;
            align-items: center;
            background: #0d1117;
            border: 1px solid #30363d;
            border-radius: 8px;
            padding: 0 16px;
        }

        .input-wrapper:focus-within {
            border-color: #58a6ff;
        }

        .prompt {
            color: #58a6ff;
            margin-right: 8px;
            font-weight: bold;
        }

        #messageInput {
            flex: 1;
            background: transparent;
            border: none;
            color: #c9d1d9;
            font-family: inherit;
            font-size: 14px;
            padding: 12px 0;
            outline: none;
        }

        #sendButton {
            background: #238636;
            color: #fff;
            border: none;
            padding: 12px 24px;
            border-radius: 8px;
            cursor: pointer;
            font-size: 14px;
            font-weight: 600;
            transition: background 0.2s;
        }

        #sendButton:hover {
            background: #2ea043;
        }

        #sendButton:disabled {
            background: #6e7681;
            cursor: not-allowed;
        }

        .code-block {
            background: #0d1117;
            border: 1px solid #30363d;
            border-radius: 6px;
            padding: 12px;
            margin: 8px 0;
            overflow-x: auto;
        }

        .code-block code {
            color: #a5d6ff;
            font-family: inherit;
        }

        pre {
            margin: 0;
        }
    </style>
</head>
<body>
    <div class="header">
        <h1>🤖 Manta AI Terminal</h1>
        <div class="status">
            <span class="status-dot" id="statusDot"></span>
            <span id="statusText">Connecting...</span>
        </div>
    </div>

    <div class="terminal" id="terminal"></div>

    <div class="input-area">
        <div class="input-wrapper">
            <span class="prompt">💬</span>
            <input type="text" id="messageInput" placeholder="Type your message..." autocomplete="off">
        </div>
        <button id="sendButton">Send</button>
    </div>

    <script>
        const terminal = document.getElementById('terminal');
        const messageInput = document.getElementById('messageInput');
        const sendButton = document.getElementById('sendButton');
        const statusDot = document.getElementById('statusDot');
        const statusText = document.getElementById('statusText');

        let ws;
        let reconnectAttempts = 0;
        const maxReconnectAttempts = 5;

        function connect() {
            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            ws = new WebSocket(`${protocol}//${window.location.host}/ws`);

            ws.onopen = () => {
                console.log('WebSocket connected');
                statusDot.classList.remove('disconnected');
                statusText.textContent = 'Connected';
                sendButton.disabled = false;
                reconnectAttempts = 0;
            };

            ws.onclose = () => {
                console.log('WebSocket closed');
                statusDot.classList.add('disconnected');
                statusText.textContent = 'Disconnected';
                sendButton.disabled = true;

                if (reconnectAttempts < maxReconnectAttempts) {
                    reconnectAttempts++;
                    setTimeout(connect, 2000);
                }
            };

            ws.onerror = (err) => {
                console.error('WebSocket error:', err);
            };

            ws.onmessage = (event) => {
                const data = JSON.parse(event.data);
                handleMessage(data);
            };
        }

        function handleMessage(data) {
            switch (data.type) {
                case 'system':
                    addSystemMessage(data.content);
                    break;
                case 'message':
                    if (data.role === 'assistant') {
                        addAssistantMessage(data.content);
                    } else {
                        addUserMessage(data.content);
                    }
                    break;
                case 'cron':
                    addCronMessage(data.content);
                    break;
                case 'typing':
                    if (data.content) {
                        showTyping();
                    } else {
                        hideTyping();
                    }
                    break;
                case 'error':
                    addSystemMessage(data.content);
                    break;
            }
        }

        function addUserMessage(content) {
            const msg = document.createElement('div');
            msg.className = 'message user';
            msg.innerHTML = `
                <div class="avatar">👤</div>
                <div class="content">${escapeHtml(content)}</div>
            `;
            terminal.appendChild(msg);
            scrollToBottom();
        }

        function addAssistantMessage(content) {
            hideTyping();
            const msg = document.createElement('div');
            msg.className = 'message assistant';
            msg.innerHTML = `
                <div class="avatar">🤖</div>
                <div class="content">${formatContent(content)}</div>
            `;
            terminal.appendChild(msg);
            scrollToBottom();
        }

        function addSystemMessage(content) {
            const msg = document.createElement('div');
            msg.className = 'message system';
            msg.innerHTML = `
                <div class="avatar">ℹ️</div>
                <div class="content">${escapeHtml(content)}</div>
            `;
            terminal.appendChild(msg);
            scrollToBottom();
        }

        function addCronMessage(content) {
            const msg = document.createElement('div');
            msg.className = 'message cron';
            msg.innerHTML = `
                <div class="avatar">⏰</div>
                <div class="content">${escapeHtml(content)}</div>
            `;
            terminal.appendChild(msg);
            scrollToBottom();
        }

        let typingElement = null;

        function showTyping() {
            if (typingElement) return;
            typingElement = document.createElement('div');
            typingElement.className = 'message assistant';
            typingElement.innerHTML = `
                <div class="avatar">🤖</div>
                <div class="content typing">
                    <span></span>
                    <span></span>
                    <span></span>
                </div>
            `;
            terminal.appendChild(typingElement);
            scrollToBottom();
        }

        function hideTyping() {
            if (typingElement) {
                typingElement.remove();
                typingElement = null;
            }
        }

        function formatContent(content) {
            let formatted = escapeHtml(content);
            formatted = formatted.replace(/```(\w+)?\n([\s\S]*?)```/g, '<div class="code-block"><pre><code>$2</code></pre></div>');
            formatted = formatted.replace(/`([^`]+)`/g, '<code>$1</code>');
            formatted = formatted.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
            formatted = formatted.replace(/\*([^*]+)\*/g, '<em>$1</em>');
            formatted = formatted.replace(/\n/g, '<br>');
            return formatted;
        }

        function escapeHtml(text) {
            const div = document.createElement('div');
            div.textContent = text;
            return div.innerHTML;
        }

        function scrollToBottom() {
            terminal.scrollTop = terminal.scrollHeight;
        }

        function sendMessage() {
            const text = messageInput.value.trim();
            if (!text || !ws || ws.readyState !== WebSocket.OPEN) return;

            addUserMessage(text);
            ws.send(text);
            messageInput.value = '';
            messageInput.focus();
        }

        sendButton.addEventListener('click', sendMessage);
        messageInput.addEventListener('keypress', (e) => {
            if (e.key === 'Enter') {
                sendMessage();
            }
        });

        connect();
        messageInput.focus();
    </script>
</body>
</html>"##;
