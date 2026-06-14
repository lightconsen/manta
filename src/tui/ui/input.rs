//! Input bar rendering.

use crate::tui::state::{AppState, InputMode};
use crate::tui::ui::{
    dim_style, highlight_style, titled_block, user_style,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
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

    let paragraph = Paragraph::new(text)
        .style(style)
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, inner);

    // Command palette overlay for slash-command input.
    if state.input_mode == InputMode::Normal
        && state.input_buffer.starts_with('/')
        && !state.palette_commands.is_empty()
    {
        let palette_height = (state.palette_commands.len() as u16 + 2).min(inner.height.saturating_sub(1));
        let palette_area = Rect {
            x: inner.x,
            y: inner.y.saturating_sub(palette_height),
            width: inner.width,
            height: palette_height,
        };
        let palette_block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title(Span::styled(
                " Commands ",
                Style::default().add_modifier(Modifier::BOLD),
            ));
        let palette_inner = palette_block.inner(palette_area);
        f.render_widget(ratatui::widgets::Clear, palette_area);
        f.render_widget(palette_block, palette_area);

        let items: Vec<Line> = state
            .palette_commands
            .iter()
            .enumerate()
            .map(|(idx, cmd)| {
                let style = if idx == state.palette_index {
                    highlight_style()
                } else {
                    dim_style()
                };
                Line::from(Span::styled(
                    format!("/{} {} - {}", cmd.name, cmd.usage, cmd.description),
                    style,
                ))
            })
            .collect();
        f.render_widget(
            Paragraph::new(items).wrap(Wrap { trim: false }),
            palette_inner,
        );
    }

    // Running / abort indicator.
    if state.is_running {
        let indicator = Paragraph::new(Line::from(vec![
            Span::styled("running", user_style()),
            Span::styled(" press Ctrl+C to stop", dim_style()),
        ]));
        let indicator_area = Rect {
            x: inner.x + inner.width.saturating_sub(25),
            y: inner.y,
            width: 25.min(inner.width),
            height: 1,
        };
        f.render_widget(indicator, indicator_area);
    }
}
