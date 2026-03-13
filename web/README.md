# Manta Web Terminal

React-based web terminal interface for Manta AI Assistant.

## Structure

```
web/
├── package.json          # Dependencies and scripts
├── tsconfig.json         # TypeScript config
├── tsconfig.node.json    # TypeScript config for Vite
├── vite.config.ts        # Vite build configuration
├── index.html            # Entry HTML
├── README.md             # This file
└── src/
    ├── main.tsx          # React entry point
    ├── App.tsx           # Main application component
    ├── styles.css        # Global styles
    ├── types.ts          # TypeScript type definitions
    ├── components/       # React components
    │   ├── MantaLogo.tsx
    │   ├── Message.tsx
    │   ├── TypingIndicator.tsx
    │   ├── Header.tsx
    │   └── InputArea.tsx
    └── utils/            # Utility functions
        ├── websocket.ts
        └── format.ts
```

## Development

```bash
# Install dependencies
npm install

# Start development server
npm run dev

# Build for production (outputs to ../assets/web_terminal.html)
npm run build
```

## Build Output

The build process uses `vite-plugin-singlefile` to bundle everything into a single HTML file:
- **Output**: `../assets/web_terminal.html`
- **Format**: Single self-contained HTML file with inlined CSS and JavaScript
- **Usage**: Loaded by the Rust backend as the web terminal interface

## Features

- WebSocket connection for real-time communication
- Message types: user, assistant, system, cron
- Typing indicators
- Code block formatting
- Auto-reconnection on disconnect
- Responsive design
