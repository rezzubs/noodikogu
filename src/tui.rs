mod effect;
mod event;
mod message;
mod model;
mod terminal;

use std::future::Future;
use std::ops::ControlFlow;

use color_eyre::eyre::{Context, Result};
use tokio::sync::mpsc;
use tracing::{info, warn};

use effect::Effect;
use message::Message;
use model::Model;
use model::context::search;
use terminal::TerminalGuard;

use crate::catalogue::{Catalogue, DatabaseError, Pagination, ScoreDetailError};
use crate::query::ScoreQuery;
use crate::tui::event::EventStream;

const SCORE_REFETCH_ATTEMPTS_MAX: u8 = 3;

#[derive(Debug)]
pub struct App {
    model: Model,
    catalogue: Catalogue,
}

impl App {
    /// Opens the catalogue database at `database_path` and prepares to run
    /// the management interface.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] if the catalogue can't be opened - see
    /// [`Catalogue::open`].
    pub async fn new(database_path: &str) -> Result<Self, DatabaseError> {
        let catalogue = Catalogue::open(database_path).await?;
        Ok(Self {
            model: Model::default(),
            catalogue,
        })
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

        // Effects can send messages back through this channel.
        let (effect_message_sender, mut effect_message_receiver) =
            mpsc::unbounded_channel::<Message>();

        loop {
            // select never drops messages. Both sources have a fair chance of
            // being read.
            let message = tokio::select! {
                message = event_stream.next_message(&self.model) => match message {
                    Some(message) => message,
                    None => break,
                },
                message = effect_message_receiver.recv() =>
                    message.expect("The transmitter exists outside the loop"),
            };

            let effect = self.model.update(message);

            if let Some(effect) = effect
                && let ControlFlow::Break(result) =
                    self.execute_effect(effect, &effect_message_sender)
            {
                return result;
            }

            terminal
                .draw(|frame| self.model.view(frame))
                .wrap_err("failed to draw to the terminal")?;
        }

        Ok(())
    }

    /// Executes one [`Effect`], as asked for by [`Model::update`].
    ///
    /// Returns:
    /// - [`ControlFlow::Continue`] means [`run`]'s loop keeps going;
    /// - [`ControlFlow::Break`] carries `run`'s return value.
    fn execute_effect(
        &self,
        effect: Effect,
        message_sender: &mpsc::UnboundedSender<Message>,
    ) -> ControlFlow<Result<()>> {
        match effect {
            Effect::Quit => ControlFlow::Break(Ok(())),
            Effect::Fatal(report) => {
                ControlFlow::Break(Err(report).wrap_err("a catalogue background fetch failed"))
            }
            Effect::FetchScoreTile {
                generation,
                query,
                pagination,
            } => {
                self.spawn_tile_fetch(generation, query, pagination, message_sender);
                ControlFlow::Continue(())
            }
            Effect::SearchInvalidated { generation, query } => {
                // Only a log line for now - see ADR 0010's "Interim
                // mitigation" section for why this doesn't prompt yet.
                warn!(
                    ?query,
                    "search results became stale; resetting and refetching from the top"
                );
                let pagination = Pagination {
                    offset: 0,
                    limit: search::TILE_SIZE,
                };
                self.spawn_tile_fetch(generation, query, pagination, message_sender);
                ControlFlow::Continue(())
            }
        }
    }

    /// Spawns a background fetch for one tile, reporting the outcome back
    /// through `message_sender`.
    fn spawn_tile_fetch(
        &self,
        generation: u64,
        query: ScoreQuery,
        pagination: Pagination,
        message_sender: &mpsc::UnboundedSender<Message>,
    ) {
        let catalogue = self.catalogue.clone();
        let message_sender = message_sender.clone();
        tokio::spawn(async move {
            let message = fetch_tile_with_retries(&catalogue, &query, pagination)
                .await
                .map_or_else(Message::Fatal, |results| Message::ScoreTileFetched {
                    generation,
                    pagination,
                    results,
                    query,
                });
            if let Err(error) = message_sender.send(message) {
                warn!(%error, "the channel was closed before a tile fetch could execute");
            }
        });
    }
}

/// Fetches one tile, retrying a `ScoreNotFound` up to
/// [`SCORE_REFETCH_ATTEMPTS_MAX`] times before giving up.
///
/// A `ScoreNotFound` here means a score listed in the tile's page of search
/// results was deleted before its people could be hydrated - a narrow race
/// between browsing and a concurrent delete, not a sign anything is actually
/// wrong. A few retries give the tile a chance to settle on a consistent
/// view instead of ending the session (via `Message::Fatal`) over what's
/// usually a one-tick race.
async fn fetch_tile_with_retries(
    catalogue: &Catalogue,
    query: &ScoreQuery,
    pagination: Pagination,
) -> Result<Vec<search::ScoreResult>> {
    retry_score_not_found(SCORE_REFETCH_ATTEMPTS_MAX, || {
        search::fetch_tile(catalogue, query, pagination)
    })
    .await
}

/// Retries `operation` while it fails with `ScoreDetailError::ScoreNotFound`,
/// up to `max_attempts` attempts total. Any other error, or a
/// `ScoreNotFound` on the final attempt, is returned wrapped with how many
/// attempts were made. `Unexpected` database errors aren't retried - there's
/// no reason to expect a second attempt to behave differently.
///
/// Kept generic over `operation` rather than inlined into
/// [`fetch_tile_with_retries`] so the retry/give-up policy can be tested
/// directly against a fake, deterministic operation instead of needing to
/// force the real race (a score deleted mid-fetch) to happen against a live
/// `Catalogue`.
async fn retry_score_not_found<T, F, Fut>(max_attempts: u8, mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, ScoreDetailError>>,
{
    let mut attempt = 1;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(ScoreDetailError::ScoreNotFound(score_id)) if attempt < max_attempts => {
                warn!(
                    "score {score_id} not found while fetching a tile, attempt {attempt}/{max_attempts}, retrying"
                );
                attempt += 1;
            }
            Err(error) => {
                return Err(error).wrap_err_with(|| {
                    format!("failed to fetch a score tile after {attempt} attempt(s)")
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};

    use super::*;
    use crate::catalogue::{DatabaseError, ScoreId};

    #[tokio::test]
    async fn retry_score_not_found_recovers_after_a_transient_not_found() {
        let attempts = AtomicU8::new(0);

        let result = retry_score_not_found(3, || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt == 0 {
                    Err(ScoreDetailError::ScoreNotFound(ScoreId(1)))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.expect("should recover on the second attempt"), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_score_not_found_gives_up_after_max_attempts() {
        let attempts = AtomicU8::new(0);

        let result: Result<()> = retry_score_not_found(3, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err(ScoreDetailError::ScoreNotFound(ScoreId(1))) }
        })
        .await;

        assert!(
            result.is_err(),
            "should give up once attempts are exhausted"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_score_not_found_does_not_retry_other_errors() {
        let attempts = AtomicU8::new(0);

        let result: Result<()> = retry_score_not_found(3, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async {
                Err(ScoreDetailError::Unexpected(DatabaseError::from(
                    turso::Error::QueryReturnedNoRows,
                )))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "a non-ScoreNotFound error should fail on the first attempt"
        );
    }
}
