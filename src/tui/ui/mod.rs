//! TUI rendering coordinator.

use crate::tui::state::{AppState, Popup};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear},
    Frame,
};

mod chat;
mod config_editor;
mod help;
mod input;
mod sidebar;
mod status_bar;

/// Render the full UI.
pub fn render(f: &mut Frame, state: &AppState) {
    let area = f.area();

    // Main vertical layout: body + status bar + input.
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(area);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(main_chunks[0]);

    chat::render(f, state, body_chunks[0]);
    sidebar::render(f, state, body_chunks[1]);
    status_bar::render(f, state, main_chunks[1]);
    input::render(f, state, main_chunks[2]);

    match state.popup {
        Popup::Help => {
            let popup_area = centered_rect(60, 70, area);
            f.render_widget(Clear, popup_area);
            help::render(f, state, popup_area);
        }
        Popup::ConfigEditor => {
            let popup_area = centered_rect(70, 70, area);
            f.render_widget(Clear, popup_area);
            config_editor::render(f, state, popup_area);
        }
        Popup::None => {}
    }
}

/// Create a centered rectangle of given percentage size.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Helper: block with a title.
pub fn titled_block(title: &str) -> Block<'_> {
    Block::default().borders(Borders::ALL).title(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// Helper: dim style for secondary text.
pub fn dim_style() -> Style {
    Style::default().fg(Color::Gray)
}

/// Helper: highlight style for selected items.
pub fn highlight_style() -> Style {
    Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// Helper: style for user messages.
pub fn user_style() -> Style {
    Style::default().fg(Color::Cyan)
}

/// Helper: style for assistant messages.
pub fn assistant_style() -> Style {
    Style::default().fg(Color::Green)
}

/// Helper: style for error toasts.
pub fn error_style() -> Style {
    Style::default().fg(Color::Red)
}

/// Helper: style for warning / system messages.
pub fn system_style() -> Style {
    Style::default().fg(Color::Yellow)
}

/// Helper: style for reasoning blocks.
pub fn reasoning_style() -> Style {
    Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::ITALIC)
}

/// Helper: style for tool-call blocks.
pub fn tool_call_style() -> Style {
    Style::default().fg(Color::Magenta)
}

/// Helper: style for code blocks.
pub fn code_style() -> Style {
    Style::default().bg(Color::Black).fg(Color::White)
}
