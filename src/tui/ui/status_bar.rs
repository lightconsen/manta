//! Status bar rendering.

use crate::tui::state::{AppState, ConnectionState};
use crate::tui::ui::{dim_style, error_style};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Render the status bar.
pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let session = state
        .current_session
        .as_deref()
        .map(|s| format!(" session: {} ", s))
        .unwrap_or_else(|| " no session ".to_string());

    let conn = match &state.connection {
        ConnectionState::Connected {
            scopes_granted,
            server_version,
            ..
        } => {
            let scopes = if scopes_granted.is_empty() {
                "none".to_string()
            } else {
                scopes_granted.join(",")
            };
            format!(" connected | v{} | scopes: {} ", server_version, scopes)
        }
        ConnectionState::Connecting => " connecting ".to_string(),
        ConnectionState::Disconnected => " disconnected ".to_string(),
        ConnectionState::Error(e) => format!(" error: {} ", e),
    };

    let toast = state
        .toasts
        .last()
        .map(|t| {
            if t.is_error {
                Span::styled(format!(" {} ", t.message), error_style())
            } else {
                Span::styled(format!(" {} ", t.message), Style::default().fg(Color::Blue))
            }
        })
        .unwrap_or_else(|| Span::styled("", Style::default()));

    let line = Line::from(vec![
        Span::styled(session, dim_style()),
        Span::styled(conn, Style::default().fg(Color::Gray)),
        toast,
    ]);

    f.render_widget(Paragraph::new(line), area);
}
