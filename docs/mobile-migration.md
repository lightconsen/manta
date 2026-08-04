# Syscity Mobile Migration Plan

Status: **Proposal** · Target: Android first, iOS second · Owner: TBD

This document is the migration plan for running **Syscity** standalone on a
mobile device — the full runtime on-device: gateway, agents, delegation, memory,
SQLite. It covers a feasibility assessment of the current codebase, the platform
constraints that shape the design, the adaptation layers required, and a phased
roadmap with acceptance criteria.

---

## 1. Goal

Run **Syscity** standalone on a mobile device: gateway, agents, delegation,
memory, and SQLite all on the phone. Strategy: **Android first, iOS second**.

- **Effort:** the phased roadmap in §5.
- **Feasibility:** possible, with a scoped-down tool surface. The agent "brain"
  (LLM API calls) is already remote over standard HTTPS, which is the core
  value and is fully portable.
- **Key asymmetry:** Android can spawn same-UID subprocesses; **iOS cannot
  fork/exec at all**. Standalone iOS is limited to in-process libraries (§4).

---

## 2. Feasibility: what the codebase gives us today

### 2.1 The core is mostly portable

The top-level `Cargo.toml` has **no GUI dependencies** (no GTK/X11/objc/tauri in
the core crate). Most of the stack is portable, but three dependencies need
mobile-specific handling (SQLite linking, `notify`, `keyring`) — see §2.2.

| Component | Dep | Mobile |
|---|---|---|
| HTTP/WS gateway | axum + tokio | ✅ portable |
| Concurrency | tokio (tasks/threads) | ✅ portable |
| Storage | sqlx + SQLite | ⚠️ needs `sqlite/bundled` (links system libsqlite3 today) |
| Agent loop, context, tools | — | ✅ portable |
| Delegation tree | in-process `SubagentRegistry` | ✅ portable |
| LLM provider calls | HTTPS | ✅ (needs network + key) |
| Web UI | React SPA | ✅ (webview/PWA) |

The desktop shell is a separate crate (`desktop/`) that **embeds the gateway
in-process** via `use syscity::` and serves it to the Tauri WebView at
`http://127.0.0.1:<port>` (`desktop/src/lib.rs`). This embedding pattern is
exactly what Tauri mobile needs — it is the mobile path's starting point.

### 2.2 Hard blockers (must be addressed)

1. **`src/computer/` is desktop-only.**
   `src/computer/mod.rs:42-46` gates platform modules with
   `#[cfg(target_os = "linux"/"macos"/"windows")]` and depends on
   X11/Wayland/CoreGraphics/Win32. It must be cfg-excluded on mobile.
2. **`~/.syscity` base directory is hardcoded.**
   `src/dirs.rs:34` returns `home_dir()/.syscity` with no env override.
   Android has no `~`; every path in the app derives from this single point.
3. **Subprocess-spawning tools.**
   `shell.rs`, `code_exec.rs`, `process.rs`, `pdf.rs`, `image.rs`, `tts.rs`,
   `patch.rs`, `nodes.rs`, `src/security/tailscale.rs`, and
   `src/mcp/client.rs` (`connect_stdio`) all `Command::new` external binaries
   that do not exist on a phone (§4).
4. **Desktop-specific execution model assumptions.**
   `fs_watch`, cron background wake, and unrestricted filesystem access are
   desktop assumptions that Android/iOS restrict (§4).
5. **Dependency-level blockers not visible in source review.**
   - **SQLite is not bundled** — `libsqlite3-sys` links the system library
     (`cc`/`pkg-config`/`vcpkg` in `Cargo.lock`); Android/iOS have no linkable
     system SQLite. Enable `sqlite/bundled`.
   - **`notify` has no iOS backend** — backends are linux+android (inotify),
     macOS (FSEvents), BSD/kqueue, windows. It is used at six call sites
     (`config/hot_reload.rs`, `config/watch.rs`, `cli/kb.rs`,
     `rag/ingestion/watch.rs`, `skills/watcher.rs`, `computer/fs_watch.rs`),
     not just the `fs_watch` tool. iOS needs a polling fallback or
     desktop-gated exclusion.
   - **`keyring` (opt-in) is incompatible with mobile** — its `apple-native` /
     `windows-native` / `secret-service` backends do not cover iOS/Android.
     Mobile builds must not enable `--features keyring`; the default 0600
     encrypted file store works in the sandbox short-term, with Android
     Keystore / iOS Keychain as the long-term backend.

### 2.3 Existing mechanisms we reuse (do not reinvent)

