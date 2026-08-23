//! Interactive terminal UI client for Syscity.
//!
//! Enabled by the `tui` Cargo feature. Connects to a running Syscity gateway
//! over WebSocket and provides real-time chat, session management, slash
//! commands, and a configuration editor.
// INVARIANTS-NONE: presentation layer; owns no shared persistent state.

mod actions;
mod app;
mod auth;
mod commands;
mod error;
mod event_loop;
mod input;
pub mod state;
pub mod ui;
mod ws_client;

use crate::error::{Result, SyscityError};

/// Run the TUI client.
///
/// `host` and `port` identify the gateway. `token` is used when the gateway is
/// configured with `auth_mode = "token"`. `session` pre-selects an existing
/// session ID if provided.
pub async fn run(host: &str, port: u16, token: Option<&str>, session: Option<&str>) -> Result<()> {
    app::run(host, port, token, session)
        .await
        .map_err(|e| SyscityError::Validation(format!("TUI error: {e}")))
}
