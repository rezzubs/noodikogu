use color_eyre::eyre;

use crate::catalogue::Pagination;
use crate::query::ScoreQuery;

/// An effect for the runtime to carry out on the model's behalf.
///
/// [`Model::update`](crate::tui::Model::update) returns these instead of
/// performing the work itself, so it stays synchronous and free of any
/// dependency on the async runtime.
///
/// Not `Clone`: [`eyre::Report`] isn't, and nothing needs `Effect` to be.
#[derive(Debug)]
pub enum Effect {
    /// Leave the main loop.
    ///
    /// Quitting is an effect rather than model state: nothing about the
    /// catalogue or the interface changes, the loop simply stops.
    Quit,
    /// A fatal error occured in the program and it must exit gracefully.
    ///
    /// Carries a full [`eyre::Report`] (rather than a flattened `String`) so
    /// context can be layered on with `wrap_err` close to where the error
    /// actually happened - e.g. inside the retry loop that produced it -
    /// instead of being lost by the time it reaches here.
    ///
    /// Counterpart to [`Message::Fatal`](crate::tui::Message::Fatal).
    Fatal(eyre::Report),
    /// Fetch one tile's worth of scores in the background.
    ///
    /// `generation` is the value of `Model::generation` at the time this was
    /// issued. This can be used to ignore stale responses.
    FetchScoreTile {
        /// The matching [`Model`](crate::tui::model::Model) generation.
        generation: u64,
        /// The query to execute on the database.
        query: ScoreQuery,
        /// Pagination for the search.
        pagination: Pagination,
    },
    /// A search's cached tiles were found to be stale and have already been
    /// discarded from the model - the search needs to restart from the top.
    // TODO:
    // A distinct effect rather than `Model::update` re-fetching inline, so
    // there's a seam in how [`App`](crate::tui::App) executes it for a
    // future confirmation prompt, without changing `Model::update`'s shape
    // again. For now, handling it just logs and re-fetches immediately.
    SearchInvalidated {
        /// The matching [`Model`](crate::tui::model::Model) generation,
        /// already bumped past the stale search's.
        generation: u64,
        /// The query to restart the search with.
        query: ScoreQuery,
    },
}

// `eyre::Report` has no meaningful notion of equality, so this can't be
// derived. Every other variant's equality is exactly what a derive would
// produce; `Fatal` is only ever compared to satisfy that (nothing needs to
// tell two fatal errors apart), so any two `Fatal`s compare equal to each
// other - a valid (if coarse) equivalence relation, since it's reflexive,
// symmetric, and transitive.
impl PartialEq for Effect {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Effect::Quit, Effect::Quit) | (Effect::Fatal(_), Effect::Fatal(_)) => true,
            (
                Effect::FetchScoreTile {
                    generation: left_generation,
                    query: left_query,
                    pagination: left_pagination,
                },
                Effect::FetchScoreTile {
                    generation: right_generation,
                    query: right_query,
                    pagination: right_pagination,
                },
            ) => {
                left_generation == right_generation
                    && left_query == right_query
                    && left_pagination == right_pagination
            }
            (
                Effect::SearchInvalidated {
                    generation: left_generation,
                    query: left_query,
                },
                Effect::SearchInvalidated {
                    generation: right_generation,
                    query: right_query,
                },
            ) => left_generation == right_generation && left_query == right_query,
            _ => false,
        }
    }
}

impl Eq for Effect {}
