//! Terminal input capture, and the mapping from crossterm events to
//! [`Message`]s.

mod keymap;

use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use ratatui::crossterm;
use tracing::error;

use crate::tui::{Message, model::Model};

/// Whether an [`EventStream`] currently exists.
static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Reads terminal events and maps them to [`Message`]s.
///
/// Crossterm reads from a process-global event source, so two of these running
/// at once would split the input stream between them at random. Holding
/// a single one for the lifetime of the interface is what keeps that from
/// happening.
#[derive(Debug)]
pub struct EventStream {
    terminal_input: crossterm::event::EventStream,
}

impl EventStream {
    /// Starts capturing terminal input, until the returned value is dropped.
    ///
    /// # Panics
    ///
    /// Panics if another [`EventStream`] already exists. Two streams executing
    /// at the same time is always a bug.
    #[must_use]
    pub fn new() -> Self {
        assert!(
            CAPTURING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok(),
            "terminal input is already being captured by another EventStream"
        );

        Self {
            terminal_input: crossterm::event::EventStream::new(),
        }
    }

    /// Waits for the next message.
    ///
    /// Terminal input which doesn't map to anything in the current model is
    /// discarded, so this only returns once something actionable arrives.
    ///
    /// Returns [`None`] once no further input can arrive, because the
    /// terminal input stream ended or failed.
    pub async fn next_message(&mut self, model: &Model) -> Option<Message> {
        loop {
            let event = match self.terminal_input.next().await? {
                Ok(event) => event,
                // Read errors are not transient: they mean the input stream
                // is gone, so there is nothing to wait for anymore.
                Err(error) => {
                    error!(%error, "Failed to read terminal input, stopping capture");
                    return None;
                }
            };

            if let Some(message) = map_event(model, &event) {
                return Some(message);
            }
        }
    }
}

impl Default for EventStream {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        CAPTURING.store(false, Ordering::Release);
    }
}

/// Maps a crossterm event to a message.
fn map_event(model: &Model, event: &crossterm::event::Event) -> Option<Message> {
    match event {
        crossterm::event::Event::Key(key_event) => keymap::keymap(model, key_event),
        crossterm::event::Event::Resize(width, height) => Some(Message::Resize(*width, *height)),
        crossterm::event::Event::Mouse(_)
        | crossterm::event::Event::Paste(_)
        | crossterm::event::Event::FocusGained
        | crossterm::event::Event::FocusLost => None,
    }
}
