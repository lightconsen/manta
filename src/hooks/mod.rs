//! Claude-Code-compatible shell hooks bridge.
//!
//! Lets an operator express interception / audit / observation behaviour as
//! plain shell commands in a standalone `~/.syscity/hooks.json` (same schema
//! as Claude Code's `settings.json` hooks), with **no plugin ABI and no
//! recompilation**.
//!
//! The four supported events map onto syscity seams as follows:
//!
//! | CC event | syscity seam |
//! |---|---|
//! | `PreToolUse`  | `ToolHooks` policy hook (before a tool executes) |
//! | `PostToolUse` | `ToolHooks` post-execute hook (after a tool finishes) |
//! | `UserPromptSubmit` | `send_to_agent` gate in the gateway dispatcher |
//! | `Stop`        | fire-and-forget hook at the end of a turn |
//!
//! ## Contract
//!
//! - **Fail-open.** A hook that crashes, times out, exits non-zero, or prints
//!   unparsable output never locks the agent out: the call is allowed /
//!   accepted / passed through, and the anomaly is `warn!`-logged.
//! - **Parameters are never rewritten.** Claude Code's `updatedInput` field
//!   is parsed but deliberately ignored — a hook can block a tool call
//!   (`deny` / `ask`) or confiscate its result (`block`), but it can never
//!   mutate the arguments the tool actually receives, so log, execution, and
//!   UI stay consistent.
//! - **Config is read once at startup.** Changes to `hooks.json` take effect
//!   on the next daemon restart; there is no hot reload.
//!
//! This module is intentionally distinct from the three existing hook
//! concepts in the codebase: `tools::hooks` (in-process `ToolHooks`),
//! `gateway::hooks` (message/event hooks), and `plugins::hooks` (plugin ABI).
//! The bridge here *consumes* `tools::hooks` — it is a shell-based producer
//! of `ToolHooks` plus two dispatcher seams.
// INVARIANTS-NONE: shell hook bridge serializes decisions per call site within one invocation; no durable state.

pub mod bridge;
pub mod config;
pub mod executor;
pub mod matcher;

pub use bridge::ShellHookBridge;
pub use config::{HookEvent, ShellHooksConfig};
