//! Chat panel rendering.

use crate::tui::state::{AppState, ChatMessage, MessageStatus};
use crate::tui::ui::{
    assistant_style, dim_style, error_style, system_style, titled_block, user_style,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

/// Render the chat message list.
pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let block = titled_block("Chat");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.messages.is_empty() {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "  Welcome to Syscity TUI",
                Style::default().add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from("  Type a message and press Enter to chat."),
            Line::from("  Press Ctrl+H for help, Ctrl+E for config editor."),
            Line::from("  Press Ctrl+C or Ctrl+Q to quit."),
        ])
        .wrap(Wrap { trim: false });
        f.render_widget(hint, inner);
        return;
    }

    let lines = build_message_lines(state, inner.width as usize);
    let total_lines = lines.len();
    let visible_height = inner.height as usize;

    let scroll = total_lines
        .saturating_sub(visible_height)
        .saturating_sub(state.scroll_offset);
    let scroll = scroll.min(total_lines.saturating_sub(visible_height));

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    f.render_widget(paragraph, inner);
}

/// Build wrapped lines for all messages.
fn build_message_lines(state: &AppState, width: usize) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    for msg in &state.messages {
        lines.extend(message_to_lines(msg, width));
        lines.push(Line::from(""));
    }
    lines
}

/// Convert a single message into wrapped lines.
fn message_to_lines(msg: &ChatMessage, _width: usize) -> Vec<Line<'_>> {
    let mut lines = Vec::new();

    let (role_label, style) = match msg.role.as_str() {
        "user" => ("You", user_style()),
        "assistant" => ("Assistant", assistant_style()),
        "system" => ("System", system_style()),
        _ => ("Unknown", dim_style()),
    };

    let status_indicator = match &msg.status {
        MessageStatus::Sending => Span::styled(" •", dim_style()),
        MessageStatus::Streaming => Span::styled(" …", dim_style()),
        MessageStatus::Complete => Span::styled("", Style::default()),
        MessageStatus::Error(e) => Span::styled(format!(" error: {}", e), error_style()),
    };

    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", role_label),
            style.add_modifier(Modifier::BOLD),
        ),
        status_indicator,
    ]));

    for line in msg.content.lines() {
        lines.push(Line::from(Span::styled(line.to_string(), style)));
    }

    if let Some(thinking) = &msg.thinking {
        lines.push(Line::from(Span::styled(
            "thinking:",
            dim_style().add_modifier(Modifier::ITALIC),
        )));
        for line in thinking.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                dim_style().add_modifier(Modifier::ITALIC),
            )));
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::ChatMessage;

    #[test]
    fn user_message_lines() {
        let msg = ChatMessage {
            role: "user".to_string(),
            content: "hello\nworld".to_string(),
            ..ChatMessage::default()
        };
        let lines = message_to_lines(&msg, 80);
        assert!(lines.len() >= 3);
    }
}
