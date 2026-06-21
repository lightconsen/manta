//! Terminal setup, panic recovery, and TUI orchestration.

use std::io::{stdout, Stdout};
use std::panic;
use std::sync::Arc;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

use crate::tui::auth::AuthConfig;
use crate::tui::error::TuiError;
use crate::tui::event_loop;
use crate::tui::input::spawn_input_reader;
use crate::tui::state::AppState;
use crate::tui::ws_client::WsClient;

/// Run the TUI.
pub async fn run(
    host: &str,
    port: u16,
    token: Option<&str>,
    session: Option<&str>,
) -> Result<(), TuiError> {
    let mut terminal = setup_terminal()?;

    // Install a panic hook that restores the terminal before printing.
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    let result = run_app(&mut terminal, host, port, token, session).await;

    restore_terminal()?;

    if let Err(ref e) = result {
        eprintln!("TUI error: {e}");
    }

    result
}

/// Initialize crossterm and ratatui.
fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>, TuiError> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(TuiError::Terminal)
}

/// Restore the terminal to its original state.
fn restore_terminal() -> Result<(), TuiError> {
    disable_raw_mode()?;
    let mut stdout = stdout();
    crossterm::execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

/// Core application lifecycle.
async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    host: &str,
    port: u16,
    token: Option<&str>,
    session: Option<&str>,
) -> Result<(), TuiError> {
    let auth = AuthConfig::from_token(token);
    let url = auth.ws_url(host, port, session, "tui");

    let (ws_client, hello) =
        WsClient::connect(&url, &auth, session, &["chat", "read", "write"]).await?;

    let state = Arc::new(RwLock::new(AppState::default()));
    {
        let mut s = state.write().await;
        s.connection = crate::tui::state::ConnectionState::Connected {
            features: hello.features,
            scopes_granted: hello.scopes_granted,
            server_version: hello.server.version,
        };
        if let Some(sid) = session {
            s.current_session = Some(sid.to_string());
        }
    }

    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_input_reader(input_tx);

    let render_interval = interval(Duration::from_millis(33));

    event_loop::run(terminal, Arc::clone(&state), ws_client, input_rx, render_interval).await
}

/// A small wrapper to print errors after terminal shutdown.
#[allow(dead_code)]
pub fn print_error(err: &TuiError) {
    eprintln!("TUI error: {err}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_terminal_does_not_panic() {
        // Terminal may not be in raw mode; restore should still be safe.
        let _ = restore_terminal();
    }
}