- **`is_available()`** — `Tool` trait method (`src/tools/types.rs:769`),
  already implemented per tool (`shell.rs:330`, `delegate_tool.rs:775`,
  `gateway.rs:296`, `planner.rs:80`, `cron_tool.rs:392`, `acp_tool.rs:265`,
  `computer.rs:139`). This is the platform capability probe: add a
  `#[cfg(target_os = "...")]` branch and the agent loop automatically skips
  unavailable tools.
- **MCP transport already has three modes** — stdio, SSE, streamable-HTTP
  (`src/mcp/client.rs:3`). Standalone mobile switches MCP servers to HTTP, or
  uses an in-process channel (§5 P3).
- **Delegation is process-internal** — child agents are tokio tasks keyed by
  `delegation:<run_id>`, not subprocesses. The entire delegation tree
  (collaboration, handoff, recursion, shared `task_state`) works on mobile
  unchanged.
- **Unified SQLite** — `data/` already holds `syscity.db` + `delegations.db`.
  DB files live fine inside the app sandbox; no storage migration needed.
- **`SYSCITY_HOME` single choke point** — everything derives from `dirs.rs`,
  so one configurable base relocates the whole tree (see P1).
- **WASM runtime already in the tree** — `wasmtime` + `wasmtime-wasi` back the
  plugin system (`src/plugins`). The §4.5 embedded `code_exec` engine is not
  net-new, and wasmtime's Android/iOS build is an early cross-compile canary.

---

## 3. Platform constraints that shape the design

### 3.1 Android

| Capability | Constraint |
|---|---|
| Subprocess spawn | Allowed for same-UID binaries; spawn from app-private dir (`filesDir`/native-lib dir). Cannot run other apps' processes. |
| Shell | `/system/bin/sh` (mksh) + `toybox` (busybox-like) exist. Runs with the app's sandboxed UID; no root, no cross-app access. |
| Bundled binaries | Native `.so` in the APK can be exec'd; no runtime compilation/install. |
| Filesystem | App-private sandbox (`filesDir`/`cacheDir`); user files via Storage Access Framework (`content://`). |
| Background | Process killed under Doze/App-Standby; need `WorkManager`/`JobScheduler` for deferred work. |
| Watch | `inotify` works inside the app sandbox only. |

### 3.2 iOS

| Capability | Constraint |
|---|---|
| Subprocess spawn | **Prohibited.** The sandbox blocks `fork`/`exec` for app code. No shell, no bundled executable. |
| Filesystem | App sandbox only (`NSDocumentDirectory` etc.). |
| Background | Suspended in background; deferred work via `BGTaskScheduler`. |
| Consequence | **In-process libraries are the only execution path.** `shell`/`process`/`code_exec`-as-subprocess are impossible; embedded interpreters/WASM are the replacement. |

### 3.3 Concurrency is not the problem

Syscity's concurrency is already thread/task-based (tokio), not process-based.
Threads give parallelism and shared-memory messaging on every platform. What
threads **cannot** provide is:

- an interpreter that does not exist on the device (threads don't conjure
  `python`), and
- fault isolation (a `panic!` in a thread kills the whole app process — which
  on mobile is the UI too).

The mobile answer for isolation is the **WASM sandbox** (`wasmtime` memory/
time/fs limits), which is stricter than a desktop subprocess. Threads + WASM
together are the mobile replacement for subprocesses.

---

## 4. The adaptation layers

### 4.1 Configurable base directory (P1)

Add an env override in `src/dirs.rs`:

```
SYSCITY_HOME  (default: <home>/.syscity  — unchanged on desktop)
```

Every path function in `dirs.rs` already derives from `syscity_dir()`, so this
one change relocates `data/`, `logs/`, `skills/`, `agents/*/workspace/`,
`delegations/*/`, `artifacts/` together. The mobile host sets it at startup:
- Android: Kotlin `context.filesDir`
- iOS: `NSDocumentDirectory`

### 4.2 Platform-gate `src/computer/` (P1)

Extend the existing `#[cfg(target_os = ...)]` gates so the module compiles to a
no-op (or is excluded) for `android`/`ios`. Screen/mouse/keyboard control has no
mobile semantics; it is replaced by mobile-native capabilities in P4.

### 4.3 `ProcessRunner` abstraction (P3)

Collect the scattered `Command::new` sites in `src/tools/*` behind a trait:

```rust
trait ProcessRunner {
    fn run(&self, argv: &[&str]) -> Result<CommandOutput, Error>;
}
```

