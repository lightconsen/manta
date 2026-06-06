# Syscity Chat UI (assistant-ui)

Web frontend for Syscity, built with [assistant-ui](https://assistant-ui.com) and the WebSocket-native protocol.

## Build

Requires Node.js 18+.

```bash
cd assets/chat-ui
npm install
npm run build
```

The build output goes to `assets/chat/` which the Rust server serves automatically.

If `assets/chat/index.html` does not exist, the server falls back to `assets/chat.html` (standalone version).

## Architecture

- `src/SyscityWebSocketTransport.ts` — Custom `ChatModelAdapter` that speaks the Syscity WebSocket protocol (`connect` handshake → `chat.send` → `chat.delta`/`chat.final` events).
- `src/App.tsx` — React app wrapping `AssistantRuntimeProvider` + `Thread`.
