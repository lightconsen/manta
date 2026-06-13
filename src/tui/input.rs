//! Async crossterm input reader.

use crate::tui::actions::TuiAction;
use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use tokio::sync::mpsc;

/// Spawn a task that reads crossterm events and sends `TuiAction`s to `tx`.
pub fn spawn_input_reader(tx: mpsc::UnboundedSender<TuiAction>) {
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        loop {
            match reader.next().await {
                Some(Ok(Event::Key(key))) => {
                    let action = TuiAction::from_key_event(key);
                    if action != TuiAction::None && tx.send(action).is_err() {
                        break;
                    }
                }
                Some(Ok(Event::Resize(cols, rows))) => {
                    if tx.send(TuiAction::Resize(cols, rows)).is_err() {
                        break;
                    }
                }
                Some(Err(_)) | None => break,
                _ => {}
            }
        }
    });
}
