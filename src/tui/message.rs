use color_eyre::eyre;

use crate::catalogue::Pagination;
use crate::query::ScoreQuery;
use crate::tui::model::context::search::ScoreResult;

/// A message for the [`update`](crate::tui::Model::update) function.
///
/// Messages describe what happened in terms the application understands, not
/// which key produced them. Deciding what a message does is
/// [`Model::update`](crate::tui::Model::update)'s job, and the answer may well
/// be nothing.
///
/// Not `Clone`/`PartialEq`/`Eq`/`Hash`: [`eyre::Report`] isn't, and nothing
/// needs `Message` to be.
#[derive(Debug)]
pub enum Message {
    /// A movement in the left direction.
    Left,
    /// A movement in the right direction.
    Right,
    /// A movement in the up direction.
    Up,
    /// A movement in the down direction.
    Down,
    /// A request to focus the other main panel.
    SwitchFocus,
    /// Write a character at the cursor position in the command line.
    WriteCharacter(char),
    /// Delete the character before the cursor,
    DeleteCharacterBefore,
    /// Delete the character on the cursor,
    DeleteCharacterOn,
    /// Move to the start of the current logical line.
    MoveToLineStart,
    /// Move to the end of the current logical line.
    MoveToLineEnd,
    /// Move to the start of the previous word.
    MoveWordLeft,
    /// Move to the end of the next word.
    MoveWordRight,
    /// Delete the word before the cursor.
    DeleteWordBefore,
    /// Delete the word after the cursor.
    DeleteWordAfter,
    /// Delete from the start of the current logical line up to the cursor.
    DeleteToLineStart,
    /// Delete from the cursor up to the end of the current logical line.
    DeleteToLineEnd,
    /// A request to exit the application.
    Quit,
    /// Ctrl+d: forward-delete like the Delete key, or exit if the command
    /// line is already empty (traditional EOF behavior) - the same dual
    /// meaning Ctrl+d has in most shells.
    CommandLineEOF,
    /// The terminal was resized to (width, height).
    Resize(u16, u16),
    /// A background
    /// [`Effect::FetchScoreTile`](crate::tui::Effect::FetchScoreTile)
    /// (or the bootstrap fetch spawned for an
    /// [`Effect::SearchInvalidated`](crate::tui::Effect::SearchInvalidated))
    /// completed successfully.
    ScoreTileFetched {
        /// The [`Model`](crate::tui::model::Model) generation this fetch
        /// was issued under.
        generation: u64,
        /// The page that was fetched.
        pagination: Pagination,
        /// The fetched page's rows.
        results: Vec<ScoreResult>,
        /// The query the fetch was issued with - needed to construct a
        /// fresh `SearchContext` (bootstrap case) or re-issue
        /// `Effect::SearchInvalidated` (stale case) without threading extra
        /// state elsewhere.
        query: ScoreQuery,
    },
    /// A fatal error occured in the program and it must exit gracefully.
    ///
    /// This exists only to emit [`Effect::Fatal`](crate::tui::Effect::Fatal)
    /// and exit the program.
    Fatal(eyre::Report),
}
