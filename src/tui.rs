mod command;
mod event;
mod message;
mod model;
mod terminal;

use color_eyre::eyre::{Context, Result};
use tracing::info;

use command::Command;
use message::Message;
use model::Model;
use terminal::TerminalGuard;

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
    /// # Errors
    ///
    /// Returns an error if the terminal cannot be set up (most likely
    /// because stdout isn't a terminal at all), or if drawing to it fails.
    pub async fn run(&mut self) -> Result<()> {
        // Logged before the terminal is taken over, so a failure to start is
        // still recorded, and so each session is marked in the log.
        info!("Starting the management interface");

        let mut terminal = TerminalGuard::new().wrap_err("failed to initialize the terminal")?;

        // Crossterm doesn't synthesize an initial resize event, so the
        // model's terminal size has to be seeded directly here - before the
        // first paint, so it's never stale even for an Up/Down keypress
        // that arrives before any real resize happens.
        self.model.terminal_size = terminal
            .size()
            .wrap_err("failed to read the terminal size")?;

        // Do an initial paint before any events come in.
        terminal
            .draw(|frame| self.model.view(frame))
            .wrap_err("failed to draw to the terminal")?;

        let mut event_stream = EventStream::new();

        while let Some(message) = event_stream.next_message(&self.model).await {
            let command = self.model.update(&message);

            if let Some(command) = command {
                match command {
                    Command::Quit => break,
                }
            }

            terminal
                .draw(|frame| self.model.view(frame))
                .wrap_err("failed to draw to the terminal")?;
        }

        Ok(())
    }
}
