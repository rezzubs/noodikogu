mod command_line;
pub(crate) mod context;
mod table;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Size},
    style::Color,
    widgets::{Block, BorderType},
};

use crate::catalogue::Pagination;
use crate::query::ScoreQuery;
use crate::tui::{Effect, Message};
use command_line::CommandLine;
use context::Context;
use context::search::{ScoreResult, SearchContext, TileFetchOutcome};

const NORMAL_COLOR: Color = Color::Reset;
const SELECTION_COLOR: Color = Color::Green;

/// Declares which of the two main panels is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Focus {
    #[default]
    CommandLine,
    Browser,
}

impl Focus {
    fn switch(&mut self) {
        *self = self.other();
    }

    fn other(self) -> Self {
        match self {
            Focus::CommandLine => Focus::Browser,
            Focus::Browser => Focus::CommandLine,
        }
    }
}

// Not `Hash`: `Context` (via `ScoreQuery`/catalogue result types nested
// inside it) isn't, and nothing needs `Model` to be either.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Model {
    /// The command line's text buffer and cursor.
    pub command_line: CommandLine,
    /// The currently focused panel.
    pub focus: Focus,
    /// The terminal's current size, kept in sync via [`Message::Resize`].
    ///
    /// `update` needs this to compute the command line's wrap width for
    /// vertical cursor movement (`Message::Up`/`Down`) - unlike `view`, it
    /// has no other access to layout information. `App::run` seeds this
    /// once at startup.
    pub terminal_size: Size,
    /// The Browser's current content - `None` until a search is entered or a
    /// score is opened.
    pub context: Option<Context>,
    /// Bumped every time `context` is replaced, so a fetch effect/response
    /// from a previous `context` can be told apart from a current one and
    /// dropped instead of corrupting the new state - see `SearchContext`'s
    /// use of it in `Model::update`.
    ///
    /// Bump via `wrapping_add(1)`, not `+=`/`saturating_add`: `+=` panics on
    /// overflow in debug builds, and `saturating_add` is actively wrong
    /// here, since once saturated it would keep minting the same value for
    /// every subsequent `context` replacement, defeating the one thing this
    /// field exists for. Wrapping back to `0` is safe because only a small
    /// window of recent generations can ever be concurrently relevant (a
    /// handful of in-flight fetches, at most); nothing can still be "in
    /// flight" from 2^64 generations ago to collide with a wrapped value.
    pub generation: u64,
}

impl Model {
    /// Update the model based on this message.
    ///
    /// Returns the effect the runtime should carry out, if any. Most messages
    /// only change the model, so [`None`] is the common answer.
    pub fn update(&mut self, message: Message) -> Option<Effect> {
        match message {
            Message::Left => {
                if self.focus == Focus::CommandLine {
                    self.command_line.move_left();
                }
            }
            Message::Right => {
                if self.focus == Focus::CommandLine {
                    self.command_line.move_right();
                }
            }
            Message::Up => {
                if self.focus == Focus::CommandLine {
                    self.command_line
                        .move_up(command_line_inner_width(self.terminal_size));
                } else if let Some(Context::Search(context)) = &mut self.context {
                    let viewport_height =
                        browser_inner_height(self.terminal_size, &self.command_line);
                    return context.move_up(self.generation, viewport_height);
                }
            }
            Message::Down => {
                if self.focus == Focus::CommandLine {
                    self.command_line
                        .move_down(command_line_inner_width(self.terminal_size));
                } else if let Some(Context::Search(context)) = &mut self.context {
                    let viewport_height =
                        browser_inner_height(self.terminal_size, &self.command_line);
                    return context.move_down(self.generation, viewport_height);
                }
            }
            Message::SwitchFocus => self.focus.switch(),
            Message::WriteCharacter(character) => self.command_line.insert_char(character),
            Message::DeleteCharacterBefore => self.command_line.delete_before(),
            Message::DeleteCharacterOn => self.command_line.delete_on(),
            // Unlike `Left`/`Right`/`Up`/`Down`, these are only ever emitted
            // by `keymap::command_line`, which only runs while the command
            // line is already focused - no runtime check needed, same as
            // `WriteCharacter`/`DeleteCharacterBefore`/`DeleteCharacterOn`
            // above.
            Message::MoveToLineStart => self.command_line.move_to_line_start(),
            Message::MoveToLineEnd => self.command_line.move_to_line_end(),
            Message::MoveWordLeft => self.command_line.move_word_left(),
            Message::MoveWordRight => self.command_line.move_word_right(),
            Message::DeleteWordBefore => self.command_line.delete_word_before(),
            Message::DeleteWordAfter => self.command_line.delete_word_after(),
            Message::DeleteToLineStart => self.command_line.delete_to_line_start(),
            Message::DeleteToLineEnd => self.command_line.delete_to_line_end(),
            Message::Quit => return Some(Effect::Quit),
            Message::CommandLineEOF => {
                if self.command_line.is_empty() {
                    return Some(Effect::Quit);
                }
                self.command_line.delete_on();
            }
            Message::Resize(width, height) => {
                self.terminal_size = Size::new(width, height);
                if let Some(Context::Search(context)) = &mut self.context {
                    let viewport_height =
                        browser_inner_height(self.terminal_size, &self.command_line);
                    return context.handle_resize(self.generation, viewport_height);
                }
            }
            Message::ScoreTileFetched {
                generation,
                pagination,
                results,
                query,
            } => return self.handle_score_tile_fetched(generation, pagination, results, query),
            Message::Fatal(error) => return Some(Effect::Fatal(error)),
        }

        None
    }

