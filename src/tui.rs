mod command;
mod event;
mod message;
mod model;

use command::Command;
use message::Message;
use model::Model;

use crate::tui::event::EventStream;

#[derive(Debug, Default)]
pub struct App {
    model: Model,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl App {
    /// Runs the main loop until the application is asked to quit, or until no
    /// further terminal input can arrive.
    ///
    /// # Panics
    ///
    /// Panics if another `App` is already running. Both would read from the
    /// same terminal and each would receive an arbitrary share of the input,
    /// so this is always a bug.
    ///
    /// Also panics if the terminal cannot be set up, which mostly means the
    /// output isn't a terminal at all.
    pub async fn run(&mut self) {
        // Nothing is drawn yet, so the terminal is held only to keep raw mode
        // and the alternate screen active for as long as the loop runs. It
        // also installs a panic hook which restores the terminal first, so a
        // panic doesn't print into a screen still in raw mode.
        let _terminal = ratatui::init();

        let mut event_stream = EventStream::new();

        while let Some(message) = event_stream.next_message(&self.model).await {
            let command = self.model.update(&message);

            if let Some(command) = command {
                match command {
                    Command::Quit => break,
                }
            }
        }

        ratatui::restore();
    }
}
