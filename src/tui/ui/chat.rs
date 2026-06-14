//! Chat panel rendering.

use crate::tui::state::{AppState, ChatMessage, MessageStatus};
use crate::tui::ui::{
    assistant_style, code_style, dim_style, error_style, reasoning_style, system_style,
    titled_block, tool_call_style, user_style,
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
            Line::from("  Press Ctrl+C to abort a run or quit."),
            Line::from("  Shift+Enter inserts a newline."),
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
        "tool" => ("Tool", tool_call_style()),
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

    if msg.parts.is_empty() {
        lines.extend(render_text_content(&msg.content, style));
    } else {
        for part in &msg.parts {
            match part.part_type.as_str() {
                "reasoning" => {
                    if let Some(text) = &part.text {
                        lines.push(Line::from(Span::styled(
                            "thinking:",
                            reasoning_style().add_modifier(Modifier::BOLD),
                        )));
                        for line in text.lines() {
                            lines.push(Line::from(Span::styled(
                                format!("  {}", line),
                                reasoning_style(),
                            )));
                        }
                    }
                }
                "tool-call" => {
                    let tool_name = part
                        .tool_name
                        .as_deref()
                        .unwrap_or(msg.tool_name.as_deref().unwrap_or("tool"));
                    lines.push(Line::from(vec![
                        Span::styled("tool: ", tool_call_style().add_modifier(Modifier::BOLD)),
                        Span::styled(tool_name, tool_call_style()),
                    ]));
                    if let Some(args) = &part.args {
                        let args_text = serde_json::to_string_pretty(args).unwrap_or_default();
                        for line in args_text.lines() {
                            lines.push(Line::from(Span::styled(
                                format!("  {}", line),
                                tool_call_style(),
                            )));
                        }
                    }
                    if let Some(result) = &part.result {
                        let result_text = serde_json::to_string_pretty(result).unwrap_or_default();
                        lines.push(Line::from(Span::styled(
                            "  → result:",
                            tool_call_style().add_modifier(Modifier::BOLD),
                        )));
                        for line in result_text.lines() {
                            lines.push(Line::from(Span::styled(
                                format!("    {}", line),
                                tool_call_style(),
                            )));
                        }
                    }
                }
                _ => {
                    if let Some(text) = &part.text {
                        lines.extend(render_text_content(text, style));
                    }
                }
            }
        }
        // Also render any plain content not represented by parts.
        if msg.content.is_empty() || msg.parts.iter().any(|p| p.part_type == "text") {
            // Already covered.
        } else {
            lines.extend(render_text_content(&msg.content, style));
        }
    }

    if let Some(thinking) = &msg.thinking {
        lines.push(Line::from(Span::styled(
            "thinking:",
            reasoning_style().add_modifier(Modifier::BOLD),
        )));
        for line in thinking.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                reasoning_style(),
            )));
        }
    }

    lines
}

/// Render text content, detecting fenced code blocks.
fn render_text_content(content: &str, base_style: Style) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut in_code = false;

    for line in content.lines() {
        if let Some(lang) = line.strip_prefix("```") {
            in_code = !in_code;
            if in_code && !lang.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("code: {}", lang.trim()),
                    code_style().add_modifier(Modifier::BOLD),
                )));
            }
            continue;
        }

        if in_code {
            lines.push(Line::from(Span::styled(line.to_string(), code_style())));
        } else {
            lines.push(Line::from(Span::styled(line.to_string(), base_style)));
        }
    }

    // If no lines were produced (empty content), render the raw text.
    if lines.is_empty() && !content.is_empty() {
        lines.push(Line::from(Span::styled(content.to_string(), base_style)));
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

    #[test]
    fn code_block_detected() {
        let content = "```rust\nlet x = 1;\n```";
        let lines = render_text_content(content, Style::default());
        assert!(lines.iter().any(|l| l.to_string().contains("code: rust")));
    }
}