    /// Bump the generation counter for a case where the context becomes
    /// incompatible.
    fn bump_generation(&mut self) {
        // wrapping to handle the unlikely case of overflow. Wrapping is better
        // than saturating because the value at least changes. At the time of
        // writing `generation` is only checked for equality so the fact that
        // it got smaller is not an issue. The only way this can cause an issue
        // is if we somehow have 2^64 objects referencing different generations
        // which is impossible in practice.
        self.generation = self.generation.wrapping_add(1);
    }

    /// Handles [`Message::ScoreTileFetched`].
    fn handle_score_tile_fetched(
        &mut self,
        generation: u64,
        pagination: Pagination,
        results: Vec<ScoreResult>,
        query: ScoreQuery,
    ) -> Option<Effect> {
        if generation != self.generation {
            // The current `Context` is stale.
            return None;
        }

        match &mut self.context {
            Some(Context::Search(context)) => {
                let viewport_height = browser_inner_height(self.terminal_size, &self.command_line);
                match context.handle_tile_fetch(
                    self.generation,
                    pagination,
                    results,
                    viewport_height,
                ) {
                    TileFetchOutcome::Applied(effect) => effect,
                    // See ADR 0010: an undersized non-tail tile proves the
                    // cached tile set is stale. Discard it entirely rather
                    // than risk showing corrupted results, and restart the
                    // search from the top.
                    TileFetchOutcome::Stale => {
                        self.bump_generation();
                        self.context = None;
                        Some(Effect::SearchInvalidated {
                            generation: self.generation,
                            query,
                        })
                    }
                }
            }
            // Bootstrap: either a brand-new search's first tile, or the
            // re-fetch after an `Effect::SearchInvalidated` reset above -
            // both resolve into this same message.
            None => {
                let viewport_height = browser_inner_height(self.terminal_size, &self.command_line);
                let Some((context, maybe_effect)) =
                    SearchContext::bootstrap(query, self.generation, results, viewport_height)
                else {
                    // An empty first page - nothing to bootstrap.
                    return None;
                };
                self.context = Some(Context::Search(context));
                maybe_effect
            }
            Some(Context::Score(_)) => None,
        }
    }

    pub fn view(&self, frame: &mut Frame) {
        let area = frame.area();

        let browser = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(match self.focus {
                Focus::CommandLine => NORMAL_COLOR,
                Focus::Browser => SELECTION_COLOR,
            });

        let inner_width = command_line_inner_width(self.terminal_size);
        let outer_height = command_line_outer_height(self.terminal_size, &self.command_line);

        // The contained number in Fill has no meaning if only a single one
        // is used.
        let [browser_area, command_line_area] =
            Layout::vertical([Constraint::Fill(0), Constraint::Length(outer_height)]).areas(area);

        let command_line_view = self.command_line.view(
            inner_width,
            command_line_area,
            self.focus == Focus::CommandLine,
        );

        frame.render_widget(browser, browser_area);
        frame.render_widget(command_line_view.widget, command_line_area);

        if let Some(cursor_position) = command_line_view.cursor_position {
            frame.set_cursor_position(cursor_position);
        }
    }
}

