//! Session sidebar rendering.

use crate::tui::state::{AppState, ConnectionState};
use crate::tui::ui::{highlight_style, titled_block};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{List, ListItem, ListState},
    Frame,
};

/// Render the session sidebar.
pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let block = titled_block("Sessions");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(idx, session)| {
            let label = session.label.as_deref().unwrap_or(&session.id).to_string();
            let prefix = if Some(&session.id) == state.current_session.as_ref() {
                "● "
            } else {
                "  "
            };
            let content = format!("{}{}", prefix, label);
            let style = if idx == state.selected_session_index {
                highlight_style()
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(content, style)))
        })
        .collect();

    let list = List::new(items);
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_session_index));
    f.render_stateful_widget(list, inner, &mut list_state);

    // Render connection indicator at the bottom of the sidebar area.
    let conn_text = match &state.connection {
        ConnectionState::Connected { .. } => Span::styled(
            " connected ",
            Style::default().fg(ratatui::style::Color::Green),
        ),
        ConnectionState::Connecting => Span::styled(
            " connecting ",
            Style::default().fg(ratatui::style::Color::Yellow),
        ),
        ConnectionState::Disconnected => Span::styled(
            " disconnected ",
            Style::default().fg(ratatui::style::Color::Gray),
        ),
        ConnectionState::Error(e) => Span::styled(
            format!(" error: {} ", e),
            Style::default().fg(ratatui::style::Color::Red),
        ),
    };
    let conn_line = Line::from(conn_text);
    let conn_para = ratatui::widgets::Paragraph::new(conn_line);
    // Place at bottom.
    if inner.height > 0 {
        let conn_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };
        f.render_widget(conn_para, conn_area);
    }
}