| Target | Implementation |
|---|---|
| Desktop | `std::process` (today's behavior) |
| Android | `sh`/`toybox` for the whitelisted set; bundled native binaries from app-private dir; error otherwise |
| iOS | No process execution; in-process library path or error |

### 4.4 `is_available()` platform branches (P3)

Add `#[cfg]`-based branches to the existing `is_available()` implementations so
the agent loop never offers a tool that cannot run:

| Tool | Android | iOS |
|---|---|---|
| `shell` | ✅ `sh`/toybox, sandboxed UID | ❌ |
| `process` (ps/kill) | ❌ (own process only) | ❌ |
| `code_exec` | WASM / embedded interpreter | WASM / embedded interpreter |
| `pdf`/`image`/`tts` | in-process Rust crates / Android TTS | in-process Rust crates / iOS TTS |
| `computer` (desktop control) | ❌ | ❌ |
| `mcp` stdio servers | in-process or HTTP | in-process or HTTP |
| `fs_watch` | sandbox-scoped `inotify` | sandbox-scoped or polling |

### 4.5 Execution engines (P3)

`code_exec` replaces subprocess interpreters with **in-process engines**,
selected by cargo feature so desktop keeps its subprocess behavior:

- JavaScript → `rquickjs` / `boa` (pure Rust, single-threaded VM; parallelize
  with a thread pool of instances)
- Python → `rustpython` (pure Rust)
- Untrusted/arbitrary → `wasmtime` + WASI (sandboxed memory/fs/time; already
  in the tree for the plugin system)

### 4.6 MCP transport (P3)

- Servers that support it → `streamable-http` (client already supports it).
- Pure-Rust MCP servers → compile into the app and connect over an in-process
  channel (`tokio::mpsc`) instead of stdio pipes.
- Node-based servers → not viable standalone; disable on mobile.

### 4.7 Background work (P4, optional)

Cron/heartbeat scheduling is in-process (`src/cron/cron/scheduler.rs`, a tokio
timer) and runs while the app is foregrounded. For wake-in-background:

- Android: `WorkManager`/`JobScheduler` via a Tauri/native plugin
- iOS: `BGTaskScheduler`

This is an enhancement, not a blocker — standalone mobile can ship with
foreground-only scheduling first.

### 4.8 User-file access (P4)

- Keep agent file operations inside the app sandbox (workspaces, delegations,
  artifacts) — no migration needed.
- User-facing files use the existing HTTP surface (`/api/v1/artifacts/*path`)
  plus an Android `content://` (SAF) adapter on `file_read`/`file_write` where
  real user-file access is required.

### 4.9 Dependency & feature exclusions (build-time)

Mobile builds must handle these dependencies explicitly:

| Dependency / feature | Why | Handling |
|---|---|---|
| `sqlite/bundled` | no linkable system SQLite on Android/iOS | enable the feature |
| `keyring` feature | backends cover macOS/Windows/Linux only | never enable on mobile; file store short-term, Keystore/Keychain later |
| `chromiumoxide` (optional) | browser automation, desktop-only | keep off |
| `tailscale` pairing | spawns `tailscale` CLI, reads `/var/run/tailscale/...` socket | cfg-gate off / mark unavailable |
| `notify` watchers (6 call sites) | no iOS backend | Android: inotify (sandbox-scoped); iOS: polling fallback or disabled |
| `nix` signal/kill paths | iOS support partial | cfg-gate `daemon.rs` + `process.rs`; verify all four in-tree nix versions compile |
| daemon mode | double-fork/setsid/PID, Unix desktop concept | mobile always runs `--foreground` |
| `dirs` crate | `home_dir()` unreliable on Android | always resolve via `SYSCITY_HOME` |

---

## 5. Phased roadmap

### Phase 1 — Cross-compilation proof (Android)

Goal: prove the core compiles and runs on Android.

| # | Task | File(s) |
|---|---|---|
| 1.1 | `SYSCITY_HOME` env override | `src/dirs.rs` |
| 1.2 | cfg-exclude `src/computer/` on mobile | `src/computer/mod.rs` |
| 1.3 | Rust `aarch64-linux-android` toolchain + build target wired | `Cargo.toml`, CI |
| 1.4 | Enable `sqlite/bundled` (no linkable system SQLite on mobile) | `Cargo.toml` |
| 1.5 | Headless smoke test: gateway starts, SQLite opens under sandbox path, HTTP responds | test harness |

**Acceptance:** `cargo build --target aarch64-linux-android` succeeds; the
binary starts on Android and serves `/api/v1/health`.

**Decision gate:** P1 acceptance requires the **dependency checklist** to pass
on `aarch64-linux-android` (and, for iOS later, `aarch64-apple-ios`):

- [ ] SQLite links via `sqlite/bundled`
- [ ] `notify` compiles (Android: inotify) or is cfg-excluded + polled (iOS)
- [ ] build without `--features keyring`
- [ ] `nix` (versions 0.26/0.27/0.28/0.29 in tree) compiles on the target
- [ ] `wasmtime` (plugin runtime) compiles on the target
- [ ] `src/computer/` excluded compiles clean

If any row fails, stop and reassess scope before proceeding.

### Phase 2 — Shell + UI on device

Goal: the core product (chat, delegation, memory) works on the phone.

| # | Task | File(s) |
|---|---|---|
| 2.1 | Tauri mobile init (android) or PWA/webview wrapper | `desktop/` |
| 2.2 | Web UI served by the embedded gateway in the app | `desktop/src/lib.rs` (already embeds gateway) |
| 2.3 | Set `SYSCITY_HOME` from `context.filesDir` at startup | mobile host |

**Acceptance:** installable Android app where the user can chat, spawn
delegations (orchestration / collaboration / handoff / recursion), and see
task_state + artifacts.

### Phase 3 — Execution-layer adaptation

Goal: bring the subprocess-dependent tools up (or down) correctly on mobile.

| # | Task | File(s) |
|---|---|---|
| 3.1 | `is_available()` platform branches | `src/tools/*.rs` |
| 3.2 | `ProcessRunner` abstraction + Android `sh`/bundled-binary impl | `src/tools/*` |
| 3.3 | `code_exec` → WASM / embedded engines behind feature flags | `src/tools/code_exec.rs`, `Cargo.toml` |
| 3.4 | MCP: HTTP transport for supported servers; in-process channel for Rust libs | `src/mcp/client.rs` |
| 3.5 | `fs_watch` sandbox-scoped / polling; process tools disabled | `src/computer/fs_watch.rs`, `src/tools/process.rs` |

**Acceptance:** the agent can execute code (WASM) and connect MCP servers
(HTTP/in-process) on-device; unavailable tools are invisible to the agent.

### Phase 4 — Mobile-native capabilities (differentiator)

Goal: turn "Syscity on a phone" into "a phone-native Syscity".

| # | Task | Notes |
|---|---|---|
| 4.1 | Camera, geolocation, notifications, haptics, sensors via Tauri plugins | desktop lacks these |
| 4.2 | SAF/`content://` adapter for user-file access | Android |
| 4.3 | Background wake: `WorkManager` / `BGTaskScheduler` for cron/heartbeat | optional enhancement |
| 4.4 | iOS build (scoped-down tool surface) | post-Android |

---

## 6. Risk register

| Risk | Impact | Mitigation |
|---|---|---|
| Hidden platform-bound dependency breaks cross-compile | Blocked build | P1 dependency checklist (§5) gates the build; revisit scope on any failure |
| Mobile secret storage diverges (keyring is desktop-only) | API keys exposed | file store (0600) short-term; Keystore/Keychain backend later (§4.9) |
| iOS tool surface too small to be useful | Product value | Ship Android-first; document iOS as chat+delegation only |
| App-process crash from a misbehaving library thread | Lost state | WASM sandbox for untrusted code; keep desktop subprocess model on desktop |
| Background scheduling complexity | Scope creep | Ship foreground-only cron first; background is opt-in (4.3) |
| User-file access semantics diverge from desktop | Confusion | Keep agent file ops sandboxed; user files via HTTP/SAF adapters (4.8) |
| Agent "workspace_only" / sandbox config interacts with sandboxed mobile FS | Security regressions | Re-verify `is_available()` + workspace confinement on device |

---

## 7. References

- Base dir choke point: `src/dirs.rs` (`syscity_dir()` at line 34)
- `is_available()` trait method: `src/tools/types.rs:769`
- `src/computer/` platform gates: `src/computer/mod.rs:42-46`
- MCP multi-transport: `src/mcp/client.rs` (stdio / SSE / streamable-HTTP)
- In-process cron scheduler: `src/cron/cron/scheduler.rs`
- Desktop shell embeds gateway: `desktop/src/lib.rs`
- Desktop bundle targets (no mobile): `desktop/tauri.conf.json`
- SPA serving: `src/gateway/lifecycle.rs:926` (`/assets/*path`)
- Delegation is process-internal: `src/agent/subagent_registry.rs`,
  `src/delegation/` (tasks keyed `delegation:<run_id>`)

---

*This document is a proposal. Phase 1 is the smallest useful slice that proves
the whole idea; it is also the recommended first PR.*