/// Terminal columns available for command-line text, after its left/right
/// border.
///
/// Shared by `update` (vertical cursor movement) and `view` (rendering) so
/// the two can never silently disagree about wrap width - both derive it
/// from the same `terminal_size`, which is why `view` reads
/// `self.terminal_size` here rather than `frame.area().width` directly
/// (the two are numerically identical in practice, since only a vertical
/// split separates the browser and command line, but funneling both call
/// sites through one function removes a class of future divergence bugs).
fn command_line_inner_width(terminal_size: Size) -> u16 {
    // Subtracting 2 to account for block border on both sides.
    terminal_size.width.saturating_sub(2)
}

/// The command line's own box height (border included), the same value
/// `view` needs for its `Layout::vertical` split.
///
/// Extracted out of `view` so `browser_inner_height` below can share it
/// rather than recomputing the command line's desired height a third way -
/// same "one function, two call sites" reasoning as
/// [`command_line_inner_width`].
fn command_line_outer_height(terminal_size: Size, command_line: &CommandLine) -> u16 {
    let inner_width = command_line_inner_width(terminal_size);

    let outer_height_max = terminal_size.height / 2;
    // sub 2 for top and bottom borders.
    let inner_height_max = usize::from(outer_height_max.saturating_sub(2));
    let desired_inner_height = command_line
        .desired_height(inner_width)
        .min(inner_height_max);

    u16::try_from(desired_inner_height.saturating_add(2)).unwrap_or(u16::MAX)
}

