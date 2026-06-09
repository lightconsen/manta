# Build from Source

## Prerequisites

- Rust 1.75+
- Node.js 22 (for the web UI)

## Build

```bash
git clone https://github.com/lightconsen/syscity.git
cd syscity
./scripts/build.sh
```

## Desktop App

```bash
./scripts/build-desktop.sh
```

Runs as a menu-bar app with the web UI embedded. macOS deployment target: 10.15+.
