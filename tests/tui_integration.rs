//! Integration tests for the ratatui-based TUI client.
//!
//! These tests use `ratatui::backend::TestBackend` to verify that the
//! main render coordinator draws expected elements without panicking.

use ratatui::{backend::TestBackend, Terminal};
use syscity::tui::state::{AppState, ConnectionState};
use syscity::tui::ui::render;

#[test]
fn render_empty_state_does_not_panic() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = AppState::default();

    terminal
        .draw(|f| render(f, &state))
        .expect("draw should succeed");
}

#[test]
fn render_shows_connection_status() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = AppState::default();
    state.connection = ConnectionState::Error("gateway offline".to_string());

    terminal
        .draw(|f| render(f, &state))
        .expect("draw should succeed");

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(content.contains("offline"), "status bar should show offline");
}

#[test]
fn render_shows_session_sidebar() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = AppState::default();
    state.ensure_session("test-session");
    state.switch_session("test-session");

    terminal
        .draw(|f| render(f, &state))
        .expect("draw should succeed");

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(content.contains("test-session"), "sidebar should show session id");
}

#[test]
fn render_with_messages_shows_chat_content() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = AppState::default();
    state.append_user_message("msg-1", "hello world");

    terminal
        .draw(|f| render(f, &state))
        .expect("draw should succeed");

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(content.contains("hello world"), "chat panel should show message content");
}
