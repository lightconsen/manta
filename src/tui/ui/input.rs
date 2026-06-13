//! Input bar rendering.

use crate::tui::state::{AppState, InputMode};
use crate::tui::ui::titled_block;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Render the input bar.
pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let title = match state.input_mode {
        InputMode::Normal => "Input",
        InputMode::ConfigEdit => "Config Value",
        InputMode::Popup => "",
    };

    let block = if title.is_empty() {
        ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL)
    } else {
        titled_block(title)
    };

    let inner = block.inner(area);
    f.render_widget(block, area);

    let text = if state.input_mode == InputMode::Popup {
        "Press Esc to close popup".to_string()
    } else {
        state.input_buffer.clone()
    };

    let style = if state.input_mode == InputMode::ConfigEdit {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let paragraph = Paragraph::new(Line::from(Span::styled(text, style)));
    f.render_widget(paragraph, inner);

    // Show a hint for slash commands.
    if state.input_mode == InputMode::Normal && state.input_buffer.starts_with('/') {
        let hint = match state.input_buffer.as_str() {
            "/n" | "/ne" | "/new" => " /new - create a session",
            "/c" | "/cl" | "/cle" | "/clea" | "/clear" => " /clear - clear chat",
            "/s" | "/st" | "/sta" | "/stat" | "/statu" | "/status" => " /status - gateway status",
            "/t" | "/to" | "/too" | "/tool" | "/tools" => " /tools - list commands",
            "/m" | "/mo" | "/mod" | "/mode" | "/model" => " /model <id> - switch model",
            "/h" | "/he" | "/hel" | "/help" => " /help - show help",
            "/co" | "/con" | "/conf" | "/confi" | "/config" => " /config - config editor",
            "/q" | "/qu" | "/qui" | "/quit" => " /quit - exit",
            _ => "",
        };
        if !hint.is_empty() {
            let hint_para = Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            )));
            let hint_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y.saturating_sub(1),
                width: inner.width,
                height: 1,
            };
            f.render_widget(hint_para, hint_area);
        }
    }
}
