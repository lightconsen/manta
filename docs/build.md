# Build from Source

## Prerequisites

| Requirement | Version | Notes |
|-------------|---------|-------|
| Rust | 1.75+ (MSRV) | Install via [rustup](https://rustup.rs) |
| Node.js | 22 | Only needed to build the web UI |
| pnpm | latest | `npm i -g pnpm` — used by the web build |

### Platform-specific system dependencies

**Linux (Debian/Ubuntu):**
```bash
sudo apt-get install -y build-essential pkg-config libssl-dev
# Optional, for the `serialport`/`hidapi` device features:
sudo apt-get install -y libudev-dev libusb-1.0-0-dev
```

**macOS:**
```bash
xcode-select --install   # Command Line Tools (clang, headers)
```
Desktop control additionally requires granting **Screen Recording** and
**Accessibility** permissions in *System Settings → Privacy & Security*.

**Windows:** Install the [Visual Studio C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
(MSVC toolchain). Desktop control and some device features are macOS-first;
core agent / channel functionality works cross-platform.

## Build

```bash
git clone https://github.com/lightconsen/syscity.git
cd syscity
./scripts/build.sh
```

`build.sh` builds the web frontend (`web/` → `dist/`) then compiles the release
binary. To build only the frontend: `./scripts/build.sh --front`.

To build just the Rust binary without the web bundle:
```bash
cargo build --release
# binary at ./target/release/syscity
```

## Feature flags

Syscity uses Cargo features to gate optional functionality. The defaults enable
all channels plus browser, vision, local embeddings, and device I/O.

| Feature | Default | Enables |
|---------|:-------:|---------|
| `telegram` / `discord` / `slack` / `whatsapp` / `qq` / `feishu` / `signal` / `imessage` / `webchat` | ✓ | Individual messaging channels |
| `all-channels` | — | All of the above at once |
| `browser` | ✓ | Browser automation (requires Chrome/Chromium installed) |
| `vision` | ✓ | ONNX-based screen vision (`ort`/`image`/`ndarray`) |
| `local-embeddings` | ✓ | On-device embeddings via `llama-cpp-2` + `hf-hub` |
| `local-summarizer` | — | On-device perception summarizer |
| `plugins` | ✓ | WASM plugin sandbox (`wasmtime`) |
| `native-plugins` | — | Native `.so`/`.dylib` plugin loading (`libloading`) |
| `hot-reload` | ✓ | Config/plugin hot-reload via file watching |
| `serialport` / `hidapi` / `gpio` | ✓ / ✓ / — | Hardware device drivers |
| `tailscale` | — | Tailscale serve / funnel networking |
| `vector-db` / `pgvector` / `sqlite-vec` | — | Vector memory backends |

Build with a custom feature set:
```bash
# Minimal headless build, no channels or browser
cargo build --release --no-default-features --features webchat,plugins

# Everything
cargo build --release --features all-channels,tailscale,vector-db
```

## Desktop App

```bash
./scripts/desktop-build.sh
```

Runs as a menu-bar app with the web UI embedded. macOS deployment target: 10.15+.

## Verifying your build

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
./target/release/syscity --version
```

## Troubleshooting

- **`openssl`/`pkg-config` errors on Linux** — install `libssl-dev` and `pkg-config`.
- **`llama-cpp-2` fails to compile** — needs a C compiler; on Linux install
  `build-essential`. To skip it, build with `--no-default-features` and omit
  `local-embeddings`/`local-summarizer`.
- **Browser tools do nothing** — the `browser` feature requires a Chrome/Chromium
  binary on `PATH`.
- **`pnpm: command not found`** — install pnpm (`npm i -g pnpm`) or build the
  Rust binary alone with `cargo build --release`.
