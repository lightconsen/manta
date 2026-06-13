//! Help popup rendering.

use crate::tui::state::AppState;
use crate::tui::ui::titled_block;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    widgets::{Row, Table},
    Frame,
};

/// Render the help popup.
pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let block = titled_block("Help (Esc to close)");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut rows = vec![
        Row::new(vec!["Ctrl+C / Ctrl+Q", "Quit"]),
        Row::new(vec!["Ctrl+H", "Open help"]),
        Row::new(vec!["Ctrl+E", "Open config editor"]),
        Row::new(vec!["Ctrl+N", "New session"]),
        Row::new(vec!["Ctrl+D", "Delete selected session"]),
        Row::new(vec!["Up/Down", "Navigate sessions / config rows"]),
        Row::new(vec!["Enter", "Select session / edit config"]),
        Row::new(vec!["PgUp/PgDown", "Scroll chat"]),
        Row::new(vec!["Esc", "Close popup"]),
    ];

    for cmd in &state.command_list {
        rows.push(Row::new(vec![
            format!("/{} {}", cmd.name, cmd.usage),
            cmd.description.clone(),
        ]));
    }

    let widths = [Constraint::Percentage(40), Constraint::Percentage(60)];
    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["Key / Command", "Description"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .column_spacing(2);
    f.render_widget(table, inner);
}
