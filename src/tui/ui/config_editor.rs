//! Config editor popup rendering.

use crate::tui::event_loop::config_keys;
use crate::tui::state::{AppState, InputMode};
use crate::tui::ui::{dim_style, highlight_style, titled_block};
use ratatui::{
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
    Frame,
};

/// Render the config editor popup.
pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let block = titled_block("Config Editor (Ctrl+S save, Esc close)");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let keys = config_keys();
    let rows: Vec<Row> = keys
        .iter()
        .enumerate()
        .map(|(idx, key): (usize, &String)| {
            let value = state
                .config_edits
                .get(key)
                .cloned()
                .or_else(|| state.config_cache.get(key).map(|v| v.to_string()))
                .unwrap_or_else(|| "-".to_string());

            let style = if idx == state.config_selected_index {
                highlight_style()
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(Span::styled(key.clone(), style)),
                Cell::from(Span::styled(
                    truncate(&value, inner.width as usize / 2),
                    style,
                )),
            ])
            .style(style)
        })
        .collect();

    let widths = [Constraint::Percentage(40), Constraint::Percentage(60)];
    let table = Table::new(rows, widths)
        .header(Row::new(vec!["Key", "Value"]).style(Style::default().add_modifier(Modifier::BOLD)))
        .column_spacing(2);
    f.render_widget(table, inner);

    if state.input_mode == InputMode::ConfigEdit {
        let hint = Paragraph::new(Line::from(Span::styled(
            "Editing: type value, Enter to confirm",
            dim_style(),
        )));
        let hint_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        f.render_widget(hint, hint_area);
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!(
            "{}...",
            &s[..s
                .char_indices()
                .nth(max_len.saturating_sub(3))
                .map(|(i, _)| i)
                .unwrap_or(s.len())]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate("hello world", 5).len(), 5);
        assert!(truncate("hello world", 5).ends_with("..."));
    }
}