/// Terminal rows available to the Browser's content, after its own
/// top/bottom border and whatever height the command line currently
/// occupies.
///
/// Unlike [`command_line_inner_width`] (a function of terminal width alone),
/// this isn't a fixed fraction of the terminal - it depends on the command
/// line's *current* (content-driven) height, so `update` needs the same
/// derivation `view` uses to lay out the two panels, for the same
/// never-silently-disagree reason `command_line_inner_width` is shared.
fn browser_inner_height(terminal_size: Size, command_line: &CommandLine) -> u16 {
    let command_line_outer_height = command_line_outer_height(terminal_size, command_line);
    // sub 2 for the browser's own top and bottom border.
    terminal_size
        .height
        .saturating_sub(command_line_outer_height)
        .saturating_sub(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::Pagination;
    use crate::query::{ScoreQuery, SearchAtom};
    use context::search::{TILE_SIZE, test_score_result};

    fn atom_query(title: &str) -> ScoreQuery {
        ScoreQuery::Atom(SearchAtom::Title(title.to_string()))
    }

    fn model() -> Model {
        // Height 4 drives `browser_inner_height` to 0, so `tiles_needed_for`
        // is 1 - the smallest the real formula can ever produce. Several
        // tests below rely on this to keep the number of tiles/keypresses
        // needed to reach a given cache state small.
        Model {
            terminal_size: Size::new(80, 4),
            ..Model::default()
        }
    }

    fn full_tile() -> Vec<ScoreResult> {
        vec![test_score_result(0); usize::try_from(TILE_SIZE).expect("TILE_SIZE fits usize")]
    }

    /// Asserts `effect` is a `FetchScoreTile` for `expected_offset` and
    /// returns its `Pagination`, for chaining into the next fetch response.
    fn expect_fetch(effect: Option<Effect>, expected_offset: u64) -> Pagination {
        match effect {
            Some(Effect::FetchScoreTile { pagination, .. }) => {
                assert_eq!(pagination.offset, expected_offset);
                assert_eq!(pagination.limit, TILE_SIZE);
                pagination
            }
            other => panic!(
                "expected a FetchScoreTile effect at offset {expected_offset}, got {other:?}"
            ),
        }
    }

    #[test]
    fn score_tile_fetched_with_a_stale_generation_is_ignored() {
        let mut model = model();
        model.generation = 5;

        let effect = model.update(Message::ScoreTileFetched {
            generation: 4,
            pagination: Pagination {
                offset: 0,
                limit: TILE_SIZE,
            },
            results: vec![test_score_result(1)],
            query: atom_query("test"),
        });

        assert!(effect.is_none());
        assert!(model.context.is_none());
    }

    #[test]
    fn score_tile_fetched_bootstraps_a_context_from_a_non_empty_first_page() {
        let mut model = model();

        model.update(Message::ScoreTileFetched {
            generation: 0,
            pagination: Pagination {
                offset: 0,
                limit: TILE_SIZE,
            },
            results: vec![test_score_result(1)],
            query: atom_query("test"),
        });

        assert!(matches!(model.context, Some(Context::Search(_))));
    }

    #[test]
    fn score_tile_fetched_with_an_empty_first_page_does_not_bootstrap() {
        let mut model = model();

        let effect = model.update(Message::ScoreTileFetched {
            generation: 0,
            pagination: Pagination {
                offset: 0,
                limit: TILE_SIZE,
            },
            results: Vec::new(),
            query: atom_query("test"),
        });

        assert!(effect.is_none());
        assert!(model.context.is_none());
    }

    /// End-to-end through `Model::update`. A
    /// bootstrap always lands at offset 0
    /// so getting `ensure_buffered` to request *behind* the front tile
    /// means scrolling far enough forward to evict it, then scrolling back.
    /// With `tiles_needed_for` at its floor of 1 (see [`model()`]), crossing
    /// two full tiles is enough to evict the first and trigger exactly that.
    #[test]
    fn score_tile_fetched_resets_the_context_when_a_non_tail_tile_is_undersized() {
        let mut model = model();
        // `Down`/`Up` only reach the Browser (and its `SearchContext`) when
        // it's focused - `Model::default()`'s focus starts on the command
        // line.
        model.focus = Focus::Browser;
        let query = atom_query("test");

        // Bootstrap at offset 0 with a full tile.
        let effect = model.update(Message::ScoreTileFetched {
            generation: 0,
            pagination: Pagination {
                offset: 0,
                limit: TILE_SIZE,
            },
            results: full_tile(),
            query: query.clone(),
        });
        let pagination = expect_fetch(effect, TILE_SIZE);

        // Resolve the auto-requested tile at offset 32, also full.
        let effect = model.update(Message::ScoreTileFetched {
            generation: 0,
            pagination,
            results: full_tile(),
            query: query.clone(),
        });
        assert!(
            effect.is_none(),
            "two full tiles already satisfy a 0-row viewport"
        );

        // Cross the first tile's rows - the last keypress also requests a
        // third tile ahead (offset 64), keeping the buffer one tile deep.
        let mut effect = None;
        for _ in 0..TILE_SIZE {
            effect = model.update(Message::Down);
        }
        let pagination = expect_fetch(effect, TILE_SIZE * 2);

        let effect = model.update(Message::ScoreTileFetched {
            generation: 0,
            pagination,
            results: full_tile(),
            query: query.clone(),
        });
        assert!(effect.is_none());

        // Cross the second tile's rows - this evicts the offset-0 tile
        // (selection is now far enough ahead of it) and requests a fourth
        // tile, offset 96 - not resolved, it gets evicted again below.
        for _ in 0..TILE_SIZE {
            model.update(Message::Down);
        }

        // Scroll back up one row: crosses into the (still-cached) offset-32
        // tile, evicts the now-unreachable offset-96 request, and - since
        // the new front tile's offset is > 0 - re-requests offset 0.
        let effect = model.update(Message::Up);
        let pagination = expect_fetch(effect, 0);

        // The re-request comes back short: something changed underneath the
        // cache while we were scrolled away from the top.
        let effect = model.update(Message::ScoreTileFetched {
            generation: 0,
            pagination,
            results: vec![test_score_result(999)],
            query: query.clone(),
        });

        assert_eq!(
            effect,
            Some(Effect::SearchInvalidated {
                generation: 1,
                query,
            })
        );
        assert!(
            model.context.is_none(),
            "the stale context should have been discarded"
        );
        assert_eq!(model.generation, 1);
    }

    #[test]
    fn bump_generation() {
        let mut model = model();

        let first_generation = model.generation;
        assert_eq!(first_generation, 0);
        model.bump_generation();
        assert_ne!(first_generation, model.generation);

        model.generation = u64::MAX;
        model.bump_generation();
        assert_ne!(model.generation, u64::MAX);
    }
}
