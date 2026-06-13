//! User actions produced by keyboard input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// High-level user intent emitted by the input mapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiAction {
    /// Send the current input as a chat message.
    SendMessage,
    /// Run a slash command (includes the leading `/`).
    RunSlashCommand(String),
    /// Append a character to the input buffer.
    InputChar(char),
    /// Delete the character before the cursor.
    InputBackspace,
    /// Move cursor left.
    CursorLeft,
    /// Move cursor right.
    CursorRight,
    /// Move to the beginning of the input.
    CursorHome,
    /// Move to the end of the input.
    CursorEnd,
    /// Scroll the chat panel up.
    ScrollUp,
    /// Scroll the chat panel down.
    ScrollDown,
    /// Switch focus to the next pane.
    FocusNext,
    /// Switch focus to the previous pane.
    FocusPrevious,
    /// Open the help popup.
    OpenHelp,
    /// Close the current popup.
    ClosePopup,
    /// Open the config editor popup.
    OpenConfigEditor,
    /// Move selection up (sidebar / popup).
    SelectUp,
    /// Move selection down (sidebar / popup).
    SelectDown,
    /// Activate the currently selected item.
    SelectEnter,
    /// Delete the currently selected session.
    DeleteSelected,
    /// Create a new session.
    NewSession,
    /// Approve a pending approval.
    Approve { id: String },
    /// Reject a pending approval.
    Reject { id: String },
    /// Terminal resized.
    Resize(u16, u16),
    /// Save pending config edits.
    SaveConfig,
    /// Quit the application.
    Quit,
    /// No-op.
    None,
}

impl TuiAction {
    /// Map a crossterm key event to a `TuiAction`.
    pub fn from_key_event(key: KeyEvent) -> Self {
        match key.code {
            KeyCode::Char('s') if key.modifiers == KeyModifiers::CONTROL => Self::SaveConfig,
            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => Self::Quit,
            KeyCode::Char('q') if key.modifiers == KeyModifiers::CONTROL => Self::Quit,
            KeyCode::Char('l') if key.modifiers == KeyModifiers::CONTROL => Self::FocusNext,
            KeyCode::Char('h') if key.modifiers == KeyModifiers::CONTROL => Self::OpenHelp,
            KeyCode::Char('e') if key.modifiers == KeyModifiers::CONTROL => Self::OpenConfigEditor,
            KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => Self::NewSession,
            KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => Self::DeleteSelected,
            KeyCode::Esc => Self::ClosePopup,
            KeyCode::Enter => Self::SendMessage,
            KeyCode::Up => Self::SelectUp,
            KeyCode::Down => Self::SelectDown,
            KeyCode::PageUp => Self::ScrollUp,
            KeyCode::PageDown => Self::ScrollDown,
            KeyCode::Left => Self::CursorLeft,
            KeyCode::Right => Self::CursorRight,
            KeyCode::Home => Self::CursorHome,
            KeyCode::End => Self::CursorEnd,
            KeyCode::Backspace => Self::InputBackspace,
            KeyCode::Delete => Self::InputBackspace,
            KeyCode::Tab => Self::FocusNext,
            KeyCode::BackTab => Self::FocusPrevious,
            KeyCode::Char(c) => Self::InputChar(c),
            _ => Self::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_quits() {
        let key = KeyEvent::from(KeyCode::Char('c'));
        // KeyEvent::from does not set modifiers; construct manually for control.
        let ctrl_c = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::empty(),
        };
        assert_eq!(TuiAction::from_key_event(ctrl_c), TuiAction::Quit);
    }

    #[test]
    fn enter_sends_message() {
        let key = KeyEvent::from(KeyCode::Enter);
        assert_eq!(TuiAction::from_key_event(key), TuiAction::SendMessage);
    }

    #[test]
    fn char_input() {
        let key = KeyEvent::from(KeyCode::Char('a'));
        assert_eq!(TuiAction::from_key_event(key), TuiAction::InputChar('a'));
    }
}
